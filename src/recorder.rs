// Recording orchestrator: spawns all threads and manages the recording session.
//
// Cross-platform structure:
// - File writer thread (cross-platform)
// - Cleanup thread (cross-platform)
// - Mic capture pipeline thread (Windows only)
// - Teams monitor thread (Windows only)
// - System tray thread (Windows only)
// - Transcription child process watchdog thread (cross-platform)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use anyhow::Result;

use crate::audio::file_writer::run_file_writer;
use crate::audio::pipeline::AudioMessage;
use crate::config::Config;

pub fn run_recorder(config: Config, _config_path: Option<std::path::PathBuf>) -> Result<()> {
    let shutdown = Arc::new(AtomicBool::new(false));
    #[allow(unused_variables)]
    let paused = Arc::new(AtomicBool::new(false));

    // Set up Ctrl+C handler.
    let shutdown_ctrlc = shutdown.clone();
    ctrlc::set_handler(move || {
        tracing::info!("Shutdown signal received");
        shutdown_ctrlc.store(true, Ordering::Relaxed);
    })?;

    let (sender, receiver) = mpsc::channel::<AudioMessage>();

    // --- File writer thread (cross-platform) ---
    let output_config = config.output.clone();
    let sample_rate = config.capture.sample_rate;
    let writer_handle = std::thread::Builder::new()
        .name("file-writer".into())
        .spawn(move || {
            if let Err(e) = run_file_writer(receiver, &output_config, sample_rate) {
                tracing::error!("File writer error: {:?}", e);
            }
        })?;

    // --- System tray thread (Windows only) ---
    #[cfg(target_os = "windows")]
    let tray_handle = {
        let recordings_dir = config.output.directory.clone();
        let tray_shutdown = shutdown.clone();
        let tray_paused = paused.clone();
        std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || {
                if let Err(e) =
                    crate::tray::run_tray(recordings_dir, _config_path.clone(), tray_shutdown, tray_paused)
                {
                    tracing::error!("Tray error: {:?}", e);
                }
            })?
    };

    // --- Mic capture pipeline thread (Windows only) ---
    #[cfg(target_os = "windows")]
    let mic_alive = Arc::new(AtomicBool::new(true));
    #[cfg(target_os = "windows")]
    let mic_handle = spawn_mic_pipeline(
        &config,
        sender.clone(),
        shutdown.clone(),
        paused.clone(),
        mic_alive.clone(),
    )?;

    // --- Teams monitor thread (Windows only) ---
    #[cfg(target_os = "windows")]
    let teams_alive = Arc::new(AtomicBool::new(true));
    #[cfg(target_os = "windows")]
    let teams_handle = spawn_teams_monitor(
        &config,
        sender.clone(),
        shutdown.clone(),
        paused.clone(),
        teams_alive.clone(),
    )?;

    // --- Cleanup thread (cross-platform) ---
    let cleanup_dir = config.output.directory.clone();
    let cleanup_config = config.storage.clone();
    let cleanup_shutdown = shutdown.clone();
    let cleanup_handle = std::thread::Builder::new()
        .name("cleanup".into())
        .spawn(move || {
            crate::storage::run_cleanup_loop(cleanup_dir, cleanup_config, cleanup_shutdown);
        })?;

    // --- Transcription child process watchdog thread ---
    let transcribe_shutdown = shutdown.clone();
    let transcribe_handle = std::thread::Builder::new()
        .name("transcribe-watchdog".into())
        .spawn(move || {
            run_transcription_watchdog(transcribe_shutdown);
        })?;

    // --- Pipeline health watchdog thread ---
    // Monitors pipeline threads and triggers self-restart if any die.
    #[cfg(target_os = "windows")]
    let watchdog_handle = {
        let wd_shutdown = shutdown.clone();
        let wd_mic_alive = mic_alive.clone();
        let wd_teams_alive = teams_alive.clone();
        let mic_enabled = config.targets.mic_enabled;

        std::thread::Builder::new()
            .name("watchdog".into())
            .spawn(move || {
                crate::monitoring::run_watchdog(wd_shutdown, move || {
                    if mic_enabled && !wd_mic_alive.load(Ordering::Relaxed) {
                        return Some("mic-capture".to_string());
                    }
                    if !wd_teams_alive.load(Ordering::Relaxed) {
                        return Some("teams-monitor".to_string());
                    }
                    None
                });
            })?
    };

    // --- Recording gap timer thread ---
    let gap_timer_handle = {
        let gap_shutdown = shutdown.clone();
        let gap_mins = config.monitoring.recording_gap_alert_mins;
        let recordings_dir = config.output.directory.clone();

        std::thread::Builder::new()
            .name("gap-timer".into())
            .spawn(move || {
                crate::monitoring::run_gap_timer(
                    recordings_dir,
                    gap_mins,
                    gap_shutdown,
                );
            })?
    };

    // Drop our copy of the sender so the file writer's channel closes when
    // all capture threads finish.
    drop(sender);

    // Wait for shutdown signal.
    while !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    tracing::info!("Shutting down...");

    // Join threads. On non-Windows the mic/teams/tray/watchdog handles don't exist.
    #[cfg(target_os = "windows")]
    {
        if let Some(h) = mic_handle {
            let _ = h.join();
        }
        let _ = teams_handle.join();
        let _ = tray_handle.join();
        let _ = watchdog_handle.join();
    }

    let _ = cleanup_handle.join();
    let _ = writer_handle.join();
    let _ = transcribe_handle.join();
    let _ = gap_timer_handle.join();

    tracing::info!("Shutdown complete");
    Ok(())
}

/// Spawn `deskmic transcribe --watch` as a child process, respawning on crash.
/// The child process acquires its own mutex ("Global\deskmic-transcriber") to
/// prevent duplicates. When the parent's shutdown flag is set, the child is killed.
fn run_transcription_watchdog(shutdown: Arc<AtomicBool>) {
    const INITIAL_BACKOFF_SECS: u64 = 5;
    const MAX_BACKOFF_SECS: u64 = 60;
    let mut backoff_secs = INITIAL_BACKOFF_SECS;

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Cannot find own executable for transcription child: {:?}", e);
            return;
        }
    };

    while !shutdown.load(Ordering::Relaxed) {
        tracing::info!("Starting transcription child process");

        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("transcribe").arg("--watch");

        // Inherit stdout/stderr so transcription logs appear in the same log stream.
        // Suppress stdin so the child doesn't try to read from the console.
        cmd.stdin(std::process::Stdio::null());

        match cmd.spawn() {
            Ok(mut child) => {
                // Reset backoff on successful spawn.
                backoff_secs = INITIAL_BACKOFF_SECS;

                // Poll the child periodically. If shutdown is requested, kill it.
                loop {
                    if shutdown.load(Ordering::Relaxed) {
                        tracing::info!("Killing transcription child process");
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }

                    match child.try_wait() {
                        Ok(Some(status)) => {
                            if status.success() {
                                tracing::info!("Transcription child exited normally");
                            } else {
                                tracing::warn!(
                                    "Transcription child exited with status: {}",
                                    status
                                );
                            }
                            break; // exit inner loop to respawn
                        }
                        Ok(None) => {
                            // Still running, check again soon.
                            std::thread::sleep(std::time::Duration::from_secs(2));
                        }
                        Err(e) => {
                            tracing::error!("Error checking transcription child: {:?}", e);
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to spawn transcription child: {:?}", e);
            }
        }

        // Backoff before respawning.
        if !shutdown.load(Ordering::Relaxed) {
            tracing::info!(
                "Transcription child will restart in {}s",
                backoff_secs
            );
            // Sleep in small increments so we can respond to shutdown quickly.
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(backoff_secs);
            while std::time::Instant::now() < deadline && !shutdown.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
        }
    }
}

/// Spawn the mic capture pipeline thread with crash-recovery outer loop.
/// Returns `None` if mic capture is disabled in config.
#[cfg(target_os = "windows")]
fn spawn_mic_pipeline(
    config: &Config,
    sender: mpsc::Sender<AudioMessage>,
    shutdown: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
) -> Result<Option<std::thread::JoinHandle<()>>> {
    if !config.targets.mic_enabled {
        alive.store(false, Ordering::Relaxed); // not started, so not "alive"
        return Ok(None);
    }

    let sample_rate = config.capture.sample_rate;
    let pre_speech_buffer_secs = config.vad.pre_speech_buffer_secs;
    let silence_threshold_secs = config.vad.silence_threshold_secs;
    let speech_threshold = config.vad.speech_threshold;

    let handle = std::thread::Builder::new()
        .name("mic-capture".into())
        .spawn(move || {
            // Exponential backoff: starts at 2s, doubles each failure, caps at 30s.
            const INITIAL_BACKOFF_SECS: u64 = 2;
            const MAX_BACKOFF_SECS: u64 = 30;
            let mut backoff_secs: u64 = INITIAL_BACKOFF_SECS;

            // Outer recovery loop: restart on transient errors.
            while !shutdown.load(Ordering::Relaxed) {
                match crate::audio::capture::MicCapture::new(sample_rate) {
                    Ok(capture) => {
                        let capture_fn =
                            || -> Result<Option<Vec<i16>>> { Ok(capture.read_frames()?) };
                        let start_fn = || -> Result<()> { capture.start() };

                        let chunk_size: usize = match sample_rate {
                            8000 => 256,
                            16000 => 512,
                            _ => 512,
                        };

                        // If we got this far, device initialised — reset backoff.
                        backoff_secs = INITIAL_BACKOFF_SECS;

                        match crate::audio::vad::Vad::new(sample_rate, speech_threshold) {
                            Ok(mut vad) => {
                                match crate::audio::pipeline::run_capture_pipeline(
                                    "mic".to_string(),
                                    capture_fn,
                                    start_fn,
                                    sample_rate,
                                    pre_speech_buffer_secs,
                                    silence_threshold_secs,
                                    &mut vad,
                                    chunk_size,
                                    sender.clone(),
                                    shutdown.clone(),
                                    paused.clone(),
                                ) {
                                    Ok(()) => {
                                        // Pipeline exited cleanly (shutdown flag set) — this is normal.
                                        // But if shutdown wasn't requested, this is unexpected and we
                                        // should retry (the pipeline shouldn't exit on its own).
                                        if shutdown.load(Ordering::Relaxed) {
                                            break;
                                        }
                                        tracing::warn!(
                                            "Mic pipeline exited unexpectedly, retrying in {}s",
                                            backoff_secs
                                        );
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "Mic pipeline error: {:?}, restarting in {}s",
                                            e,
                                            backoff_secs
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to create VAD: {:?}, retrying in {}s",
                                    e,
                                    backoff_secs
                                );
                            }
                        }

                        if let Err(e) = capture.stop() {
                            tracing::warn!("Error stopping mic capture: {:?}", e);
                        }
                    }
                    Err(e) => {
                        tracing::error!("Mic init failed: {:?}, retrying in {}s", e, backoff_secs);
                    }
                }

                if !shutdown.load(Ordering::Relaxed) {
                    tracing::info!(
                        "Mic recovery: sleeping {}s before retry (device may be waking up)",
                        backoff_secs
                    );
                    std::thread::sleep(std::time::Duration::from_secs(backoff_secs));
                    backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
                }
            }

            // Thread is exiting — mark as not alive.
            alive.store(false, Ordering::Relaxed);
        })?;

    Ok(Some(handle))
}

/// Spawn the Teams process monitor thread.
#[cfg(target_os = "windows")]
fn spawn_teams_monitor(
    config: &Config,
    sender: mpsc::Sender<AudioMessage>,
    shutdown: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>> {
    let teams_config = config.clone();
    let handle = std::thread::Builder::new()
        .name("teams-monitor".into())
        .spawn(move || {
            if let Err(e) = crate::audio::teams_monitor::run_teams_monitor(
                teams_config,
                sender,
                shutdown,
                paused,
            ) {
                tracing::error!("Teams monitor error: {:?}", e);
            }
            // Thread is exiting — mark as not alive.
            alive.store(false, Ordering::Relaxed);
        })?;
    Ok(handle)
}
