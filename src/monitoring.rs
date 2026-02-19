// Pipeline health monitoring: watchdog, recording gap timer, toast notifications.
//
// - `run_watchdog`: checks pipeline thread health, triggers self-restart on failure.
// - `run_gap_timer`: checks for recording gaps, fires toast notifications.
// - `send_toast`: Windows toast notification helper.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Sends a Windows toast notification with the given title and body.
///
/// Uses the Windows `ToastNotification` API via the `windows` crate.
/// Falls back to tracing::warn if toast fails (e.g. no notification permission).
#[cfg(target_os = "windows")]
pub fn send_toast(title: &str, body: &str) {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    let toast_xml = format!(
        r#"<toast>
            <visual>
                <binding template="ToastGeneric">
                    <text>{}</text>
                    <text>{}</text>
                </binding>
            </visual>
        </toast>"#,
        xml_escape(title),
        xml_escape(body),
    );

    let result = (|| -> anyhow::Result<()> {
        let doc = XmlDocument::new()?;
        doc.LoadXml(&windows::core::HSTRING::from(&toast_xml))?;

        let toast = ToastNotification::CreateToastNotification(&doc)?;

        // Use a well-known AppUserModelId. The Windows PowerShell AUMID works
        // without needing to register our own.
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(
            &windows::core::HSTRING::from("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe"),
        )?;

        notifier.Show(&toast)?;
        Ok(())
    })();

    if let Err(e) = result {
        tracing::warn!("Failed to show toast notification: {:?}", e);
    }
}

/// No-op on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
pub fn send_toast(title: &str, body: &str) {
    tracing::info!("Toast (non-Windows): {} - {}", title, body);
}

/// XML-escape a string for use in toast XML.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Watchdog: monitors pipeline thread health. If any thread has exited unexpectedly,
/// fires a toast and triggers a self-restart.
///
/// `thread_handles` is a list of (name, is_finished_fn) for each thread to monitor.
/// Using closures allows testing without real threads.
pub fn run_watchdog<F>(
    shutdown: Arc<AtomicBool>,
    is_any_thread_dead: F,
) where
    F: Fn() -> Option<String>, // returns Some(thread_name) if a thread has died
{
    while !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_secs(10));

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        if let Some(dead_thread) = is_any_thread_dead() {
            tracing::error!(
                "Watchdog: {} thread has died unexpectedly, triggering restart",
                dead_thread
            );
            send_toast(
                "deskmic: Recording stopped",
                &format!(
                    "The {} thread died unexpectedly. Restarting...",
                    dead_thread
                ),
            );

            // Attempt self-restart.
            match self_restart() {
                Ok(()) => {
                    tracing::info!("Watchdog: self-restart initiated, exiting current process");
                    std::process::exit(0);
                }
                Err(e) => {
                    tracing::error!("Watchdog: self-restart failed: {:?}", e);
                    send_toast(
                        "deskmic: Restart failed",
                        "Recording failed and could not auto-restart. Please restart manually.",
                    );
                    // Continue running in degraded state rather than exiting.
                }
            }
        }
    }
}

/// Spawn a new instance of this executable and return Ok if successful.
fn self_restart() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe).spawn()?;
    Ok(())
}

/// Determines whether a recording gap alert should be fired.
///
/// Returns `true` if:
/// - `newest_wav_time` is `Some` and older than `gap_mins` from `now`
/// - `newest_wav_time` is `None` and `process_start` is older than `gap_mins` from `now`
///
/// This is a pure function extracted for testability.
pub fn should_alert_gap(
    newest_wav_time: Option<SystemTime>,
    process_start: SystemTime,
    now: SystemTime,
    gap_mins: u32,
) -> bool {
    if gap_mins == 0 {
        return false; // disabled
    }

    let threshold = Duration::from_secs(gap_mins as u64 * 60);

    match newest_wav_time {
        Some(t) => now.duration_since(t).unwrap_or(Duration::ZERO) > threshold,
        None => now.duration_since(process_start).unwrap_or(Duration::ZERO) > threshold,
    }
}

/// Find the newest `.wav` file in today's date folder under `recordings_dir`.
/// Returns its modification time, or `None` if no WAV files exist.
pub fn newest_wav_in_today(recordings_dir: &Path) -> Option<SystemTime> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let today_dir = recordings_dir.join(&today);

    if !today_dir.exists() {
        return None;
    }

    let mut newest: Option<SystemTime> = None;

    if let Ok(entries) = std::fs::read_dir(&today_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("wav") {
                if let Ok(metadata) = path.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        newest = Some(match newest {
                            Some(prev) if modified > prev => modified,
                            Some(prev) => prev,
                            None => modified,
                        });
                    }
                }
            }
        }
    }

    newest
}

/// Recording gap timer: checks for recording gaps and fires toast notifications.
///
/// Every 60 seconds, checks if the newest WAV file in today's folder is older
/// than `gap_mins` minutes. If so, fires a toast notification (once per gap).
pub fn run_gap_timer(
    recordings_dir: PathBuf,
    gap_mins: u32,
    shutdown: Arc<AtomicBool>,
) {
    if gap_mins == 0 {
        tracing::info!("Recording gap alerts disabled (gap_mins = 0)");
        return;
    }

    let process_start = SystemTime::now();
    let mut alerted = false;

    while !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_secs(60));

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let newest = newest_wav_in_today(&recordings_dir);
        let now = SystemTime::now();

        if should_alert_gap(newest, process_start, now, gap_mins) {
            if !alerted {
                tracing::warn!(
                    "No audio recorded in the last {} minutes",
                    gap_mins
                );
                send_toast(
                    "deskmic: Recording gap",
                    &format!(
                        "No audio recorded in the last {} minutes. Recording may have stopped.",
                        gap_mins
                    ),
                );
                alerted = true;
            }
        } else {
            // Reset alert flag when a new recording appears.
            alerted = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_alert_gap_no_wav_within_threshold() {
        let start = SystemTime::now() - Duration::from_secs(60 * 20); // 20 mins ago
        let now = SystemTime::now();
        // No WAV files, but only 20 mins since start, threshold is 30 mins.
        assert!(!should_alert_gap(None, start, now, 30));
    }

    #[test]
    fn test_should_alert_gap_no_wav_past_threshold() {
        let start = SystemTime::now() - Duration::from_secs(60 * 35); // 35 mins ago
        let now = SystemTime::now();
        // No WAV files, 35 mins since start, threshold is 30 mins — alert.
        assert!(should_alert_gap(None, start, now, 30));
    }

    #[test]
    fn test_should_alert_gap_recent_wav() {
        let start = SystemTime::now() - Duration::from_secs(60 * 60); // 1 hour ago
        let wav_time = SystemTime::now() - Duration::from_secs(60 * 10); // 10 mins ago
        let now = SystemTime::now();
        // WAV is 10 mins old, threshold is 30 mins — no alert.
        assert!(!should_alert_gap(Some(wav_time), start, now, 30));
    }

    #[test]
    fn test_should_alert_gap_old_wav() {
        let start = SystemTime::now() - Duration::from_secs(60 * 60);
        let wav_time = SystemTime::now() - Duration::from_secs(60 * 40); // 40 mins ago
        let now = SystemTime::now();
        // WAV is 40 mins old, threshold is 30 mins — alert.
        assert!(should_alert_gap(Some(wav_time), start, now, 30));
    }

    #[test]
    fn test_should_alert_gap_disabled() {
        let start = SystemTime::now() - Duration::from_secs(60 * 60);
        let now = SystemTime::now();
        // gap_mins = 0 means disabled.
        assert!(!should_alert_gap(None, start, now, 0));
    }

    #[test]
    fn test_should_alert_gap_custom_threshold() {
        let start = SystemTime::now() - Duration::from_secs(60 * 20);
        let now = SystemTime::now();
        // 20 mins since start, threshold is 15 mins — alert.
        assert!(should_alert_gap(None, start, now, 15));
    }

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("hello"), "hello");
        assert_eq!(xml_escape("<b>bold</b>"), "&lt;b&gt;bold&lt;/b&gt;");
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape(r#"say "hi""#), "say &quot;hi&quot;");
    }

    #[test]
    fn test_newest_wav_in_nonexistent_dir() {
        let result = newest_wav_in_today(Path::new("/nonexistent/path/recordings"));
        assert!(result.is_none());
    }

    #[test]
    fn test_newest_wav_in_empty_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let today_dir = tmp.path().join(&today);
        std::fs::create_dir_all(&today_dir).unwrap();

        let result = newest_wav_in_today(tmp.path());
        assert!(result.is_none());
    }

    #[test]
    fn test_newest_wav_finds_newest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let today_dir = tmp.path().join(&today);
        std::fs::create_dir_all(&today_dir).unwrap();

        // Create two WAV files with a small time gap.
        std::fs::write(today_dir.join("old.wav"), b"old").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(today_dir.join("new.wav"), b"new").unwrap();

        // Also create a non-WAV file that's even newer — should be ignored.
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(today_dir.join("transcript.txt"), b"text").unwrap();

        let result = newest_wav_in_today(tmp.path());
        assert!(result.is_some());

        // The newest WAV should be new.wav, which should be more recent than old.wav.
        let new_meta = today_dir.join("new.wav").metadata().unwrap();
        let expected = new_meta.modified().unwrap();
        assert_eq!(result.unwrap(), expected);
    }
}
