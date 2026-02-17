// File writer thread: receives AudioMessages from capture pipelines and writes WAV files.
//
// Each speech segment becomes one WAV file. Files are rotated if they exceed
// `max_file_duration_mins`. Optionally organized into date-based subdirectories.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use anyhow::Result;
use chrono::Local;
use hound::{SampleFormat, WavSpec, WavWriter};

use crate::audio::pipeline::AudioMessage;
use crate::config::OutputConfig;

struct ActiveFile {
    writer: WavWriter<std::io::BufWriter<std::fs::File>>,
    path: PathBuf,
    sample_count: usize,
    max_samples: usize,
}

/// Runs the file writer loop. Call on a dedicated thread.
///
/// Blocks until the channel is closed (all senders dropped), then finalizes
/// any remaining open files before returning.
pub fn run_file_writer(
    receiver: Receiver<AudioMessage>,
    output_config: &OutputConfig,
    sample_rate: u32,
) -> Result<()> {
    let mut active_files: HashMap<String, ActiveFile> = HashMap::new();
    let max_samples = (output_config.max_file_duration_mins as usize) * 60 * sample_rate as usize;

    for msg in receiver {
        match msg {
            AudioMessage::SpeechStart {
                source,
                samples,
                sample_rate: sr,
            } => {
                // Close any existing file for this source.
                if let Some(active) = active_files.remove(&source) {
                    active.writer.finalize()?;
                    tracing::info!("Closed {}", active.path.display());
                }

                // Create new file.
                let path = make_file_path(
                    &output_config.directory,
                    &source,
                    output_config.organize_by_date,
                );
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                let spec = WavSpec {
                    channels: 1,
                    sample_rate: sr,
                    bits_per_sample: 16,
                    sample_format: SampleFormat::Int,
                };
                let mut writer = WavWriter::create(&path, spec)?;
                for &sample in &samples {
                    writer.write_sample(sample)?;
                }
                tracing::info!("Started recording: {}", path.display());

                active_files.insert(
                    source.clone(),
                    ActiveFile {
                        writer,
                        path,
                        sample_count: samples.len(),
                        max_samples,
                    },
                );
            }

            AudioMessage::SpeechContinue { source, samples } => {
                if let Some(active) = active_files.get_mut(&source) {
                    for &sample in &samples {
                        active.writer.write_sample(sample)?;
                    }
                    active.sample_count += samples.len();

                    if active.sample_count >= active.max_samples {
                        let active = active_files.remove(&source).unwrap();
                        active.writer.finalize()?;
                        tracing::info!("Rotated (max duration): {}", active.path.display());
                    }
                }
            }

            AudioMessage::SpeechEnd { source } => {
                if let Some(active) = active_files.remove(&source) {
                    active.writer.finalize()?;
                    tracing::info!("Finished recording: {}", active.path.display());
                }
            }
        }
    }

    // Channel closed -- finalize all open files.
    for (_, active) in active_files {
        active.writer.finalize()?;
        tracing::info!("Finalized on shutdown: {}", active.path.display());
    }

    Ok(())
}

fn make_file_path(base_dir: &Path, source: &str, organize_by_date: bool) -> PathBuf {
    let now = Local::now();
    let filename = format!("{}_{}.wav", source, now.format("%H-%M-%S"));

    if organize_by_date {
        base_dir
            .join(now.format("%Y-%m-%d").to_string())
            .join(filename)
    } else {
        base_dir.join(filename)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn test_make_file_path_with_date() {
        let path = make_file_path(Path::new("/tmp/recordings"), "mic", true);
        let path_str = path.to_str().unwrap();
        assert!(path_str.contains("mic_"));
        assert!(path_str.ends_with(".wav"));
        assert!(path_str.contains(&Local::now().format("%Y-%m-%d").to_string()));
    }

    #[test]
    fn test_make_file_path_without_date() {
        let path = make_file_path(Path::new("/tmp/recordings"), "teams", false);
        let path_str = path.to_str().unwrap();
        assert!(path_str.starts_with("/tmp/recordings/teams_"));
        assert!(path_str.ends_with(".wav"));
        assert!(!path_str.contains(&Local::now().format("%Y-%m-%d").to_string()));
    }

    #[test]
    fn test_file_writer_creates_valid_wav() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let output_config = OutputConfig {
            directory: tmp_dir.path().to_path_buf(),
            max_file_duration_mins: 30,
            organize_by_date: false,
        };

        let (tx, rx) = mpsc::channel();
        let sample_rate = 16000u32;

        // Send a complete speech segment: Start -> Continue -> End.
        let start_samples: Vec<i16> = (0..1600).map(|i| (i % 256) as i16).collect();
        let continue_samples: Vec<i16> = (0..800).map(|i| (i % 128) as i16).collect();

        tx.send(AudioMessage::SpeechStart {
            source: "test-mic".to_string(),
            samples: start_samples.clone(),
            sample_rate,
        })
        .unwrap();

        tx.send(AudioMessage::SpeechContinue {
            source: "test-mic".to_string(),
            samples: continue_samples.clone(),
        })
        .unwrap();

        tx.send(AudioMessage::SpeechEnd {
            source: "test-mic".to_string(),
        })
        .unwrap();

        // Drop sender so the receiver loop exits.
        drop(tx);

        let result = run_file_writer(rx, &output_config, sample_rate);
        assert!(result.is_ok(), "file writer failed: {:?}", result);

        // Find the WAV file in the temp directory.
        let entries: Vec<_> = std::fs::read_dir(tmp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1, "Expected exactly one WAV file");

        let wav_path = entries[0].path();
        assert!(wav_path.to_str().unwrap().ends_with(".wav"));

        // Read it back and verify.
        let reader = hound::WavReader::open(&wav_path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, sample_rate);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, SampleFormat::Int);

        let samples: Vec<i16> = reader.into_samples::<i16>().map(|s| s.unwrap()).collect();
        let total_expected = start_samples.len() + continue_samples.len();
        assert_eq!(
            samples.len(),
            total_expected,
            "WAV should contain {} samples, got {}",
            total_expected,
            samples.len()
        );

        // Verify the actual sample data matches.
        assert_eq!(&samples[..start_samples.len()], &start_samples[..]);
        assert_eq!(&samples[start_samples.len()..], &continue_samples[..]);
    }

    #[test]
    fn test_file_writer_organizes_by_date() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let output_config = OutputConfig {
            directory: tmp_dir.path().to_path_buf(),
            max_file_duration_mins: 30,
            organize_by_date: true,
        };

        let (tx, rx) = mpsc::channel();
        let sample_rate = 16000u32;

        tx.send(AudioMessage::SpeechStart {
            source: "mic".to_string(),
            samples: vec![100i16; 160],
            sample_rate,
        })
        .unwrap();

        tx.send(AudioMessage::SpeechEnd {
            source: "mic".to_string(),
        })
        .unwrap();

        drop(tx);

        run_file_writer(rx, &output_config, sample_rate).unwrap();

        // Should have created a date subdirectory.
        let date_dir = tmp_dir
            .path()
            .join(Local::now().format("%Y-%m-%d").to_string());
        assert!(date_dir.exists(), "Date directory should exist");

        let entries: Vec<_> = std::fs::read_dir(&date_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].path().to_str().unwrap().contains("mic_"));
    }
}
