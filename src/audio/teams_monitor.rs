// Teams process monitor: detects when Teams starts/stops and manages capture pipeline.
//
// `find_teams_pid` is cross-platform (uses sysinfo which works on all platforms).
// `run_teams_monitor` is Windows-only because it uses `TeamsCapture`.

use std::ffi::OsStr;
use sysinfo::{ProcessRefreshKind, RefreshKind, System};

/// Finds the PID of the first matching process from the given list of process names.
///
/// This function is cross-platform — it uses the `sysinfo` crate which works
/// on Windows, Linux, and macOS. Returns `None` if no matching process is found.
pub fn find_teams_pid(process_names: &[String]) -> Option<u32> {
    let refreshes = RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing());
    let system = System::new_with_specifics(refreshes);
    for name in process_names {
        let os_name = OsStr::new(name);
        let mut procs = system.processes_by_name(os_name);
        if let Some(proc_) = procs.next() {
            return Some(proc_.pid().as_u32());
        }
    }
    None
}

// --- Windows-only monitor that spawns the Teams capture pipeline ---

#[cfg(target_os = "windows")]
mod monitor {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::Arc;

    use anyhow::Result;

    use crate::audio::pipeline::{run_capture_pipeline, AudioMessage};
    use crate::audio::teams_capture::TeamsCapture;
    use crate::audio::vad::Vad;
    use crate::config::Config;

    use super::find_teams_pid;

    /// Monitors for Teams process and spawns/stops capture pipeline accordingly.
    ///
    /// Polls every 5 seconds for the Teams process. When detected, creates a
    /// `TeamsCapture` and runs the audio pipeline. When the process disappears,
    /// shuts down the pipeline and waits for the next appearance.
    pub fn run_teams_monitor(
        config: Config,
        sender: Sender<AudioMessage>,
        shutdown: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
    ) -> Result<()> {
        let mut active_pid: Option<u32> = None;
        let mut pipeline_shutdown: Option<Arc<AtomicBool>> = None;
        let mut pipeline_handle: Option<std::thread::JoinHandle<()>> = None;

        while !shutdown.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let current_pid = find_teams_pid(&config.targets.processes);

            match (active_pid, current_pid) {
                (None, Some(pid)) => {
                    // Teams just started — spawn capture pipeline.
                    tracing::info!("Teams detected (PID {}), starting capture", pid);
                    let pipe_shutdown = Arc::new(AtomicBool::new(false));
                    let pipe_shutdown_clone = pipe_shutdown.clone();
                    let sender_clone = sender.clone();
                    let paused_clone = paused.clone();
                    let sample_rate = config.capture.sample_rate;
                    let pre_speech_buffer_secs = config.vad.pre_speech_buffer_secs;
                    let silence_threshold_secs = config.vad.silence_threshold_secs;
                    let speech_threshold = config.vad.speech_threshold;

                    let handle = std::thread::Builder::new()
                        .name("teams-capture".into())
                        .spawn(move || {
                            match TeamsCapture::new(pid, sample_rate) {
                                Ok(capture) => {
                                    let capture_fn = || -> Result<Option<Vec<i16>>> {
                                        Ok(capture.read_frames()?)
                                    };
                                    let start_fn = || -> Result<()> { capture.start() };

                                    // Determine VAD chunk size based on sample rate.
                                    let chunk_size: usize = match sample_rate {
                                        8000 => 256,
                                        16000 => 512,
                                        _ => 512,
                                    };

                                    match Vad::new(sample_rate, speech_threshold) {
                                        Ok(mut vad) => {
                                            if let Err(e) = run_capture_pipeline(
                                                "teams".to_string(),
                                                capture_fn,
                                                start_fn,
                                                sample_rate,
                                                pre_speech_buffer_secs,
                                                silence_threshold_secs,
                                                &mut vad,
                                                chunk_size,
                                                sender_clone,
                                                pipe_shutdown_clone,
                                                paused_clone,
                                            ) {
                                                tracing::error!("Teams pipeline error: {:?}", e);
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!("Failed to create VAD: {:?}", e);
                                        }
                                    }

                                    if let Err(e) = capture.stop() {
                                        tracing::warn!("Error stopping Teams capture: {:?}", e);
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to start Teams capture for PID {}: {:?}",
                                        pid,
                                        e
                                    );
                                }
                            }
                        })?;

                    active_pid = Some(pid);
                    pipeline_shutdown = Some(pipe_shutdown);
                    pipeline_handle = Some(handle);
                }
                (Some(_), None) => {
                    // Teams process gone — stop capture.
                    tracing::info!("Teams process gone, stopping capture");
                    if let Some(ps) = pipeline_shutdown.take() {
                        ps.store(true, Ordering::Relaxed);
                    }
                    if let Some(handle) = pipeline_handle.take() {
                        let _ = handle.join();
                    }
                    active_pid = None;
                }
                (Some(old_pid), Some(new_pid)) if old_pid != new_pid => {
                    // Teams PID changed (process restarted) — restart capture.
                    tracing::info!(
                        "Teams PID changed {} -> {}, restarting capture",
                        old_pid,
                        new_pid
                    );
                    if let Some(ps) = pipeline_shutdown.take() {
                        ps.store(true, Ordering::Relaxed);
                    }
                    if let Some(handle) = pipeline_handle.take() {
                        let _ = handle.join();
                    }
                    // Set active_pid to None so next iteration picks up the new PID.
                    active_pid = None;
                }
                _ => {
                    // No change — continue polling.
                }
            }
        }

        // Shutdown: clean up any active pipeline.
        if let Some(ps) = pipeline_shutdown.take() {
            ps.store(true, Ordering::Relaxed);
        }
        if let Some(handle) = pipeline_handle.take() {
            let _ = handle.join();
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub use monitor::run_teams_monitor;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_nonexistent_process() {
        let pid = find_teams_pid(&["definitely-not-a-real-process-12345.exe".to_string()]);
        assert!(pid.is_none());
    }

    #[test]
    fn test_find_empty_process_list() {
        let pid = find_teams_pid(&[]);
        assert!(pid.is_none());
    }
}
