// System tray UI for deskmic (Windows only).
//
// Provides pause/resume, open recordings folder, open settings, and quit actions.
// Also displays transcription status from the status file written by the
// transcriber child process.
// Requires a Win32 message pump to process tray icon events.

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;

use crate::transcribe::status::TranscriptionStatus;

/// How often to poll the transcription status file (seconds).
const STATUS_POLL_INTERVAL_SECS: u64 = 5;

/// Run the system tray UI on the current thread.
///
/// This function blocks until `shutdown` is set to `true`. It pumps Win32
/// messages so that `tray-icon` menu events are delivered.
pub fn run_tray(
    recordings_dir: std::path::PathBuf,
    config_path: Option<std::path::PathBuf>,
    shutdown: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) -> Result<()> {
    // Build menu
    let menu = Menu::new();
    let status_item = MenuItem::new("Status: Recording", false, None);
    let pause_item = MenuItem::new("Pause", true, None);
    let resume_item = MenuItem::new("Resume", true, None);
    let open_folder_item = MenuItem::new("Open Recordings", true, None);
    let settings_item = MenuItem::new("Settings", true, None);

    // Transcription status items (grayed-out, informational only)
    let tx_status_item = MenuItem::new("Transcriber: starting...", false, None);
    let tx_queue_item = MenuItem::new("Queue: -", false, None);
    let tx_session_item = MenuItem::new("Session: -", false, None);
    let tx_cpu_item = MenuItem::new("CPU: -", false, None);

    let quit_item = MenuItem::new("Quit", true, None);

    menu.append(&status_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&pause_item)?;
    menu.append(&resume_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&tx_status_item)?;
    menu.append(&tx_queue_item)?;
    menu.append(&tx_session_item)?;
    menu.append(&tx_cpu_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&open_folder_item)?;
    menu.append(&settings_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;

    // Create a simple 16×16 red icon (RGBA).
    let icon = Icon::from_rgba(vec![255, 0, 0, 255].repeat(16 * 16), 16, 16)?;

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("deskmic - Recording")
        .with_icon(icon)
        .build()?;

    let mut last_status_poll = Instant::now();

    // Event loop — process menu events + pump Win32 messages.
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Process menu events
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == quit_item.id() {
                shutdown.store(true, Ordering::Relaxed);
                break;
            } else if event.id == pause_item.id() {
                paused.store(true, Ordering::Relaxed);
                status_item.set_text("Status: Paused");
            } else if event.id == resume_item.id() {
                paused.store(false, Ordering::Relaxed);
                status_item.set_text("Status: Recording");
            } else if event.id == open_folder_item.id() {
                let _ = std::process::Command::new("explorer")
                    .arg(&recordings_dir)
                    .spawn();
            } else if event.id == settings_item.id() {
                let path = config_path.clone().unwrap_or_else(|| {
                    // No config file exists yet — create a default one beside the exe.
                    let default_path = std::env::current_exe()
                        .ok()
                        .and_then(|exe| exe.parent().map(|p| p.join("deskmic.toml")))
                        .unwrap_or_else(|| std::path::PathBuf::from("deskmic.toml"));
                    if !default_path.exists() {
                        let content = crate::config::Config::generate_default_commented();
                        let _ = std::fs::write(&default_path, &content);
                    }
                    default_path
                });
                let _ = std::process::Command::new("notepad").arg(&path).spawn();
            }
        }

        // Periodically poll the transcription status file.
        if last_status_poll.elapsed().as_secs() >= STATUS_POLL_INTERVAL_SECS {
            last_status_poll = Instant::now();
            update_transcription_display(
                &recordings_dir,
                &tray_icon,
                &tx_status_item,
                &tx_queue_item,
                &tx_session_item,
                &tx_cpu_item,
                &paused,
            );
        }

        // Pump Win32 messages so tray-icon receives window messages.
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::*;
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    Ok(())
}

/// Read the transcription status file and update tray tooltip + menu items.
fn update_transcription_display(
    recordings_dir: &std::path::Path,
    tray_icon: &tray_icon::TrayIcon,
    tx_status_item: &MenuItem,
    tx_queue_item: &MenuItem,
    tx_session_item: &MenuItem,
    tx_cpu_item: &MenuItem,
    paused: &Arc<AtomicBool>,
) {
    let recording_state = if paused.load(Ordering::Relaxed) {
        "Paused"
    } else {
        "Recording"
    };

    match TranscriptionStatus::read(recordings_dir) {
        Some(status) => {
            // Update tooltip with combined recording + transcription info
            let tx_summary = status.tooltip_summary();
            let tooltip = format!("deskmic - {} | {}", recording_state, tx_summary);
            // Windows tooltips are limited to 127 chars
            let tooltip = if tooltip.len() > 127 {
                format!("{}...", &tooltip[..124])
            } else {
                tooltip
            };
            let _ = tray_icon.set_tooltip(Some(&tooltip));

            // Update menu items
            tx_status_item.set_text(format!("Transcriber: {}", status.state));

            if status.queue_length > 0 {
                tx_queue_item.set_text(format!("Queue: {} files pending", status.queue_length));
            } else {
                tx_queue_item.set_text("Queue: empty");
            }

            let words = status.session.words;
            let mins = status.session.audio_secs / 60.0;
            tx_session_item.set_text(format!(
                "Session: {} files, {:.1} min, {} words",
                status.session.files_done, mins, words
            ));

            tx_cpu_item.set_text(format!("CPU: {:.0}%", status.last_cpu_percent));
        }
        None => {
            // No status file yet — transcriber may not have started
            let _ = tray_icon.set_tooltip(Some(&format!(
                "deskmic - {} | Transcriber: not running",
                recording_state
            )));
            tx_status_item.set_text("Transcriber: not running");
            tx_queue_item.set_text("Queue: -");
            tx_session_item.set_text("Session: -");
            tx_cpu_item.set_text("CPU: -");
        }
    }
}
