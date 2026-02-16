use std::path::Path;

use anyhow::Result;
use reqwest::blocking::multipart;

use crate::config::AzureConfig;
use crate::transcribe::backend::{Transcript, TranscriptionBackend};

pub struct AzureOpenAIBackend {
    endpoint: String,
    api_key: String,
    deployment: String,
}

impl AzureOpenAIBackend {
    pub fn new(config: &AzureConfig) -> Result<Self> {
        let api_key = if config.api_key.is_empty() {
            std::env::var("DESKMIC_AZURE_KEY")
                .map_err(|_| anyhow::anyhow!("Azure API key not configured"))?
        } else {
            config.api_key.clone()
        };

        Ok(Self {
            endpoint: config.endpoint.clone(),
            api_key,
            deployment: config.deployment.clone(),
        })
    }
}

impl TranscriptionBackend for AzureOpenAIBackend {
    fn name(&self) -> &str {
        "azure-openai"
    }

    fn transcribe(&self, audio_path: &Path) -> Result<Transcript> {
        let url = format!(
            "{}/openai/deployments/{}/audio/transcriptions?api-version=2024-06-01",
            self.endpoint, self.deployment
        );

        let file_bytes = std::fs::read(audio_path)?;
        let filename = audio_path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("audio path has no filename: {}", audio_path.display()))?
            .to_string_lossy()
            .to_string();

        let form = multipart::Form::new()
            .part(
                "file",
                multipart::Part::bytes(file_bytes)
                    .file_name(filename.clone())
                    .mime_str("audio/wav")?,
            )
            .text("response_format", "json");

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()?;
        let response = client
            .post(&url)
            .header("api-key", &self.api_key)
            .multipart(form)
            .send()?;

        let response = response.error_for_status()?;
        let body: serde_json::Value = response.json()?;
        let text = body["text"].as_str().unwrap_or("").to_string();

        // Get duration from WAV header
        let reader = hound::WavReader::open(audio_path)?;
        let spec = reader.spec();
        let duration_secs = reader.duration() as f64 / spec.sample_rate as f64;

        let source = if filename.starts_with("mic") {
            "mic"
        } else {
            "teams"
        };
        let timestamp = audio_path
            .parent()
            .and_then(|p| p.file_name())
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();

        Ok(Transcript {
            timestamp,
            source: source.to_string(),
            duration_secs,
            file: filename,
            text,
        })
    }
}
