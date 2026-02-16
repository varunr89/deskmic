// System tray UI for deskmic (Windows only).
//
// Provides pause/resume, open recordings folder, open settings, and quit actions.
// Requires a Win32 message pump to process tray icon events.

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;

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
    let quit_item = MenuItem::new("Quit", true, None);

    menu.append(&status_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&pause_item)?;
    menu.append(&resume_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&open_folder_item)?;
    menu.append(&settings_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;

    // Create a simple 16×16 red icon (RGBA).
    let icon = Icon::from_rgba(vec![255, 0, 0, 255].repeat(16 * 16), 16, 16)?;

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("deskmic - Recording")
        .with_icon(icon)
        .build()?;

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
                if let Some(ref path) = config_path {
                    let _ = std::process::Command::new("notepad").arg(path).spawn();
                }
            }
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
