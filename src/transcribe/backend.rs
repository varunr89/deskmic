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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_secs: Option<f64>,
}

pub trait TranscriptionBackend: Send {
    fn name(&self) -> &str;
    fn transcribe(&self, audio_path: &Path) -> Result<Vec<Transcript>>;
}
