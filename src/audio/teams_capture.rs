// WASAPI application loopback capture for a specific process (Teams).
//
// This entire module is Windows-only since it depends on the `wasapi` crate
// and the Windows 11 Application Loopback API.

#![cfg(target_os = "windows")]

use std::collections::VecDeque;

use anyhow::Result;
use wasapi::*;

/// Captures audio from a specific process (e.g. Teams) via WASAPI Application Loopback.
///
/// This uses the Windows 11 per-process audio capture API. The `include_tree`
/// flag controls whether child processes of the target are also captured.
///
/// The captured format is 16-bit mono PCM at the requested sample rate.
/// WASAPI's autoconvert feature handles any necessary resampling.
pub struct TeamsCapture {
    audio_client: AudioClient,
    capture_client: AudioCaptureClient,
    event_handle: Handle,
    sample_rate: u32,
    process_id: u32,
}

impl TeamsCapture {
    /// Create a new `TeamsCapture` for the given process ID.
    ///
    /// `process_id` must be a valid PID of the target process.
    /// `desired_sample_rate` should be 16000 (for VAD compatibility) or 8000.
    pub fn new(process_id: u32, desired_sample_rate: u32) -> Result<Self> {
        initialize_mta().map_err(|e| anyhow::anyhow!("COM MTA initialization failed: {:?}", e))?;

        // Request 16-bit mono PCM at the desired sample rate.
        let desired_format = WaveFormat::new(
            16,                           // bits per sample
            16,                           // valid bits per sample
            &SampleType::Int,             // integer samples
            desired_sample_rate as usize, // sample rate
            1,                            // mono
            None,                         // no specific channel mask
        );

        // Use the Windows 11 Application Loopback API.
        // include_tree = true captures audio from the process and its child processes.
        let include_tree = true;
        let mut audio_client =
            AudioClient::new_application_loopback_client(process_id, include_tree).map_err(
                |e| {
                    anyhow::anyhow!(
                        "Failed to create application loopback client for PID {}: {:?}",
                        process_id,
                        e
                    )
                },
            )?;

        // Use event-driven shared mode with autoconvert so WASAPI handles
        // resampling from the process's audio format to our desired format.
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        };
        audio_client
            .initialize_client(&desired_format, &Direction::Capture, &mode)
            .map_err(|e| {
                anyhow::anyhow!("Failed to initialize application loopback client: {:?}", e)
            })?;

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
            process_id,
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
    /// if no data was captured in this cycle.
    /// Returns `Err` on timeout or device errors.
    pub fn read_frames(&self) -> Result<Option<Vec<i16>>> {
        // Wait for WASAPI to signal that a buffer is ready.
        self.event_handle
            .wait_for_event(1000)
            .map_err(|e| anyhow::anyhow!("Event wait timeout/error: {:?}", e))?;

        // Read captured bytes into a VecDeque, matching the pattern from capture.rs.
        let mut sample_queue: VecDeque<u8> = VecDeque::new();
        let _buffer_info = self
            .capture_client
            .read_from_device_to_deque(&mut sample_queue)
            .map_err(|e| anyhow::anyhow!("Failed to read from capture device: {:?}", e))?;

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

    /// Stop the capture stream.
    pub fn stop(&self) -> Result<()> {
        self.audio_client
            .stop_stream()
            .map_err(|e| anyhow::anyhow!("Failed to stop capture stream: {:?}", e))?;
        Ok(())
    }

    /// The process ID this capture was configured for.
    pub fn process_id(&self) -> u32 {
        self.process_id
    }

    /// The sample rate this capture was configured with.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
