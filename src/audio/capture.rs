// WASAPI microphone capture module.
//
// This entire module is Windows-only since it depends on the `wasapi` crate.

#![cfg(target_os = "windows")]

use std::collections::VecDeque;

use anyhow::Result;
use thiserror::Error;
use wasapi::*;

/// Errors that can occur during audio capture.
///
/// `DeviceInvalidated` signals that the audio device was lost (e.g. sleep/wake,
/// USB unplug, default device change) and the pipeline should re-initialise.
#[derive(Error, Debug)]
pub enum CaptureError {
    #[error("Device invalidated (sleep/wake or device change)")]
    DeviceInvalidated,
    #[error("Capture error: {0}")]
    Other(#[from] anyhow::Error),
}

/// Captures audio from the default microphone via WASAPI in shared event-driven mode.
///
/// The captured format is 16-bit mono PCM at the requested sample rate.
/// WASAPI's autoconvert feature handles any necessary resampling from the
/// device's native format.
pub struct MicCapture {
    audio_client: AudioClient,
    capture_client: AudioCaptureClient,
    event_handle: Handle,
    sample_rate: u32,
    blockalign: u32,
}

impl MicCapture {
    /// Create a new `MicCapture` that will capture from the default recording device.
    ///
    /// `desired_sample_rate` should be 16000 (for VAD compatibility) or 8000.
    pub fn new(desired_sample_rate: u32) -> Result<Self> {
        initialize_mta().map_err(|e| anyhow::anyhow!("COM MTA initialization failed: {:?}", e))?;

        let enumerator = DeviceEnumerator::new()
            .map_err(|e| anyhow::anyhow!("Failed to create device enumerator: {:?}", e))?;
        let device = enumerator
            .get_default_device(&Direction::Capture)
            .map_err(|e| anyhow::anyhow!("Failed to get default capture device: {:?}", e))?;

        let mut audio_client = device
            .get_iaudioclient()
            .map_err(|e| anyhow::anyhow!("Failed to get IAudioClient: {:?}", e))?;

        // Request 16-bit mono PCM at the desired sample rate.
        let desired_format = WaveFormat::new(
            16,                           // bits per sample
            16,                           // valid bits per sample
            &SampleType::Int,             // integer samples
            desired_sample_rate as usize, // sample rate
            1,                            // mono
            None,                         // no specific channel mask
        );
        let blockalign = desired_format.get_blockalign();

        // Use event-driven shared mode with autoconvert so WASAPI handles
        // resampling from the device's native format to our desired format.
        let (_, min_time) = audio_client
            .get_device_period()
            .map_err(|e| anyhow::anyhow!("Failed to get device period: {:?}", e))?;

        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: min_time,
        };
        audio_client
            .initialize_client(&desired_format, &Direction::Capture, &mode)
            .map_err(|e| anyhow::anyhow!("Failed to initialize audio client: {:?}", e))?;

        let event_handle = audio_client
            .set_get_eventhandle()
            .map_err(|e| anyhow::anyhow!("Failed to set/get event handle: {:?}", e))?;

        let capture_client = audio_client
            .get_audiocaptureclient()
            .map_err(|e| anyhow::anyhow!("Failed to get AudioCaptureClient: {:?}", e))?;

        Ok(Self {
            audio_client,
            capture_client,
            event_handle,
            sample_rate: desired_sample_rate,
            blockalign,
        })
    }

    /// Start the capture stream. Must be called before `read_frames`.
    pub fn start(&self) -> Result<()> {
        self.audio_client
            .start_stream()
            .map_err(|e| anyhow::anyhow!("Failed to start capture stream: {:?}", e))?;
        Ok(())
    }

    /// Wait for the next event and read captured frames as 16-bit PCM samples.
    ///
    /// Returns `Ok(Some(samples))` when audio data is available, or `Ok(None)`
    /// if no data was captured in this cycle (e.g. silence flags set).
    /// Returns `Err(CaptureError::DeviceInvalidated)` when the device is lost
    /// (sleep/wake, USB unplug, default device change), or `Err(CaptureError::Other)`
    /// for other failures.
    pub fn read_frames(&self) -> std::result::Result<Option<Vec<i16>>, CaptureError> {
        // Wait for WASAPI to signal that a buffer is ready.
        if let Err(e) = self.event_handle.wait_for_event(1000) {
            let msg = format!("{:?}", e);
            if Self::is_device_invalidated_error(&msg) {
                return Err(CaptureError::DeviceInvalidated);
            }
            return Err(CaptureError::Other(anyhow::anyhow!(
                "Event wait timeout/error: {}",
                msg
            )));
        }

        // Read captured bytes into a VecDeque, matching the pattern from the
        // wasapi crate's record example.
        let mut sample_queue: VecDeque<u8> = VecDeque::new();
        if let Err(e) = self
            .capture_client
            .read_from_device_to_deque(&mut sample_queue)
        {
            let msg = format!("{:?}", e);
            if Self::is_device_invalidated_error(&msg) {
                return Err(CaptureError::DeviceInvalidated);
            }
            return Err(CaptureError::Other(anyhow::anyhow!(
                "Failed to read from capture device: {}",
                msg
            )));
        }

        if sample_queue.is_empty() {
            return Ok(None);
        }

        // Convert the raw byte deque into contiguous bytes, then to i16 samples.
        // Our format is 16-bit (2 bytes per sample), mono.
        let bytes: Vec<u8> = sample_queue.into_iter().collect();
        let samples: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        Ok(Some(samples))
    }

    /// Check whether a WASAPI error message indicates the audio device was
    /// invalidated (removed, disconnected, or the default changed).
    ///
    /// Common HRESULT codes:
    /// - `AUDCLNT_E_DEVICE_INVALIDATED` (0x88890004)
    /// - `AUDCLNT_E_SERVICE_NOT_RUNNING` (0x88890010)
    /// - `AUDCLNT_E_ENDPOINT_OFFLOAD_NOT_CAPABLE`
    fn is_device_invalidated_error(msg: &str) -> bool {
        let msg_upper = msg.to_uppercase();
        msg_upper.contains("AUDCLNT_E_DEVICE_INVALIDATED")
            || msg_upper.contains("AUDCLNT_E_SERVICE_NOT_RUNNING")
            || msg_upper.contains("88890004")
            || msg_upper.contains("88890010")
            || msg_upper.contains("DEVICE_INVALIDATED")
            || msg_upper.contains("ENDPOINT_CREATE_FAILED")
    }

    /// Stop the capture stream.
    pub fn stop(&self) -> Result<()> {
        self.audio_client
            .stop_stream()
            .map_err(|e| anyhow::anyhow!("Failed to stop capture stream: {:?}", e))?;
        Ok(())
    }

    /// The sample rate this capture was configured with.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
