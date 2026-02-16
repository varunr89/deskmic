use std::sync::mpsc;
use tempfile::TempDir;

#[test]
fn test_file_writer_creates_wav_on_speech() {
    use deskmic::audio::file_writer::run_file_writer;
    use deskmic::audio::pipeline::AudioMessage;
    use deskmic::config::OutputConfig;

    let tmp = TempDir::new().unwrap();
    // Capture date before spawning writer to avoid midnight race condition
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let output_config = OutputConfig {
        directory: tmp.path().to_path_buf(),
        max_file_duration_mins: 60,
        organize_by_date: true,
    };

    let (sender, receiver) = mpsc::channel();

    let config_clone = output_config.clone();
    let writer = std::thread::spawn(move || {
        run_file_writer(receiver, &config_clone, 16000).unwrap();
    });

    // Simulate speech
    let samples: Vec<i16> = (0..16000).map(|i| (i % 100) as i16).collect();
    sender
        .send(AudioMessage::SpeechStart {
            source: "mic".to_string(),
            samples: samples.clone(),
            sample_rate: 16000,
        })
        .unwrap();

    sender
        .send(AudioMessage::SpeechContinue {
            source: "mic".to_string(),
            samples: samples.clone(),
        })
        .unwrap();

    sender
        .send(AudioMessage::SpeechEnd {
            source: "mic".to_string(),
        })
        .unwrap();

    // Close channel to stop writer
    drop(sender);
    writer.join().unwrap();

    // Verify a WAV file was created
    let date_dir = tmp.path().join(&today);
    assert!(date_dir.exists(), "Date directory should exist");

    let files: Vec<_> = std::fs::read_dir(&date_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "wav")
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(files.len(), 1, "Should have exactly one WAV file");

    // Verify the WAV is valid
    let wav_path = files[0].path();
    let reader = hound::WavReader::open(&wav_path).unwrap();
    assert_eq!(reader.spec().sample_rate, 16000);
    assert_eq!(reader.spec().channels, 1);
    assert_eq!(reader.spec().bits_per_sample, 16);

    // Verify all samples were written (16000 in SpeechStart + 16000 in SpeechContinue)
    let sample_count = reader.into_samples::<i16>().count();
    assert_eq!(sample_count, 32000, "WAV should contain all sent samples");
}
