mod cli;
mod config;

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
    let _config = Config::load(cli.config.as_deref())?;

    match cli.command.unwrap_or(Commands::Record) {
        Commands::Record => {
            tracing::info!("Starting recording...");
            // TODO: Task 3+
            Ok(())
        }
        Commands::Install => {
            tracing::info!("Installing to startup...");
            // TODO: Task 9
            Ok(())
        }
        Commands::Uninstall => {
            tracing::info!("Removing from startup...");
            // TODO: Task 9
            Ok(())
        }
        Commands::Status => {
            tracing::info!("Checking status...");
            // TODO: Task 10
            Ok(())
        }
        Commands::Transcribe { watch, backend } => {
            tracing::info!(
                watch = watch,
                backend = ?backend,
                "Transcribing..."
            );
            // TODO: Task 7+
            Ok(())
        }
    }
}
