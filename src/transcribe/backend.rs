use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub timestamp: String,
    pub source: String,
    pub duration_secs: f64,
    pub file: String,
    pub text: String,
}

pub trait TranscriptionBackend: Send {
    fn name(&self) -> &str;
    fn transcribe(&self, audio_path: &Path) -> Result<Transcript>;
}
