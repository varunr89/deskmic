use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Current state of the transcription process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriberState {
    /// Waiting for system to become idle.
    Idle,
    /// Currently transcribing a file.
    Transcribing,
    /// All pending files have been processed.
    UpToDate,
    /// An error occurred during the last operation.
    Error,
}

impl std::fmt::Display for TranscriberState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Transcribing => write!(f, "Transcribing"),
            Self::UpToDate => write!(f, "Up to date"),
            Self::Error => write!(f, "Error"),
        }
    }
}

/// Cumulative statistics for transcription.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TranscriptionStats {
    pub files_done: u64,
    pub audio_secs: f64,
    pub words: u64,
}

/// Status snapshot written to disk by the transcription process and read by
/// the tray UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionStatus {
    pub state: TranscriberState,
    /// File currently being transcribed (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    /// Number of files waiting to be transcribed.
    pub queue_length: usize,
    /// Stats accumulated since the transcriber process started.
    pub session: TranscriptionStats,
    /// Last observed CPU usage percentage.
    pub last_cpu_percent: f32,
    /// ISO-8601 timestamp of the last status update.
    pub updated_at: String,
    /// Human-readable error message (if state == Error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

const STATUS_FILE_NAME: &str = ".transcription-status.json";

impl TranscriptionStatus {
    /// Create a new status with default (idle) values.
    pub fn new() -> Self {
        Self {
            state: TranscriberState::Idle,
            current_file: None,
            queue_length: 0,
            session: TranscriptionStats::default(),
            last_cpu_percent: 0.0,
            updated_at: chrono::Local::now().to_rfc3339(),
            error_message: None,
        }
    }

    /// Write the status to the status file in the recordings directory.
    pub fn write(&self, recordings_dir: &Path) -> Result<()> {
        let path = recordings_dir.join(STATUS_FILE_NAME);
        let content = serde_json::to_string_pretty(self)?;
        // Write atomically: write to temp then rename, to avoid the reader
        // seeing a half-written file.
        let tmp_path = recordings_dir.join(".transcription-status.json.tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    /// Read the status file from the recordings directory. Returns `None` if
    /// the file doesn't exist or can't be parsed (e.g. partially written).
    pub fn read(recordings_dir: &Path) -> Option<Self> {
        let path = recordings_dir.join(STATUS_FILE_NAME);
        let content = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Update the timestamp to now.
    pub fn touch(&mut self) {
        self.updated_at = chrono::Local::now().to_rfc3339();
    }

    /// Format a one-line summary suitable for a tray tooltip.
    pub fn tooltip_summary(&self) -> String {
        match self.state {
            TranscriberState::Transcribing => {
                let file_hint = self
                    .current_file
                    .as_deref()
                    .and_then(|f| f.rsplit(['/', '\\']).next())
                    .unwrap_or("...");
                format!(
                    "Transcribing: {} | {} queued | {} done",
                    file_hint, self.queue_length, self.session.files_done
                )
            }
            TranscriberState::UpToDate => {
                format!(
                    "Transcription up to date ({} files, {:.0}s audio)",
                    self.session.files_done, self.session.audio_secs
                )
            }
            TranscriberState::Idle => {
                format!(
                    "Transcriber idle (CPU: {:.0}%) | {} done",
                    self.last_cpu_percent, self.session.files_done
                )
            }
            TranscriberState::Error => {
                let msg = self.error_message.as_deref().unwrap_or("unknown error");
                format!("Transcription error: {}", msg)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_status_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let mut status = TranscriptionStatus::new();
        status.state = TranscriberState::Transcribing;
        status.current_file = Some("2026-02-16/mic_14-30-00.wav".to_string());
        status.queue_length = 5;
        status.session.files_done = 3;
        status.session.audio_secs = 120.5;
        status.session.words = 250;
        status.last_cpu_percent = 15.2;

        status.write(tmp.path()).unwrap();
        let loaded = TranscriptionStatus::read(tmp.path()).unwrap();

        assert_eq!(loaded.state, TranscriberState::Transcribing);
        assert_eq!(loaded.current_file.as_deref(), Some("2026-02-16/mic_14-30-00.wav"));
        assert_eq!(loaded.queue_length, 5);
        assert_eq!(loaded.session.files_done, 3);
        assert!((loaded.session.audio_secs - 120.5).abs() < 0.01);
        assert_eq!(loaded.session.words, 250);
    }

    #[test]
    fn test_status_read_nonexistent() {
        let tmp = TempDir::new().unwrap();
        assert!(TranscriptionStatus::read(tmp.path()).is_none());
    }

    #[test]
    fn test_tooltip_transcribing() {
        let mut status = TranscriptionStatus::new();
        status.state = TranscriberState::Transcribing;
        status.current_file = Some("2026-02-16/mic_14-30-00.wav".to_string());
        status.queue_length = 42;
        status.session.files_done = 8;
        let tip = status.tooltip_summary();
        assert!(tip.contains("mic_14-30-00.wav"));
        assert!(tip.contains("42 queued"));
        assert!(tip.contains("8 done"));
    }

    #[test]
    fn test_tooltip_up_to_date() {
        let mut status = TranscriptionStatus::new();
        status.state = TranscriberState::UpToDate;
        status.session.files_done = 10;
        status.session.audio_secs = 300.0;
        let tip = status.tooltip_summary();
        assert!(tip.contains("up to date"));
        assert!(tip.contains("10 files"));
    }

    #[test]
    fn test_tooltip_idle() {
        let mut status = TranscriptionStatus::new();
        status.state = TranscriberState::Idle;
        status.last_cpu_percent = 75.3;
        let tip = status.tooltip_summary();
        assert!(tip.contains("idle"));
        assert!(tip.contains("75%"));
    }

    #[test]
    fn test_tooltip_error() {
        let mut status = TranscriptionStatus::new();
        status.state = TranscriberState::Error;
        status.error_message = Some("model not found".to_string());
        let tip = status.tooltip_summary();
        assert!(tip.contains("error"));
        assert!(tip.contains("model not found"));
    }

    #[test]
    fn test_serde_state_values() {
        // Verify snake_case serialization
        let json = serde_json::to_string(&TranscriberState::UpToDate).unwrap();
        assert_eq!(json, "\"up_to_date\"");
        let json = serde_json::to_string(&TranscriberState::Transcribing).unwrap();
        assert_eq!(json, "\"transcribing\"");
    }
}
