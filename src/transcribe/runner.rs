use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::Config;
use crate::transcribe::backend::{Transcript, TranscriptionBackend};
use crate::transcribe::state::TranscriptionState;

/// Find all unprocessed WAV files in the recordings directory.
fn find_pending_files(recordings_dir: &Path, state: &TranscriptionState) -> Result<Vec<PathBuf>> {
    let mut pending = Vec::new();

    if !recordings_dir.exists() {
        return Ok(pending);
    }

    for entry in std::fs::read_dir(recordings_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        for file in std::fs::read_dir(entry.path())? {
            let file = file?;
            let path = file.path();
            if path.extension().map(|e| e == "wav").unwrap_or(false) {
                let relative = path
                    .strip_prefix(recordings_dir)?
                    .to_string_lossy()
                    .to_string();
                if !state.is_transcribed(&relative) {
                    pending.push(path);
                }
            }
        }
    }

    pending.sort(); // deterministic order
    Ok(pending)
}

/// Build the appropriate backend from config.
fn build_backend(
    config: &Config,
    backend_override: Option<&str>,
) -> Result<Box<dyn TranscriptionBackend>> {
    let backend_name = backend_override.unwrap_or(&config.transcription.backend);

    match backend_name {
        "local" => {
            #[cfg(target_os = "windows")]
            {
                use crate::transcribe::whisper_local::WhisperLocal;
                let model_file = format!("ggml-{}.bin", config.transcription.model);
                // TODO: resolve actual model path (exe dir, then %APPDATA%\deskmic\models\)
                Ok(Box::new(WhisperLocal::new(&model_file)?))
            }
            #[cfg(not(target_os = "windows"))]
            {
                anyhow::bail!("Local whisper backend is only available on Windows")
            }
        }
        "azure" => {
            use crate::transcribe::azure_openai::AzureOpenAIBackend;
            Ok(Box::new(AzureOpenAIBackend::new(
                &config.transcription.azure,
            )?))
        }
        other => anyhow::bail!("Unknown transcription backend: {}", other),
    }
}

/// Append a transcript to the daily JSONL file and update state.
fn save_transcript(
    transcript: &Transcript,
    audio_path: &Path,
    recordings_dir: &Path,
    state: &mut TranscriptionState,
) -> Result<()> {
    let transcript_dir = recordings_dir.join("transcripts");
    std::fs::create_dir_all(&transcript_dir)?;

    let date_dir = audio_path
        .parent()
        .and_then(|p| p.file_name())
        .map(|d| d.to_string_lossy().to_string())
        .ok_or_else(|| {
            anyhow::anyhow!("cannot determine date dir from: {}", audio_path.display())
        })?;
    let jsonl_path = transcript_dir.join(format!("{}.jsonl", date_dir));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&jsonl_path)?;
    use std::io::Write;
    writeln!(file, "{}", serde_json::to_string(transcript)?)?;

    // Mark as transcribed
    let relative = audio_path
        .strip_prefix(recordings_dir)?
        .to_string_lossy()
        .to_string();
    state.mark_transcribed(relative);
    state.save(recordings_dir)?;

    Ok(())
}

/// Run one-shot transcription of all pending files.
pub fn run_transcribe_oneshot(config: &Config, backend_override: Option<&str>) -> Result<()> {
    let recordings_dir = &config.output.directory;
    let mut state = TranscriptionState::load(recordings_dir)?;
    let pending = find_pending_files(recordings_dir, &state)?;

    if pending.is_empty() {
        tracing::info!("No pending files to transcribe");
        return Ok(());
    }

    tracing::info!("Found {} pending files", pending.len());
    let backend = build_backend(config, backend_override)?;

    for path in &pending {
        tracing::info!("Transcribing: {}", path.display());
        match backend.transcribe(path) {
            Ok(transcript) => {
                tracing::info!(
                    "Transcribed: {} ({:.1}s)",
                    transcript.file,
                    transcript.duration_secs
                );
                save_transcript(&transcript, path, recordings_dir, &mut state)?;
            }
            Err(e) => {
                tracing::error!("Failed to transcribe {}: {:?}", path.display(), e);
                // Continue with next file
            }
        }
    }

    Ok(())
}

/// Run idle-aware transcription daemon.
pub fn run_transcribe_watch(config: &Config, backend_override: Option<&str>) -> Result<()> {
    let idle_config = &config.transcription.idle_watch;

    loop {
        // Check CPU usage
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_all();
        std::thread::sleep(std::time::Duration::from_secs(1));
        sys.refresh_cpu_all();

        let cpu_usage: f32 =
            sys.cpus().iter().map(|c| c.cpu_usage()).sum::<f32>() / sys.cpus().len() as f32;

        if cpu_usage < idle_config.cpu_threshold_percent {
            tracing::info!("System idle (CPU: {:.1}%), processing...", cpu_usage);
            if let Err(e) = run_transcribe_oneshot(config, backend_override) {
                tracing::error!("Transcription batch failed: {:?}", e);
            }
        } else {
            tracing::debug!("System busy (CPU: {:.1}%), waiting...", cpu_usage);
        }

        std::thread::sleep(std::time::Duration::from_secs(
            idle_config.idle_check_interval_secs,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper to create a minimal valid WAV file.
    fn create_wav_file(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        // Write a short silent clip (160 samples = 10ms at 16kHz)
        for _ in 0..160 {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn test_find_pending_files_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let state = TranscriptionState::default();
        let pending = find_pending_files(tmp.path(), &state).unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn test_find_pending_files_nonexistent_dir() {
        let state = TranscriptionState::default();
        let pending = find_pending_files(Path::new("/nonexistent/path"), &state).unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn test_find_pending_files_discovers_wav() {
        let tmp = TempDir::new().unwrap();
        let date_dir = tmp.path().join("2026-02-16");
        std::fs::create_dir_all(&date_dir).unwrap();
        create_wav_file(&date_dir.join("mic_14-30-00.wav"));
        create_wav_file(&date_dir.join("teams_14-30-00.wav"));

        let state = TranscriptionState::default();
        let pending = find_pending_files(tmp.path(), &state).unwrap();
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn test_find_pending_files_skips_transcribed() {
        let tmp = TempDir::new().unwrap();
        let date_dir = tmp.path().join("2026-02-16");
        std::fs::create_dir_all(&date_dir).unwrap();
        create_wav_file(&date_dir.join("mic_14-30-00.wav"));
        create_wav_file(&date_dir.join("teams_14-30-00.wav"));

        let mut state = TranscriptionState::default();
        state.mark_transcribed("2026-02-16/mic_14-30-00.wav".to_string());

        let pending = find_pending_files(tmp.path(), &state).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].to_string_lossy().contains("teams_14-30-00.wav"));
    }

    #[test]
    fn test_find_pending_files_ignores_non_wav() {
        let tmp = TempDir::new().unwrap();
        let date_dir = tmp.path().join("2026-02-16");
        std::fs::create_dir_all(&date_dir).unwrap();
        std::fs::write(date_dir.join("notes.txt"), "hello").unwrap();
        create_wav_file(&date_dir.join("mic_14-30-00.wav"));

        let state = TranscriptionState::default();
        let pending = find_pending_files(tmp.path(), &state).unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn test_find_pending_files_ignores_root_files() {
        let tmp = TempDir::new().unwrap();
        // WAV at root level (not in a date subdirectory) should be ignored
        create_wav_file(&tmp.path().join("stray.wav"));

        let state = TranscriptionState::default();
        let pending = find_pending_files(tmp.path(), &state).unwrap();
        assert!(pending.is_empty());
    }
}
