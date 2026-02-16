mod audio;
mod cli;
mod commands;
mod config;
mod recorder;
mod storage;
mod transcribe;
#[cfg(target_os = "windows")]
mod tray;

use clap::Parser;
use cli::{Cli, Commands};
use config::Config;

fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("deskmic=info".parse()?),
        )
        .init();

    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;

    match cli.command.unwrap_or(Commands::Record) {
        Commands::Record => {
            tracing::info!("Starting deskmic recorder");
            crate::recorder::run_recorder(config, cli.config)
        }
        Commands::Install => crate::commands::install_startup(),
        Commands::Uninstall => crate::commands::uninstall_startup(),
        Commands::Status => crate::commands::show_status(&config.output.directory),
        Commands::Transcribe { watch, backend } => {
            if watch {
                crate::transcribe::runner::run_transcribe_watch(&config, backend.as_deref())
            } else {
                crate::transcribe::runner::run_transcribe_oneshot(&config, backend.as_deref())
            }
        }
    }
}
