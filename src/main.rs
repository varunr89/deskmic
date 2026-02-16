use clap::Parser;
use deskmic::cli::{Cli, Commands};
use deskmic::config::Config;

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
            deskmic::recorder::run_recorder(config, cli.config)
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
