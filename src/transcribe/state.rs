use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TranscriptionState {
    pub transcribed_files: HashSet<String>,
}

impl TranscriptionState {
    pub fn load(recordings_dir: &Path) -> Result<Self> {
        let path = recordings_dir.join(".deskmic-state.json");
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, recordings_dir: &Path) -> Result<()> {
        let path = recordings_dir.join(".deskmic-state.json");
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn is_transcribed(&self, file_path: &str) -> bool {
        self.transcribed_files.contains(file_path)
    }

    pub fn mark_transcribed(&mut self, file_path: String) {
        self.transcribed_files.insert(file_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_state_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let mut state = TranscriptionState::default();
        state.mark_transcribed("2026-02-16/mic_14-30-00.wav".to_string());
        state.save(tmp.path()).unwrap();

        let loaded = TranscriptionState::load(tmp.path()).unwrap();
        assert!(loaded.is_transcribed("2026-02-16/mic_14-30-00.wav"));
        assert!(!loaded.is_transcribed("2026-02-16/teams_14-30-00.wav"));
    }

    #[test]
    fn test_empty_state_from_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let state = TranscriptionState::load(tmp.path()).unwrap();
        assert!(state.transcribed_files.is_empty());
    }
}
