use clap::{Parser, Subcommand};
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
}
