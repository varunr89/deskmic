use std::path::Path;

use anyhow::Result;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::transcribe::backend::{Transcript, TranscriptionBackend};

pub struct WhisperLocal {
    ctx: WhisperContext,
}

impl WhisperLocal {
    pub fn new(model_path: &str) -> Result<Self> {
        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .map_err(|e| anyhow::anyhow!("Failed to load Whisper model: {:?}", e))?;
        Ok(Self { ctx })
    }
}

impl TranscriptionBackend for WhisperLocal {
    fn name(&self) -> &str {
        "whisper-local"
    }

    fn transcribe(&self, audio_path: &Path) -> Result<Vec<Transcript>> {
        let mut reader = hound::WavReader::open(audio_path)?;
        let spec = reader.spec();
        let samples_i16: Vec<i16> = reader
            .samples::<i16>()
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let samples_f32: Vec<f32> = samples_i16.iter().map(|&s| s as f32 / 32768.0).collect();
        let total_duration = samples_f32.len() as f64 / spec.sample_rate as f64;

        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow::anyhow!("Failed to create state: {:?}", e))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(4);
        params.set_language(Some("en"));

        state
            .full(params, &samples_f32)
            .map_err(|e| anyhow::anyhow!("Transcription failed: {:?}", e))?;

        let filename = audio_path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("audio path has no filename: {}", audio_path.display()))?
            .to_string_lossy()
            .to_string();
        let source = if filename.starts_with("mic") {
            "mic"
        } else {
            "teams"
        };
        let speaker = if source == "mic" {
            Some("You".to_string())
        } else {
            None
        };
        let timestamp = audio_path
            .parent()
            .and_then(|p| p.file_name())
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();

        let n_segments = state.full_n_segments();

        // If only one segment or short audio, emit a single transcript
        if n_segments <= 1 {
            let mut text = String::new();
            for i in 0..n_segments {
                if let Some(segment) = state.get_segment(i) {
                    if let Ok(segment_text) = segment.to_str_lossy() {
                        text.push_str(&segment_text);
                    }
                }
            }
            return Ok(vec![Transcript {
                timestamp,
                source: source.to_string(),
                duration_secs: total_duration,
                file: filename,
                text: text.trim().to_string(),
                speaker,
                start_secs: Some(0.0),
                end_secs: Some(total_duration),
            }]);
        }

        // Multiple segments: emit one transcript per segment with timestamps
        let mut transcripts = Vec::new();
        for i in 0..n_segments {
            if let Some(segment) = state.get_segment(i) {
                let text = match segment.to_str_lossy() {
                    Ok(t) => t.trim().to_string(),
                    Err(_) => continue,
                };
                if text.is_empty() {
                    continue;
                }
                // Whisper timestamps are in centiseconds (10ms units)
                let start = segment.start_timestamp() as f64 / 100.0;
                let end = segment.end_timestamp() as f64 / 100.0;
                let duration = end - start;

                transcripts.push(Transcript {
                    timestamp: timestamp.clone(),
                    source: source.to_string(),
                    duration_secs: duration,
                    file: filename.clone(),
                    text,
                    speaker: speaker.clone(),
                    start_secs: Some(start),
                    end_secs: Some(end),
                });
            }
        }

        // Fall back to single transcript if no segments produced text
        if transcripts.is_empty() {
            transcripts.push(Transcript {
                timestamp,
                source: source.to_string(),
                duration_secs: total_duration,
                file: filename,
                text: String::new(),
                speaker,
                start_secs: Some(0.0),
                end_secs: Some(total_duration),
            });
        }

        Ok(transcripts)
    }
}
