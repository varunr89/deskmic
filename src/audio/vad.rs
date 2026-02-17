// VAD (Voice Activity Detection) wrapper
//
// The real implementation uses `voice_activity_detector` which is only
// available on Windows. On non-Windows platforms we provide a stub.

/// Trait for voice activity detection, allowing platform-specific implementations.
pub trait VadProcessor {
    /// Returns true if the given audio chunk contains speech.
    fn is_speech(&mut self, samples: &[i16]) -> bool;
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use anyhow::Result;
    use voice_activity_detector::VoiceActivityDetector;

    pub struct Vad {
        detector: VoiceActivityDetector,
        threshold: f32,
    }

    impl Vad {
        pub fn new(sample_rate: u32, threshold: f32) -> Result<Self> {
            let chunk_size = match sample_rate {
                8000 => 256usize,
                16000 => 512usize,
                _ => anyhow::bail!("VAD only supports 8000 or 16000 Hz sample rate"),
            };
            let detector = VoiceActivityDetector::builder()
                .sample_rate(sample_rate)
                .chunk_size(chunk_size)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build VAD: {:?}", e))?;
            Ok(Self {
                detector,
                threshold,
            })
        }
    }

    impl VadProcessor for Vad {
        fn is_speech(&mut self, samples: &[i16]) -> bool {
            let probability = self.detector.predict(samples.to_vec());
            probability >= self.threshold
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_silence_is_not_speech() {
            let mut vad = Vad::new(16000, 0.5).unwrap();
            let silence = vec![0i16; 512];
            assert!(!vad.is_speech(&silence));
        }

        #[test]
        fn test_vad_initializes() {
            let vad = Vad::new(16000, 0.5);
            assert!(vad.is_ok());
        }

        #[test]
        fn test_invalid_sample_rate() {
            let vad = Vad::new(44100, 0.5);
            assert!(vad.is_err());
        }
    }
}

#[cfg(target_os = "windows")]
pub use platform::Vad;
