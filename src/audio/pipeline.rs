// Capture pipeline: ties audio capture -> ring buffer -> VAD -> file writer channel.
//
// The `AudioMessage` enum is cross-platform (used by the file writer in later tasks).
// The `run_capture_pipeline` function is also cross-platform — it accepts a
// `&mut dyn VadProcessor` trait object, so the caller provides the platform-specific
// VAD implementation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use anyhow::Result;

use crate::audio::ring_buffer::RingBuffer;
use crate::audio::vad::VadProcessor;

/// Messages sent from the capture pipeline to the file writer.
#[derive(Debug)]
pub enum AudioMessage {
    /// Speech has begun. Contains the pre-speech buffer plus the first speech chunk.
    SpeechStart {
        source: String,
        samples: Vec<i16>,
        sample_rate: u32,
    },
    /// Ongoing speech data.
    SpeechContinue { source: String, samples: Vec<i16> },
    /// Speech has ended (silence threshold exceeded).
    SpeechEnd { source: String },
}

/// Runs the capture -> VAD -> file-writer pipeline on the calling thread.
///
/// This function is generic over the audio source and VAD implementation:
/// - `capture_fn`: called repeatedly to obtain the next chunk of i16 samples.
///   Returns `Ok(None)` if the device was invalidated (triggers graceful shutdown).
/// - `start_fn`: called once before the capture loop begins (e.g. to start WASAPI stream).
/// - `vad`: any implementation of `VadProcessor`.
/// - `sender`: channel for `AudioMessage`s consumed by the file writer.
/// - `shutdown`: atomic flag; when set to `true`, the loop exits.
/// - `paused`: atomic flag; when `true`, audio is still drained from the capture
///   device (to prevent WASAPI buffer overflow) but VAD processing is skipped,
///   buffers are cleared, and any in-progress speech segment is closed out.
///
/// The pipeline buffers non-speech audio in a ring buffer so that the first
/// `pre_speech_buffer_secs` of audio before speech onset is included in the
/// `SpeechStart` message.
pub fn run_capture_pipeline(
    source_name: String,
    capture_fn: impl Fn() -> Result<Option<Vec<i16>>>,
    start_fn: impl Fn() -> Result<()>,
    sample_rate: u32,
    pre_speech_buffer_secs: f32,
    silence_threshold_secs: f32,
    vad: &mut dyn VadProcessor,
    chunk_size: usize,
    sender: Sender<AudioMessage>,
    shutdown: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) -> Result<()> {
    let mut ring_buffer = RingBuffer::new(sample_rate, pre_speech_buffer_secs);
    let silence_samples = (sample_rate as f32 * silence_threshold_secs) as usize;

    let mut is_speaking = false;
    let mut silence_count: usize = 0;
    let mut pending_samples: Vec<i16> = Vec::new();

    start_fn()?;

    while !shutdown.load(Ordering::Relaxed) {
        let samples = match capture_fn()? {
            Some(s) => s,
            None => {
                tracing::warn!(
                    "{}: capture returned None, device may be invalidated",
                    source_name
                );
                break;
            }
        };

        // When paused, drain audio (already read above) but skip all processing.
        // Close out any in-progress speech segment so the WAV file is finalized.
        if paused.load(Ordering::Relaxed) {
            if is_speaking {
                let _ = sender.send(AudioMessage::SpeechEnd {
                    source: source_name.clone(),
                });
                is_speaking = false;
                silence_count = 0;
            }
            pending_samples.clear();
            ring_buffer.clear();
            continue;
        }

        pending_samples.extend_from_slice(&samples);

        // Process complete chunks through VAD.
        while pending_samples.len() >= chunk_size {
            let chunk: Vec<i16> = pending_samples.drain(..chunk_size).collect();
            let speech = vad.is_speech(&chunk);

            if speech {
                silence_count = 0;

                if !is_speaking {
                    // Transition: silence -> speech.
                    is_speaking = true;
                    let mut initial = ring_buffer.drain();
                    initial.extend_from_slice(&chunk);
                    sender.send(AudioMessage::SpeechStart {
                        source: source_name.clone(),
                        samples: initial,
                        sample_rate,
                    })?;
                } else {
                    // Continuing speech.
                    sender.send(AudioMessage::SpeechContinue {
                        source: source_name.clone(),
                        samples: chunk,
                    })?;
                }
            } else if is_speaking {
                // Silence during speech — count toward threshold but still send data
                // so the WAV file includes the trailing silence.
                silence_count += chunk_size;
                sender.send(AudioMessage::SpeechContinue {
                    source: source_name.clone(),
                    samples: chunk,
                })?;

                if silence_count >= silence_samples {
                    // Enough silence to end the speech segment.
                    is_speaking = false;
                    silence_count = 0;
                    sender.send(AudioMessage::SpeechEnd {
                        source: source_name.clone(),
                    })?;
                }
            } else {
                // Not speaking — push to ring buffer for pre-speech context.
                ring_buffer.push(&chunk);
            }
        }
    }

    // If we exit the loop while still in a speech segment, close it out.
    if is_speaking {
        let _ = sender.send(AudioMessage::SpeechEnd {
            source: source_name,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::sync::Arc;

    /// A test VAD that considers any chunk where the first sample is non-zero as speech.
    struct TestVad;

    impl VadProcessor for TestVad {
        fn is_speech(&mut self, samples: &[i16]) -> bool {
            samples.first().map_or(false, |&s| s != 0)
        }
    }

    #[test]
    fn test_pipeline_detects_speech_and_silence() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));

        // We'll feed: 2 chunks of silence, 2 chunks of speech, then enough silence to trigger end.
        let chunk_size = 4;
        let silence_chunk = vec![0i16; chunk_size];
        let speech_chunk = vec![100i16; chunk_size];

        // silence_threshold_secs = 0.5 at sample_rate=8 means 4 samples of silence to trigger end.
        // That's exactly 1 silent chunk after speech.
        let sample_rate = 8;
        let pre_speech_buffer_secs = 0.5; // ring buffer holds 4 samples
        let silence_threshold_secs = 0.5; // 4 samples of silence triggers end

        let mut chunks: Vec<Option<Vec<i16>>> = vec![
            Some(silence_chunk.clone()), // silence -> ring buffer
            Some(silence_chunk.clone()), // silence -> ring buffer (overwrites some)
            Some(speech_chunk.clone()),  // speech start
            Some(speech_chunk.clone()),  // speech continue
            Some(silence_chunk.clone()), // silence during speech -> triggers end
            None,                        // signals capture ended
        ];
        chunks.reverse(); // so we can pop from the end

        let chunks = std::cell::RefCell::new(chunks);

        let capture_fn = || -> Result<Option<Vec<i16>>> {
            let mut c = chunks.borrow_mut();
            match c.pop() {
                Some(val) => Ok(val),
                None => Ok(None),
            }
        };

        let start_fn = || -> Result<()> { Ok(()) };
        let mut vad = TestVad;

        let result = run_capture_pipeline(
            "test-mic".to_string(),
            capture_fn,
            start_fn,
            sample_rate,
            pre_speech_buffer_secs,
            silence_threshold_secs,
            &mut vad,
            chunk_size,
            tx,
            shutdown,
            Arc::new(AtomicBool::new(false)),
        );

        assert!(result.is_ok());

        // Collect all messages.
        let messages: Vec<AudioMessage> = rx.try_iter().collect();

        // We expect: SpeechStart, SpeechContinue, SpeechContinue (silence during speech), SpeechEnd.
        assert!(
            messages.len() >= 3,
            "Expected at least 3 messages, got {}",
            messages.len()
        );

        // First message should be SpeechStart.
        match &messages[0] {
            AudioMessage::SpeechStart {
                source,
                samples,
                sample_rate: sr,
            } => {
                assert_eq!(source, "test-mic");
                assert_eq!(*sr, 8);
                // Should include ring buffer (pre-speech) + first speech chunk.
                assert!(samples.len() >= chunk_size);
            }
            other => panic!("Expected SpeechStart, got {:?}", other),
        }

        // Last message should be SpeechEnd.
        match messages.last().unwrap() {
            AudioMessage::SpeechEnd { source } => {
                assert_eq!(source, "test-mic");
            }
            other => panic!("Expected SpeechEnd, got {:?}", other),
        }
    }

    #[test]
    fn test_pipeline_shutdown_flag() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(true)); // already shut down

        let capture_fn = || -> Result<Option<Vec<i16>>> {
            // Should never be called because shutdown is already set.
            panic!("capture_fn should not be called when shutdown is true");
        };

        let start_fn = || -> Result<()> { Ok(()) };
        let mut vad = TestVad;

        let result = run_capture_pipeline(
            "test-mic".to_string(),
            capture_fn,
            start_fn,
            16000,
            5.0,
            3.0,
            &mut vad,
            512,
            tx,
            shutdown,
            Arc::new(AtomicBool::new(false)),
        );

        assert!(result.is_ok());
        // No messages should have been sent.
        let messages: Vec<AudioMessage> = rx.try_iter().collect();
        assert!(messages.is_empty());
    }

    #[test]
    fn test_pipeline_paused_skips_processing() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(true)); // start paused

        let chunk_size = 4;
        let speech_chunk = vec![100i16; chunk_size];

        // Feed several speech chunks (which would trigger SpeechStart if not paused),
        // then None to end the loop.
        let mut chunks: Vec<Option<Vec<i16>>> = vec![
            Some(speech_chunk.clone()),
            Some(speech_chunk.clone()),
            Some(speech_chunk.clone()),
            None,
        ];
        chunks.reverse();

        let chunks = std::cell::RefCell::new(chunks);

        let capture_fn = || -> Result<Option<Vec<i16>>> {
            let mut c = chunks.borrow_mut();
            match c.pop() {
                Some(val) => Ok(val),
                None => Ok(None),
            }
        };

        let start_fn = || -> Result<()> { Ok(()) };
        let mut vad = TestVad;

        let result = run_capture_pipeline(
            "test-mic".to_string(),
            capture_fn,
            start_fn,
            8,
            0.5,
            0.5,
            &mut vad,
            chunk_size,
            tx,
            shutdown,
            paused,
        );

        assert!(result.is_ok());

        // No messages should have been sent because we were paused the whole time.
        let messages: Vec<AudioMessage> = rx.try_iter().collect();
        assert!(
            messages.is_empty(),
            "Expected no messages when paused, got {}",
            messages.len()
        );
    }
}
