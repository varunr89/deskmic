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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // For CLI subcommands that produce output, re-attach to the parent console.
    let needs_console = !matches!(cli.command, None | Some(Commands::Record));
    #[cfg(target_os = "windows")]
    if needs_console {
        attach_console();
    }

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
    }
}
