// Recording orchestrator: spawns all threads and manages the recording session.
//
// Cross-platform structure:
// - File writer thread (cross-platform)
// - Cleanup thread (cross-platform)
// - Mic capture pipeline thread (Windows only)
// - Teams monitor thread (Windows only)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use anyhow::Result;

use crate::audio::file_writer::run_file_writer;
use crate::audio::pipeline::AudioMessage;
use crate::config::Config;

pub fn run_recorder(config: Config) -> Result<()> {
    let shutdown = Arc::new(AtomicBool::new(false));

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

    // --- Mic capture pipeline thread (Windows only) ---
    #[cfg(target_os = "windows")]
    let mic_handle = spawn_mic_pipeline(&config, sender.clone(), shutdown.clone())?;

    // --- Teams monitor thread (Windows only) ---
    #[cfg(target_os = "windows")]
    let teams_handle = spawn_teams_monitor(&config, sender.clone(), shutdown.clone())?;

    // --- Cleanup thread (cross-platform) ---
    let cleanup_dir = config.output.directory.clone();
    let cleanup_config = config.storage.clone();
    let cleanup_shutdown = shutdown.clone();
    let cleanup_handle = std::thread::Builder::new()
        .name("cleanup".into())
        .spawn(move || {
            crate::storage::run_cleanup_loop(cleanup_dir, cleanup_config, cleanup_shutdown);
        })?;

    // Drop our copy of the sender so the file writer's channel closes when
    // all capture threads finish.
    drop(sender);

    // Wait for shutdown signal.
    while !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    tracing::info!("Shutting down...");

    // Join threads. On non-Windows the mic/teams handles don't exist.
    #[cfg(target_os = "windows")]
    {
        if let Some(h) = mic_handle {
            let _ = h.join();
        }
        let _ = teams_handle.join();
    }

    let _ = cleanup_handle.join();
    let _ = writer_handle.join();

    tracing::info!("Shutdown complete");
    Ok(())
}

/// Spawn the mic capture pipeline thread with crash-recovery outer loop.
/// Returns `None` if mic capture is disabled in config.
#[cfg(target_os = "windows")]
fn spawn_mic_pipeline(
    config: &Config,
    sender: mpsc::Sender<AudioMessage>,
    shutdown: Arc<AtomicBool>,
) -> Result<Option<std::thread::JoinHandle<()>>> {
    if !config.targets.mic_enabled {
        return Ok(None);
    }

    let sample_rate = config.capture.sample_rate;
    let pre_speech_buffer_secs = config.vad.pre_speech_buffer_secs;
    let silence_threshold_secs = config.vad.silence_threshold_secs;
    let speech_threshold = config.vad.speech_threshold;

    let handle = std::thread::Builder::new()
        .name("mic-capture".into())
        .spawn(move || {
            // Outer recovery loop: restart on transient errors.
            while !shutdown.load(Ordering::Relaxed) {
                match crate::audio::capture::MicCapture::new(sample_rate) {
                    Ok(capture) => {
                        let capture_fn = || -> Result<Option<Vec<i16>>> { capture.read_frames() };
                        let start_fn = || -> Result<()> { capture.start() };

                        let chunk_size: usize = match sample_rate {
                            8000 => 256,
                            16000 => 512,
                            _ => 512,
                        };

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
                                ) {
                                    Ok(()) => break,
                                    Err(e) => {
                                        tracing::error!(
                                            "Mic pipeline error: {:?}, restarting in 2s",
                                            e
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to create VAD: {:?}, retrying in 2s", e);
                            }
                        }

                        if let Err(e) = capture.stop() {
                            tracing::warn!("Error stopping mic capture: {:?}", e);
                        }
                    }
                    Err(e) => {
                        tracing::error!("Mic init failed: {:?}, retrying in 2s", e);
                    }
                }

                if !shutdown.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
            }
        })?;

    Ok(Some(handle))
}

/// Spawn the Teams process monitor thread.
#[cfg(target_os = "windows")]
fn spawn_teams_monitor(
    config: &Config,
    sender: mpsc::Sender<AudioMessage>,
    shutdown: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>> {
    let teams_config = config.clone();
    let handle = std::thread::Builder::new()
        .name("teams-monitor".into())
        .spawn(move || {
            if let Err(e) =
                crate::audio::teams_monitor::run_teams_monitor(teams_config, sender, shutdown)
            {
                tracing::error!("Teams monitor error: {:?}", e);
            }
        })?;
    Ok(handle)
}
