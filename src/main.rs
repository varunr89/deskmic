// Hide the console window on Windows. CLI subcommands that need output
// (status, transcribe, install, uninstall) re-attach or allocate a console.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use clap::Parser;
use deskmic::cli::{Cli, Commands};
use deskmic::config::Config;

/// Re-attach to the parent console (if launched from a terminal) so that
/// CLI subcommands can print output. No-op on non-Windows.
#[cfg(target_os = "windows")]
fn attach_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Acquire a named mutex to prevent multiple instances of a given component.
/// Returns the mutex handle on success, or None if another instance already holds it.
/// The handle must be kept alive for the lifetime of the process.
#[cfg(target_os = "windows")]
fn try_acquire_instance_mutex(
    name: &str,
) -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_ALREADY_EXISTS;
    use windows::Win32::System::Threading::CreateMutexW;

    let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let result = unsafe { CreateMutexW(None, false, PCWSTR(wide_name.as_ptr())) };

    match result {
        Ok(handle) => {
            // CreateMutexW succeeded — check if we're the first owner or a duplicate.
            let last_error = unsafe { windows::Win32::Foundation::GetLastError() };
            if last_error == ERROR_ALREADY_EXISTS {
                // Another instance already holds this mutex.
                None
            } else {
                Some(handle)
            }
        }
        Err(_) => None,
    }
}

/// Show a Windows message box (used for single-instance notification).
#[cfg(target_os = "windows")]
fn show_already_running_message() {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

    let text: Vec<u16> = "deskmic is already running.\nCheck the system tray for the icon."
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let caption: Vec<u16> = "deskmic"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // For CLI subcommands that produce output, re-attach to the parent console.
    let needs_console = !matches!(cli.command, None | Some(Commands::Record));
    #[cfg(target_os = "windows")]
    if needs_console {
        attach_console();
    }

    // Single-instance check: the Record command acquires "Global\deskmic",
    // the Transcribe --watch command acquires "Global\deskmic-transcriber".
    // Other subcommands (status, install, etc.) don't need a mutex.
    #[cfg(target_os = "windows")]
    let _instance_mutex = {
        let mutex_name = match &cli.command {
            None | Some(Commands::Record) => Some("Global\\deskmic"),
            Some(Commands::Transcribe { watch: true, .. }) => {
                Some("Global\\deskmic-transcriber")
            }
            _ => None,
        };

        if let Some(name) = mutex_name {
            match try_acquire_instance_mutex(name) {
                Some(handle) => Some(handle),
                None => {
                    // Another instance is already running.
                    if matches!(cli.command, None | Some(Commands::Record)) {
                        show_already_running_message();
                    }
                    // For transcribe --watch launched as child process, just exit silently.
                    std::process::exit(0);
                }
            }
        } else {
            None
        }
    };

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("deskmic=info".parse()?),
        )
        .init();

    let (config, resolved_config_path) = Config::load_with_path(cli.config.as_deref())?;

    match cli.command.unwrap_or(Commands::Record) {
        Commands::Record => {
            tracing::info!("Starting deskmic recorder");
            deskmic::recorder::run_recorder(config, resolved_config_path.or(cli.config))
        }
        Commands::Install => deskmic::commands::install_startup(),
        Commands::Uninstall => deskmic::commands::uninstall_startup(),
        Commands::Status => deskmic::commands::show_status(&config.output.directory),
        Commands::Transcribe { watch, backend } => {
            if watch {
                deskmic::transcribe::runner::run_transcribe_watch(&config, backend.as_deref())
            } else {
                deskmic::transcribe::runner::run_transcribe_oneshot(&config, backend.as_deref())
            }
        }
        Commands::Summarize { period } => {
            deskmic::summarize::runner::run_summarize(&config, &period)
        }
    }
}
