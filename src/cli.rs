use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "deskmic",
    version,
    about = "Always-on Windows audio recorder with VAD and batch transcription"
)]
pub struct Cli {
    /// Path to config file
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start recording (default if no subcommand)
    Record,

    /// Add deskmic to Windows Startup folder
    Install,

    /// Remove deskmic from Windows Startup folder
    Uninstall,

    /// Show recording status, disk usage, file count
    Status,

    /// Transcribe pending audio files
    Transcribe {
        /// Run as idle-aware daemon instead of one-shot
        #[arg(long)]
        watch: bool,

        /// Force a specific backend (local or azure)
        #[arg(long)]
        backend: Option<String>,
    },

    /// Summarize transcripts and email the summary
    Summarize {
        /// Time period to summarize
        #[arg(long, default_value = "daily")]
        period: SummarizePeriod,
    },
}

#[derive(Debug, Clone, ValueEnum)]
pub enum SummarizePeriod {
    /// Summarize yesterday's transcripts
    Daily,
    /// Summarize the last 7 days of transcripts
    Weekly,
}
