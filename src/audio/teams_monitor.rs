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

/// Checks whether a process with the given PID is still alive.
pub fn is_process_alive(pid: u32) -> bool {
    let refreshes = RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing());
    let system = System::new_with_specifics(refreshes);
    system
        .process(sysinfo::Pid::from_u32(pid))
        .is_some()
}

/// Action to take when evaluating Teams PID changes.
#[derive(Debug, PartialEq)]
pub enum PidAction {
    /// No Teams process found, and none was active. Do nothing.
    NoChange,
    /// Teams just appeared — start capture on this PID.
    StartCapture(u32),
    /// Teams process disappeared — stop capture.
    StopCapture,
    /// A different PID was found but old PID is still alive — keep current capture.
    KeepCurrent,
    /// Old PID is dead and a new one is available — tear down and restart.
    RestartCapture(u32),
}

/// Determines what action to take given the current active PID, the newly found PID,
/// and whether the old process is still alive. This is the core decision logic
/// extracted from `run_teams_monitor` for testability.
pub fn decide_pid_action(
    active_pid: Option<u32>,
    found_pid: Option<u32>,
    is_old_alive: impl Fn(u32) -> bool,
) -> PidAction {
    match (active_pid, found_pid) {
        (None, None) => PidAction::NoChange,
        (None, Some(pid)) => PidAction::StartCapture(pid),
        (Some(_), None) => PidAction::StopCapture,
        (Some(old), Some(new)) if old == new => PidAction::NoChange,
        (Some(old), Some(new)) => {
            if is_old_alive(old) {
                PidAction::KeepCurrent
            } else {
                PidAction::RestartCapture(new)
            }
        }
    }
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

    use super::{decide_pid_action, find_teams_pid, is_process_alive, PidAction};

    /// Monitors for Teams process and spawns/stops capture pipeline accordingly.
    ///
    /// Polls every 5 seconds for the Teams process. When detected, creates a
    /// `TeamsCapture` and runs the audio pipeline. When the process disappears,
    /// shuts down the pipeline and waits for the next appearance.
    ///
    /// Uses `decide_pid_action` to handle PID changes correctly: if the old PID
    /// is still alive but a different PID is found (Teams runs multiple processes),
    /// we keep the current capture instead of tearing down and restarting.
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

            let action = decide_pid_action(active_pid, current_pid, is_process_alive);

            match action {
                PidAction::StartCapture(pid) => {
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
                PidAction::StopCapture => {
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
                PidAction::RestartCapture(new_pid) => {
                    // Old PID is dead, new PID found — tear down and restart.
                    tracing::info!(
                        "Teams PID changed (old process dead), restarting capture on PID {}",
                        new_pid,
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
                PidAction::KeepCurrent => {
                    // Different PID found but old PID is still alive — ignore.
                    // This fixes #13: Teams runs multiple processes, non-deterministic
                    // iteration order causes different PIDs each poll.
                    tracing::debug!(
                        "Teams returned different PID but active process still alive, keeping current capture"
                    );
                }
                PidAction::NoChange => {
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

    #[test]
    fn test_pid_action_no_teams_no_active() {
        let action = decide_pid_action(None, None, |_| false);
        assert_eq!(action, PidAction::NoChange);
    }

    #[test]
    fn test_pid_action_teams_just_started() {
        let action = decide_pid_action(None, Some(1234), |_| false);
        assert_eq!(action, PidAction::StartCapture(1234));
    }

    #[test]
    fn test_pid_action_teams_disappeared() {
        let action = decide_pid_action(Some(1234), None, |_| false);
        assert_eq!(action, PidAction::StopCapture);
    }

    #[test]
    fn test_pid_action_same_pid_no_change() {
        let action = decide_pid_action(Some(1234), Some(1234), |_| true);
        assert_eq!(action, PidAction::NoChange);
    }

    #[test]
    fn test_pid_action_different_pid_old_alive_keeps_current() {
        // This is the key fix for #13: old PID alive + different PID found = keep current
        let action = decide_pid_action(Some(1234), Some(5678), |pid| {
            assert_eq!(pid, 1234); // should check the OLD pid
            true // old pid is alive
        });
        assert_eq!(action, PidAction::KeepCurrent);
    }

    #[test]
    fn test_pid_action_different_pid_old_dead_restarts() {
        // Old PID is dead + different PID found = restart on new PID
        let action = decide_pid_action(Some(1234), Some(5678), |pid| {
            assert_eq!(pid, 1234);
            false // old pid is dead
        });
        assert_eq!(action, PidAction::RestartCapture(5678));
    }
}
