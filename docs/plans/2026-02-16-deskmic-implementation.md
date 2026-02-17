# deskmic Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a bulletproof, always-on Windows audio recorder that captures mic + Teams process audio with VAD-gated recording, plus an async batch transcription pipeline.

**Architecture:** Single Rust binary with subcommands (`record`, `transcribe`, `install`, `status`). Recording uses WASAPI via the `wasapi` crate with per-process Application Loopback for Teams. Silero VAD via `voice_activity_detector` gates file writing. Transcription is a separate subcommand using pluggable backends (local whisper-rs, Azure OpenAI API). No async runtime — OS threads + `std::sync::mpsc` channels.

**Tech Stack:** Rust, wasapi 0.22, voice_activity_detector 0.2, hound 3.5, whisper-rs 0.15, tray-icon 0.21, clap 4.5, sysinfo 0.38, tracing, serde/toml

**Design doc:** `docs/plans/2026-02-16-deskmic-design.md`

---

## Task 1: Project Scaffold & Configuration

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/config.rs`
- Create: `src/cli.rs`
- Test: `src/config.rs` (inline tests)

**Step 1: Initialize the Rust project**

```bash
cargo init --name deskmic
```

**Step 2: Set up Cargo.toml with all dependencies**

```toml
[package]
name = "deskmic"
version = "0.1.0"
edition = "2024"
description = "Always-on Windows audio recorder with VAD and batch transcription"
license = "MIT"

[dependencies]
# CLI
clap = { version = "4.5", features = ["derive"] }

# Config
serde = { version = "1", features = ["derive"] }
toml = "0.8"
serde_json = "1"

# Audio capture (Windows WASAPI)
wasapi = "0.22"

# WAV writing
hound = "3.5"

# Voice Activity Detection (Silero VAD)
voice_activity_detector = "0.2"

# Process enumeration
sysinfo = "0.38"

# System tray
tray-icon = "0.21"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["fmt", "env-filter"] }

# Utilities
chrono = { version = "0.4", features = ["serde"] }
dirs = "6"
anyhow = "1"
thiserror = "2"

# Transcription (local)
whisper-rs = "0.15"

# Transcription (cloud)
reqwest = { version = "0.12", features = ["blocking", "multipart", "json"] }
```

**Step 3: Write the CLI parser in `src/cli.rs`**

```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "deskmic", version, about = "Always-on Windows audio recorder with VAD and batch transcription")]
pub struct Cli {
    /// Path to config file
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start recording (default if no subcommand)
    Record,

    /// Add deskmic to Windows Startup folder
    Install,

    /// Remove deskmic from Windows Startup folder
    Uninstall,

    /// Show recording status, disk usage, file count
    Status,

    /// Transcribe pending audio files
    Transcribe {
        /// Run as idle-aware daemon instead of one-shot
        #[arg(long)]
        watch: bool,

        /// Force a specific backend (local or azure)
        #[arg(long)]
        backend: Option<String>,
    },
}
```

**Step 4: Write the config module in `src/config.rs`**

```rust
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub capture: CaptureConfig,
    pub vad: VadConfig,
    pub output: OutputConfig,
    pub targets: TargetsConfig,
    pub storage: StorageConfig,
    pub transcription: TranscriptionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    pub pre_speech_buffer_secs: f32,
    pub silence_threshold_secs: f32,
    pub speech_threshold: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub directory: PathBuf,
    pub max_file_duration_mins: u32,
    pub organize_by_date: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetsConfig {
    pub processes: Vec<String>,
    pub mic_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub retention_days: u32,
    pub cleanup_interval_hours: u32,
    pub max_disk_usage_gb: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptionConfig {
    pub backend: String,
    pub model: String,
    pub azure: AzureConfig,
    pub idle_watch: IdleWatchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AzureConfig {
    pub endpoint: String,
    pub api_key: String,
    pub deployment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleWatchConfig {
    pub cpu_threshold_percent: f32,
    pub idle_check_interval_secs: u64,
}

// Implement Default for all config structs with sensible values
impl Default for Config { /* ... all fields with defaults ... */ }
impl Default for CaptureConfig {
    fn default() -> Self {
        Self { sample_rate: 16000, bit_depth: 16, channels: 1 }
    }
}
impl Default for VadConfig {
    fn default() -> Self {
        Self { pre_speech_buffer_secs: 5.0, silence_threshold_secs: 3.0, speech_threshold: 0.5 }
    }
}
// ... etc for all config structs (see design doc for default values)

impl Config {
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        // 1. Check explicit path
        // 2. Check beside exe
        // 3. Check %APPDATA%\deskmic\config.toml
        // 4. Fall back to defaults
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_valid() {
        let config = Config::default();
        assert_eq!(config.capture.sample_rate, 16000);
        assert_eq!(config.vad.speech_threshold, 0.5);
        assert_eq!(config.storage.retention_days, 30);
    }

    #[test]
    fn test_parse_toml_config() {
        let toml_str = r#"
            [capture]
            sample_rate = 48000

            [vad]
            speech_threshold = 0.8
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.capture.sample_rate, 48000);
        assert_eq!(config.vad.speech_threshold, 0.8);
        // Defaults still applied for unspecified fields
        assert_eq!(config.storage.retention_days, 30);
    }
}
```

**Step 5: Write minimal `src/main.rs`**

```rust
mod cli;
mod config;

use clap::Parser;
use cli::{Cli, Commands};
use config::Config;

fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("deskmic=info".parse()?)
        )
        .init();

    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;

    match cli.command.unwrap_or(Commands::Record) {
        Commands::Record => {
            tracing::info!("Starting recording...");
            // TODO: Task 3+
            Ok(())
        }
        Commands::Install => {
            tracing::info!("Installing to startup...");
            // TODO: Task 9
            Ok(())
        }
        Commands::Uninstall => {
            tracing::info!("Removing from startup...");
            // TODO: Task 9
            Ok(())
        }
        Commands::Status => {
            tracing::info!("Checking status...");
            // TODO: Task 10
            Ok(())
        }
        Commands::Transcribe { watch, backend } => {
            tracing::info!("Transcribing...");
            // TODO: Task 7+
            Ok(())
        }
    }
}
```

**Step 6: Verify it compiles and runs**

```bash
cargo build
cargo run -- --help
cargo run -- record
cargo run -- transcribe --help
cargo test
```

Expected: compiles, `--help` shows CLI structure, `record` prints "Starting recording...", tests pass.

**Step 7: Commit**

```bash
git init
git add -A
git commit -m "feat: project scaffold with CLI and config parsing"
```

---

## Task 2: Ring Buffer & VAD Engine

**Files:**
- Create: `src/audio/mod.rs`
- Create: `src/audio/ring_buffer.rs`
- Create: `src/audio/vad.rs`
- Modify: `src/main.rs` (add `mod audio`)

**Step 1: Write ring buffer tests**

```rust
// src/audio/ring_buffer.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_buffer() {
        let buf = RingBuffer::new(16000, 5.0); // 5 seconds at 16kHz
        assert_eq!(buf.len(), 0);
        assert!(buf.drain().is_empty());
    }

    #[test]
    fn test_push_and_drain() {
        let mut buf = RingBuffer::new(16000, 5.0);
        let samples: Vec<i16> = (0..1000).collect();
        buf.push(&samples);
        assert_eq!(buf.len(), 1000);
        let drained = buf.drain();
        assert_eq!(drained.len(), 1000);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_overflow_evicts_oldest() {
        let mut buf = RingBuffer::new(16000, 1.0); // 1 second = 16000 samples
        let samples: Vec<i16> = (0..20000).map(|i| i as i16).collect();
        buf.push(&samples);
        assert_eq!(buf.len(), 16000); // capped at capacity
        let drained = buf.drain();
        // Should contain the newest 16000 samples (4000..20000)
        assert_eq!(drained[0], 4000);
    }
}
```

**Step 2: Implement RingBuffer**

```rust
// src/audio/ring_buffer.rs
use std::collections::VecDeque;

pub struct RingBuffer {
    buffer: VecDeque<i16>,
    capacity: usize,
}

impl RingBuffer {
    pub fn new(sample_rate: u32, duration_secs: f32) -> Self {
        let capacity = (sample_rate as f32 * duration_secs) as usize;
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, samples: &[i16]) {
        for &sample in samples {
            if self.buffer.len() >= self.capacity {
                self.buffer.pop_front();
            }
            self.buffer.push_back(sample);
        }
    }

    pub fn drain(&mut self) -> Vec<i16> {
        self.buffer.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }
}
```

**Step 3: Run ring buffer tests**

```bash
cargo test ring_buffer
```

Expected: all 3 tests pass.

**Step 4: Write VAD wrapper**

```rust
// src/audio/vad.rs
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
        Ok(Self { detector, threshold })
    }

    /// Returns true if the chunk contains speech.
    /// `samples` should be exactly chunk_size samples (512 for 16kHz).
    pub fn is_speech(&mut self, samples: &[i16]) -> bool {
        let probability = self.detector.predict(samples.to_vec());
        probability >= self.threshold
    }
}
```

**Step 5: Write VAD integration test**

```rust
// src/audio/vad.rs
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
```

**Step 6: Wire up `src/audio/mod.rs`**

```rust
pub mod ring_buffer;
pub mod vad;
```

**Step 7: Add `mod audio;` to `src/main.rs` and run all tests**

```bash
cargo test
```

Expected: all tests pass (config + ring_buffer + vad).

**Step 8: Commit**

```bash
git add -A
git commit -m "feat: ring buffer and Silero VAD wrapper"
```

---

## Task 3: Microphone Capture Pipeline

**Files:**
- Create: `src/audio/capture.rs`
- Create: `src/audio/pipeline.rs`
- Modify: `src/audio/mod.rs`

**Step 1: Write the WASAPI mic capture module**

```rust
// src/audio/capture.rs
use anyhow::Result;
use wasapi::*;

/// Captures audio from the default microphone via WASAPI.
pub struct MicCapture {
    audio_client: AudioClient,
    capture_client: AudioCaptureClient,
    event_handle: Handle,
    sample_rate: u32,
}

impl MicCapture {
    pub fn new(desired_sample_rate: u32) -> Result<Self> {
        initialize_mta().ok().map_err(|e| anyhow::anyhow!("COM init failed: {:?}", e))?;

        // Get default capture device
        let device = get_default_device(&Direction::Capture)?;
        let mut audio_client = device.get_iaudioclient()?;

        let desired_format = WaveFormat::new(
            16, 16,
            &SampleType::Int,
            desired_sample_rate as usize,
            1, // mono
            None,
        );

        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        };

        audio_client.initialize_client(&desired_format, &Direction::Capture, &mode)?;
        let event_handle = audio_client.set_get_eventhandle()?;
        let capture_client = audio_client.get_audiocaptureclient()?;

        Ok(Self {
            audio_client,
            capture_client,
            event_handle,
            sample_rate: desired_sample_rate,
        })
    }

    pub fn start(&self) -> Result<()> {
        self.audio_client.start_stream()?;
        Ok(())
    }

    /// Blocks until audio data is available, then returns samples as i16.
    /// Returns None if the stream was invalidated (e.g., sleep/wake).
    pub fn read_frames(&self) -> Result<Option<Vec<i16>>> {
        self.event_handle.wait_for_event(1000)
            .map_err(|e| anyhow::anyhow!("Event wait failed: {:?}", e))?;

        match self.capture_client.read_from_device_to_deveice_as_bytes() {
            // Note: actual API may differ — check wasapi crate docs for exact method
            // The capture client returns raw bytes; convert to i16 samples
            Ok(bytes) => {
                let samples: Vec<i16> = bytes.chunks_exact(2)
                    .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
                    .collect();
                Ok(Some(samples))
            }
            Err(e) => {
                // Check if device was invalidated (sleep/wake)
                tracing::warn!("Capture read error: {:?}", e);
                Ok(None)
            }
        }
    }

    pub fn stop(&self) -> Result<()> {
        self.audio_client.stop_stream()?;
        Ok(())
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
```

> **Note to implementer:** The exact `wasapi` crate API for reading captured audio may differ from above. Consult the crate's `record` example at https://github.com/HEnquist/wasapi-rs/tree/master/examples. The key pattern is: event-driven capture → read bytes → convert to i16 samples. Adjust method names as needed.

**Step 2: Write the capture pipeline**

This is the core loop that ties capture → ring buffer → VAD → file writer channel.

```rust
// src/audio/pipeline.rs
use std::sync::mpsc::Sender;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::Result;
use crate::audio::ring_buffer::RingBuffer;
use crate::audio::vad::Vad;
use crate::config::VadConfig;

/// Message sent from capture pipeline to file writer.
pub enum AudioMessage {
    /// Speech started — includes the pre-buffer audio.
    SpeechStart {
        source: String,
        samples: Vec<i16>,
        sample_rate: u32,
    },
    /// Continuing speech data.
    SpeechContinue {
        source: String,
        samples: Vec<i16>,
    },
    /// Speech ended — silence threshold exceeded.
    SpeechEnd {
        source: String,
    },
}

/// Runs the capture → VAD → file writer pipeline.
/// Call this on a dedicated thread.
pub fn run_capture_pipeline(
    source_name: String,
    capture_fn: impl Fn() -> Result<Option<Vec<i16>>>,
    start_fn: impl Fn() -> Result<()>,
    sample_rate: u32,
    vad_config: &VadConfig,
    sender: Sender<AudioMessage>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let mut ring_buffer = RingBuffer::new(sample_rate, vad_config.pre_speech_buffer_secs);
    let mut vad = Vad::new(sample_rate, vad_config.speech_threshold)?;

    let chunk_size: usize = if sample_rate == 16000 { 512 } else { 256 };
    let silence_samples = (sample_rate as f32 * vad_config.silence_threshold_secs) as usize;

    let mut is_speaking = false;
    let mut silence_count: usize = 0;
    let mut pending_samples: Vec<i16> = Vec::new();

    start_fn()?;

    while !shutdown.load(Ordering::Relaxed) {
        let samples = match capture_fn()? {
            Some(s) => s,
            None => {
                // Device invalidated — caller should handle re-init
                tracing::warn!("{}: capture returned None, device may be invalidated", source_name);
                break;
            }
        };

        // Accumulate samples and process in chunk_size blocks
        pending_samples.extend_from_slice(&samples);

        while pending_samples.len() >= chunk_size {
            let chunk: Vec<i16> = pending_samples.drain(..chunk_size).collect();
            let speech = vad.is_speech(&chunk);

            if speech {
                silence_count = 0;

                if !is_speaking {
                    // Speech just started — flush ring buffer as pre-buffer
                    is_speaking = true;
                    let pre_buffer = ring_buffer.drain();
                    let mut initial = pre_buffer;
                    initial.extend_from_slice(&chunk);
                    sender.send(AudioMessage::SpeechStart {
                        source: source_name.clone(),
                        samples: initial,
                        sample_rate,
                    })?;
                } else {
                    sender.send(AudioMessage::SpeechContinue {
                        source: source_name.clone(),
                        samples: chunk,
                    })?;
                }
            } else {
                if is_speaking {
                    silence_count += chunk_size;
                    // Still write the silence (tail)
                    sender.send(AudioMessage::SpeechContinue {
                        source: source_name.clone(),
                        samples: chunk,
                    })?;

                    if silence_count >= silence_samples {
                        is_speaking = false;
                        silence_count = 0;
                        sender.send(AudioMessage::SpeechEnd {
                            source: source_name.clone(),
                        })?;
                    }
                } else {
                    // Not speaking — feed into ring buffer
                    ring_buffer.push(&chunk);
                }
            }
        }
    }

    // Graceful shutdown: end any open speech segment
    if is_speaking {
        sender.send(AudioMessage::SpeechEnd {
            source: source_name.clone(),
        })?;
    }

    Ok(())
}
```

**Step 3: Update `src/audio/mod.rs`**

```rust
pub mod capture;
pub mod pipeline;
pub mod ring_buffer;
pub mod vad;
```

**Step 4: Verify it compiles**

```bash
cargo build
```

Expected: compiles (may need adjustments to wasapi API calls based on actual crate docs). Existing tests still pass.

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: mic capture and VAD pipeline with ring buffer"
```

---

## Task 4: Teams Process Audio Capture

**Files:**
- Create: `src/audio/teams_capture.rs`
- Create: `src/audio/teams_monitor.rs`
- Modify: `src/audio/mod.rs`

**Step 1: Write Teams process capture**

```rust
// src/audio/teams_capture.rs
use anyhow::Result;
use wasapi::*;

/// Captures audio from a specific process (Teams) via WASAPI Application Loopback.
/// Requires Windows 11.
pub struct TeamsCapture {
    audio_client: AudioClient,
    capture_client: AudioCaptureClient,
    event_handle: Handle,
    sample_rate: u32,
    process_id: u32,
}

impl TeamsCapture {
    pub fn new(process_id: u32, desired_sample_rate: u32) -> Result<Self> {
        initialize_mta().ok().map_err(|e| anyhow::anyhow!("COM init failed: {:?}", e))?;

        let desired_format = WaveFormat::new(
            16, 16,
            &SampleType::Int,
            desired_sample_rate as usize,
            1, // mono
            None,
        );

        let include_tree = true; // capture child processes too
        let mut audio_client = AudioClient::new_application_loopback_client(
            process_id,
            include_tree,
        )?;

        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        };

        audio_client.initialize_client(&desired_format, &Direction::Capture, &mode)?;
        let event_handle = audio_client.set_get_eventhandle()?;
        let capture_client = audio_client.get_audiocaptureclient()?;

        Ok(Self {
            audio_client,
            capture_client,
            event_handle,
            sample_rate: desired_sample_rate,
            process_id,
        })
    }

    pub fn start(&self) -> Result<()> {
        self.audio_client.start_stream()?;
        Ok(())
    }

    pub fn read_frames(&self) -> Result<Option<Vec<i16>>> {
        // Same pattern as MicCapture::read_frames
        // Event wait → read bytes → convert to i16
        self.event_handle.wait_for_event(1000)
            .map_err(|e| anyhow::anyhow!("Event wait failed: {:?}", e))?;

        // Read and convert — adjust to actual wasapi API
        // Return None if device/process invalidated
        todo!("Implement based on wasapi crate's actual read API")
    }

    pub fn stop(&self) -> Result<()> {
        self.audio_client.stop_stream()?;
        Ok(())
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }
}
```

**Step 2: Write the Teams process monitor**

```rust
// src/audio/teams_monitor.rs
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::ffi::OsStr;
use sysinfo::{ProcessRefreshKind, RefreshKind, System};
use anyhow::Result;
use crate::audio::pipeline::{run_capture_pipeline, AudioMessage};
use crate::audio::teams_capture::TeamsCapture;
use crate::config::Config;

/// Finds the PID of the first matching Teams process.
pub fn find_teams_pid(process_names: &[String]) -> Option<u32> {
    let refreshes = RefreshKind::nothing()
        .with_processes(ProcessRefreshKind::new());
    let system = System::new_with_specifics(refreshes);

    for name in process_names {
        let mut procs = system.processes_by_name(OsStr::new(name));
        if let Some(proc) = procs.next() {
            return Some(proc.pid().as_u32());
        }
    }
    None
}

/// Monitors for Teams process and spawns/stops capture pipeline.
/// Run this on a dedicated thread.
pub fn run_teams_monitor(
    config: Config,
    sender: Sender<AudioMessage>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let mut active_pid: Option<u32> = None;
    let mut pipeline_shutdown: Option<Arc<AtomicBool>> = None;
    let mut pipeline_handle: Option<std::thread::JoinHandle<()>> = None;

    while !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_secs(5));

        let current_pid = find_teams_pid(&config.targets.processes);

        match (active_pid, current_pid) {
            (None, Some(pid)) => {
                // Teams just started — spawn capture pipeline
                tracing::info!("Teams detected (PID {}), starting capture", pid);
                let pipe_shutdown = Arc::new(AtomicBool::new(false));
                let pipe_shutdown_clone = pipe_shutdown.clone();
                let sender_clone = sender.clone();
                let vad_config = config.vad.clone();
                let sample_rate = config.capture.sample_rate;

                let handle = std::thread::Builder::new()
                    .name("teams-capture".into())
                    .spawn(move || {
                        match TeamsCapture::new(pid, sample_rate) {
                            Ok(capture) => {
                                let start = || capture.start();
                                let read = || capture.read_frames();
                                if let Err(e) = run_capture_pipeline(
                                    "teams".to_string(),
                                    read,
                                    start,
                                    sample_rate,
                                    &vad_config,
                                    sender_clone,
                                    pipe_shutdown_clone,
                                ) {
                                    tracing::error!("Teams pipeline error: {:?}", e);
                                }
                            }
                            Err(e) => tracing::error!("Failed to start Teams capture: {:?}", e),
                        }
                    })?;

                active_pid = Some(pid);
                pipeline_shutdown = Some(pipe_shutdown);
                pipeline_handle = Some(handle);
            }
            (Some(_), None) => {
                // Teams exited — stop capture pipeline
                tracing::info!("Teams process gone, stopping capture");
                if let Some(ps) = pipeline_shutdown.take() {
                    ps.store(true, Ordering::Relaxed);
                }
                if let Some(handle) = pipeline_handle.take() {
                    let _ = handle.join();
                }
                active_pid = None;
            }
            (Some(old_pid), Some(new_pid)) if old_pid != new_pid => {
                // Teams restarted with new PID — restart pipeline
                tracing::info!("Teams PID changed {} -> {}, restarting capture", old_pid, new_pid);
                if let Some(ps) = pipeline_shutdown.take() {
                    ps.store(true, Ordering::Relaxed);
                }
                if let Some(handle) = pipeline_handle.take() {
                    let _ = handle.join();
                }
                active_pid = None;
                // Will be picked up on next loop iteration
            }
            _ => {} // No change
        }
    }

    // Cleanup
    if let Some(ps) = pipeline_shutdown.take() {
        ps.store(true, Ordering::Relaxed);
    }
    if let Some(handle) = pipeline_handle.take() {
        let _ = handle.join();
    }

    Ok(())
}
```

**Step 3: Write a unit test for `find_teams_pid`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_nonexistent_process() {
        let pid = find_teams_pid(&["definitely-not-a-real-process-12345.exe".to_string()]);
        assert!(pid.is_none());
    }
}
```

**Step 4: Update `src/audio/mod.rs` and verify compilation**

```bash
cargo build
cargo test
```

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: Teams process audio capture and monitor"
```

---

## Task 5: File Writer Thread

**Files:**
- Create: `src/audio/file_writer.rs`
- Modify: `src/audio/mod.rs`

**Step 1: Write the file writer**

```rust
// src/audio/file_writer.rs
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use anyhow::Result;
use chrono::Local;
use hound::{WavSpec, WavWriter};
use crate::audio::pipeline::AudioMessage;
use crate::config::OutputConfig;

struct ActiveFile {
    writer: WavWriter<std::io::BufWriter<std::fs::File>>,
    path: PathBuf,
    sample_count: usize,
    max_samples: usize,
}

/// Runs the file writer loop. Call on a dedicated thread.
pub fn run_file_writer(
    receiver: Receiver<AudioMessage>,
    output_config: &OutputConfig,
    sample_rate: u32,
) -> Result<()> {
    let mut active_files: HashMap<String, ActiveFile> = HashMap::new();

    let max_samples = (output_config.max_file_duration_mins as usize) * 60 * sample_rate as usize;

    for msg in receiver {
        match msg {
            AudioMessage::SpeechStart { source, samples, sample_rate: sr } => {
                // Close any existing file for this source
                if let Some(active) = active_files.remove(&source) {
                    active.writer.finalize()?;
                    tracing::info!("Closed {}", active.path.display());
                }

                // Create new file
                let path = make_file_path(&output_config.directory, &source, output_config.organize_by_date);
                std::fs::create_dir_all(path.parent().unwrap())?;

                let spec = WavSpec {
                    channels: 1,
                    sample_rate: sr,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let mut writer = WavWriter::create(&path, spec)?;
                for &sample in &samples {
                    writer.write_sample(sample)?;
                }
                tracing::info!("Started recording: {}", path.display());

                active_files.insert(source.clone(), ActiveFile {
                    writer,
                    path,
                    sample_count: samples.len(),
                    max_samples,
                });
            }

            AudioMessage::SpeechContinue { source, samples } => {
                if let Some(active) = active_files.get_mut(&source) {
                    for &sample in &samples {
                        active.writer.write_sample(sample)?;
                    }
                    active.sample_count += samples.len();

                    // Force rotation if max duration reached
                    if active.sample_count >= active.max_samples {
                        let active = active_files.remove(&source).unwrap();
                        active.writer.finalize()?;
                        tracing::info!("Rotated (max duration): {}", active.path.display());
                        // Next SpeechContinue will be dropped until new SpeechStart
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

    // Channel closed — finalize all open files
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
        base_dir.join(now.format("%Y-%m-%d").to_string()).join(filename)
    } else {
        base_dir.join(filename)
    }
}
```

**Step 2: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_file_path_with_date() {
        let path = make_file_path(Path::new("/tmp/recordings"), "mic", true);
        let path_str = path.to_str().unwrap();
        assert!(path_str.contains("mic_"));
        assert!(path_str.ends_with(".wav"));
        // Should have a date directory component
        assert!(path_str.contains(&Local::now().format("%Y-%m-%d").to_string()));
    }

    #[test]
    fn test_make_file_path_without_date() {
        let path = make_file_path(Path::new("/tmp/recordings"), "teams", false);
        let path_str = path.to_str().unwrap();
        assert!(path_str.starts_with("/tmp/recordings/teams_"));
        assert!(!path_str.contains(&Local::now().format("%Y-%m-%d").to_string()));
    }
}
```

**Step 3: Run tests**

```bash
cargo test file_writer
```

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: file writer thread with WAV output and rotation"
```

---

## Task 6: Recording Orchestrator (Wire It All Together)

**Files:**
- Create: `src/recorder.rs`
- Modify: `src/main.rs`

**Step 1: Write the recorder orchestrator**

This is the top-level module that spawns all threads and manages the recording session.

```rust
// src/recorder.rs
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use anyhow::Result;
use crate::audio::capture::MicCapture;
use crate::audio::file_writer::run_file_writer;
use crate::audio::pipeline::{run_capture_pipeline, AudioMessage};
use crate::audio::teams_monitor::run_teams_monitor;
use crate::config::Config;

pub fn run_recorder(config: Config) -> Result<()> {
    let shutdown = Arc::new(AtomicBool::new(false));

    // Set up Ctrl+C handler
    let shutdown_ctrlc = shutdown.clone();
    ctrlc::set_handler(move || {
        tracing::info!("Shutdown signal received");
        shutdown_ctrlc.store(true, Ordering::Relaxed);
    })?;

    let (sender, receiver) = mpsc::channel::<AudioMessage>();

    // Spawn file writer thread
    let output_config = config.output.clone();
    let sample_rate = config.capture.sample_rate;
    let writer_handle = std::thread::Builder::new()
        .name("file-writer".into())
        .spawn(move || {
            if let Err(e) = run_file_writer(receiver, &output_config, sample_rate) {
                tracing::error!("File writer error: {:?}", e);
            }
        })?;

    // Spawn mic capture pipeline
    let mic_sender = sender.clone();
    let mic_shutdown = shutdown.clone();
    let mic_vad_config = config.vad.clone();
    let mic_sample_rate = config.capture.sample_rate;
    let mic_enabled = config.targets.mic_enabled;

    let mic_handle = if mic_enabled {
        Some(std::thread::Builder::new()
            .name("mic-capture".into())
            .spawn(move || {
                // Outer recovery loop
                while !mic_shutdown.load(Ordering::Relaxed) {
                    match MicCapture::new(mic_sample_rate) {
                        Ok(capture) => {
                            let start = || capture.start();
                            let read = || capture.read_frames();
                            match run_capture_pipeline(
                                "mic".to_string(),
                                read,
                                start,
                                mic_sample_rate,
                                &mic_vad_config,
                                mic_sender.clone(),
                                mic_shutdown.clone(),
                            ) {
                                Ok(()) => break, // clean shutdown
                                Err(e) => {
                                    tracing::error!("Mic pipeline error: {:?}, restarting in 2s", e);
                                    std::thread::sleep(std::time::Duration::from_secs(2));
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Mic init failed: {:?}, retrying in 2s", e);
                            std::thread::sleep(std::time::Duration::from_secs(2));
                        }
                    }
                }
            })?)
    } else {
        None
    };

    // Spawn Teams monitor
    let teams_sender = sender.clone();
    let teams_shutdown = shutdown.clone();
    let teams_config = config.clone();
    let teams_handle = std::thread::Builder::new()
        .name("teams-monitor".into())
        .spawn(move || {
            if let Err(e) = run_teams_monitor(teams_config, teams_sender, teams_shutdown) {
                tracing::error!("Teams monitor error: {:?}", e);
            }
        })?;

    // Drop our sender so file writer sees channel close on shutdown
    drop(sender);

    // Wait for shutdown
    while !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    tracing::info!("Shutting down...");

    // Wait for threads
    if let Some(h) = mic_handle { let _ = h.join(); }
    let _ = teams_handle.join();
    let _ = writer_handle.join();

    tracing::info!("Shutdown complete");
    Ok(())
}
```

**Step 2: Add `ctrlc` dependency to `Cargo.toml`**

```toml
ctrlc = { version = "3.4", features = ["termination"] }
```

**Step 3: Wire into `src/main.rs`**

Update the `Commands::Record` match arm:

```rust
Commands::Record => {
    tracing::info!("Starting deskmic recorder");
    crate::recorder::run_recorder(config)
}
```

**Step 4: Verify compilation**

```bash
cargo build
```

**Step 5: Manual smoke test (on Windows)**

```bash
cargo run
# Should start, show "Starting deskmic recorder", attempt to open mic
# Ctrl+C should trigger graceful shutdown
```

**Step 6: Commit**

```bash
git add -A
git commit -m "feat: recording orchestrator with crash recovery and graceful shutdown"
```

---

## Task 7: Storage Cleanup

**Files:**
- Create: `src/storage.rs`
- Modify: `src/main.rs`

**Step 1: Write the cleanup module with tests**

```rust
// src/storage.rs
use std::path::Path;
use chrono::{Local, NaiveDate};
use anyhow::Result;
use crate::config::StorageConfig;

/// Deletes recording folders older than retention_days.
pub fn cleanup_old_recordings(recordings_dir: &Path, config: &StorageConfig) -> Result<u64> {
    let cutoff = Local::now().date_naive() - chrono::Duration::days(config.retention_days as i64);
    let mut bytes_freed: u64 = 0;

    if !recordings_dir.exists() {
        return Ok(0);
    }

    for entry in std::fs::read_dir(recordings_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Only process date-formatted directories (YYYY-MM-DD)
        if let Ok(folder_date) = NaiveDate::parse_from_str(&name_str, "%Y-%m-%d") {
            if folder_date < cutoff {
                let size = dir_size(&entry.path())?;
                std::fs::remove_dir_all(entry.path())?;
                bytes_freed += size;
                tracing::info!("Deleted old recordings: {} ({} bytes)", name_str, size);
            }
        }
    }

    Ok(bytes_freed)
}

/// Enforce max disk usage by deleting oldest folders first.
pub fn enforce_disk_limit(recordings_dir: &Path, max_bytes: u64) -> Result<()> {
    let current = dir_size(recordings_dir)?;
    if current <= max_bytes {
        return Ok(());
    }

    // Collect date folders, sorted oldest first
    let mut folders: Vec<(NaiveDate, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(recordings_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Ok(date) = NaiveDate::parse_from_str(&name_str, "%Y-%m-%d") {
            folders.push((date, entry.path()));
        }
    }
    folders.sort_by_key(|(date, _)| *date);

    let mut remaining = current;
    for (date, path) in folders {
        if remaining <= max_bytes {
            break;
        }
        let size = dir_size(&path)?;
        std::fs::remove_dir_all(&path)?;
        remaining -= size;
        tracing::info!("Deleted {} to free space ({} bytes)", date, size);
    }

    Ok(())
}

fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                total += metadata.len();
            } else if metadata.is_dir() {
                total += dir_size(&entry.path())?;
            }
        }
    }
    Ok(total)
}

/// Returns (total_files, total_bytes) for the recordings directory.
pub fn get_storage_stats(recordings_dir: &Path) -> Result<(usize, u64)> {
    let mut count = 0;
    let mut bytes = 0;

    if !recordings_dir.exists() {
        return Ok((0, 0));
    }

    for entry in std::fs::read_dir(recordings_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            for file in std::fs::read_dir(entry.path())? {
                let file = file?;
                if file.file_type()?.is_file() {
                    count += 1;
                    bytes += file.metadata()?.len();
                }
            }
        }
    }

    Ok((count, bytes))
}

/// Run cleanup loop on a dedicated thread.
pub fn run_cleanup_loop(
    recordings_dir: std::path::PathBuf,
    config: StorageConfig,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let interval = std::time::Duration::from_secs(config.cleanup_interval_hours as u64 * 3600);

    // Run immediately on start
    run_cleanup_once(&recordings_dir, &config);

    while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
        // Sleep in small increments to check shutdown
        let start = std::time::Instant::now();
        while start.elapsed() < interval && !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_secs(10));
        }

        if !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            run_cleanup_once(&recordings_dir, &config);
        }
    }
}

fn run_cleanup_once(recordings_dir: &Path, config: &StorageConfig) {
    if let Err(e) = cleanup_old_recordings(recordings_dir, config) {
        tracing::error!("Cleanup error: {:?}", e);
    }
    if let Some(max_gb) = config.max_disk_usage_gb {
        let max_bytes = (max_gb * 1_073_741_824.0) as u64;
        if let Err(e) = enforce_disk_limit(recordings_dir, max_bytes) {
            tracing::error!("Disk limit enforcement error: {:?}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir; // add tempfile = "3" to [dev-dependencies]

    #[test]
    fn test_cleanup_old_recordings() {
        let tmp = TempDir::new().unwrap();

        // Create an "old" folder
        let old_date = (Local::now().date_naive() - chrono::Duration::days(35))
            .format("%Y-%m-%d").to_string();
        let old_dir = tmp.path().join(&old_date);
        fs::create_dir(&old_dir).unwrap();
        fs::write(old_dir.join("test.wav"), b"fake audio data").unwrap();

        // Create a "recent" folder
        let recent_date = Local::now().date_naive().format("%Y-%m-%d").to_string();
        let recent_dir = tmp.path().join(&recent_date);
        fs::create_dir(&recent_dir).unwrap();
        fs::write(recent_dir.join("test.wav"), b"fake audio data").unwrap();

        let config = StorageConfig {
            retention_days: 30,
            cleanup_interval_hours: 24,
            max_disk_usage_gb: None,
        };

        cleanup_old_recordings(tmp.path(), &config).unwrap();

        assert!(!old_dir.exists(), "Old folder should be deleted");
        assert!(recent_dir.exists(), "Recent folder should be kept");
    }
}
```

**Step 2: Add dev-dependency**

```toml
[dev-dependencies]
tempfile = "3"
```

**Step 3: Run tests**

```bash
cargo test storage
```

**Step 4: Wire cleanup thread into `recorder.rs`**

Add to `run_recorder()`:

```rust
// Spawn cleanup thread
let cleanup_dir = config.output.directory.clone();
let cleanup_config = config.storage.clone();
let cleanup_shutdown = shutdown.clone();
let cleanup_handle = std::thread::Builder::new()
    .name("cleanup".into())
    .spawn(move || {
        crate::storage::run_cleanup_loop(cleanup_dir, cleanup_config, cleanup_shutdown);
    })?;
```

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: automatic storage cleanup with retention and disk limits"
```

---

## Task 8: Transcription Pipeline

**Files:**
- Create: `src/transcribe/mod.rs`
- Create: `src/transcribe/backend.rs`
- Create: `src/transcribe/whisper_local.rs`
- Create: `src/transcribe/azure_openai.rs`
- Create: `src/transcribe/state.rs`
- Create: `src/transcribe/runner.rs`
- Modify: `src/main.rs`

**Step 1: Define the backend trait and transcript type**

```rust
// src/transcribe/backend.rs
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
}

pub trait TranscriptionBackend: Send {
    fn name(&self) -> &str;
    fn transcribe(&self, audio_path: &Path) -> Result<Transcript>;
}
```

**Step 2: Implement local Whisper backend**

```rust
// src/transcribe/whisper_local.rs
use std::path::Path;
use anyhow::Result;
use whisper_rs::{WhisperContext, WhisperContextParameters, FullParams, SamplingStrategy};
use crate::transcribe::backend::{Transcript, TranscriptionBackend};

pub struct WhisperLocal {
    ctx: WhisperContext,
}

impl WhisperLocal {
    pub fn new(model_path: &str) -> Result<Self> {
        let ctx = WhisperContext::new_with_params(
            model_path,
            WhisperContextParameters::default(),
        ).map_err(|e| anyhow::anyhow!("Failed to load Whisper model: {:?}", e))?;
        Ok(Self { ctx })
    }
}

impl TranscriptionBackend for WhisperLocal {
    fn name(&self) -> &str {
        "whisper-local"
    }

    fn transcribe(&self, audio_path: &Path) -> Result<Transcript> {
        // Read WAV file
        let mut reader = hound::WavReader::open(audio_path)?;
        let spec = reader.spec();
        let samples_i16: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();

        // Convert i16 to f32 normalized [-1.0, 1.0]
        let samples_f32: Vec<f32> = samples_i16.iter()
            .map(|&s| s as f32 / 32768.0)
            .collect();

        let duration_secs = samples_f32.len() as f64 / spec.sample_rate as f64;

        // Run whisper
        let mut state = self.ctx.create_state()
            .map_err(|e| anyhow::anyhow!("Failed to create state: {:?}", e))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(4);
        params.set_language(Some("en"));

        state.full(params, &samples_f32)
            .map_err(|e| anyhow::anyhow!("Transcription failed: {:?}", e))?;

        let mut text = String::new();
        let n_segments = state.full_n_segments()
            .map_err(|e| anyhow::anyhow!("Failed to get segments: {:?}", e))?;
        for i in 0..n_segments {
            if let Ok(segment) = state.full_get_segment_text(i) {
                text.push_str(&segment);
                text.push(' ');
            }
        }

        // Extract source and timestamp from filename
        let filename = audio_path.file_name().unwrap().to_string_lossy().to_string();
        let source = if filename.starts_with("mic") { "mic" } else { "teams" };
        let timestamp = audio_path.parent()
            .and_then(|p| p.file_name())
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();

        Ok(Transcript {
            timestamp,
            source: source.to_string(),
            duration_secs,
            file: filename,
            text: text.trim().to_string(),
        })
    }
}
```

**Step 3: Implement Azure OpenAI backend**

```rust
// src/transcribe/azure_openai.rs
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
        let filename = audio_path.file_name().unwrap().to_string_lossy().to_string();

        let form = multipart::Form::new()
            .part("file", multipart::Part::bytes(file_bytes)
                .file_name(filename.clone())
                .mime_str("audio/wav")?)
            .text("response_format", "json");

        let client = reqwest::blocking::Client::new();
        let response = client.post(&url)
            .header("api-key", &self.api_key)
            .multipart(form)
            .send()?;

        let body: serde_json::Value = response.json()?;
        let text = body["text"].as_str().unwrap_or("").to_string();

        // Get duration from WAV header
        let reader = hound::WavReader::open(audio_path)?;
        let spec = reader.spec();
        let duration_secs = reader.duration() as f64 / spec.sample_rate as f64;

        let source = if filename.starts_with("mic") { "mic" } else { "teams" };
        let timestamp = audio_path.parent()
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
```

**Step 4: Implement state tracking**

```rust
// src/transcribe/state.rs
use std::collections::HashSet;
use std::path::Path;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TranscriptionState {
    pub transcribed_files: HashSet<String>,
}

impl TranscriptionState {
    pub fn load(recordings_dir: &Path) -> Result<Self> {
        let path = recordings_dir.join(".deskmic-state.json");
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, recordings_dir: &Path) -> Result<()> {
        let path = recordings_dir.join(".deskmic-state.json");
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn is_transcribed(&self, file_path: &str) -> bool {
        self.transcribed_files.contains(file_path)
    }

    pub fn mark_transcribed(&mut self, file_path: String) {
        self.transcribed_files.insert(file_path);
    }
}
```

**Step 5: Implement the transcription runner**

```rust
// src/transcribe/runner.rs
use std::path::{Path, PathBuf};
use anyhow::Result;
use crate::config::Config;
use crate::transcribe::backend::{Transcript, TranscriptionBackend};
use crate::transcribe::state::TranscriptionState;
use crate::transcribe::whisper_local::WhisperLocal;
use crate::transcribe::azure_openai::AzureOpenAIBackend;

/// Find all unprocessed WAV files in the recordings directory.
fn find_pending_files(recordings_dir: &Path, state: &TranscriptionState) -> Result<Vec<PathBuf>> {
    let mut pending = Vec::new();

    if !recordings_dir.exists() {
        return Ok(pending);
    }

    for entry in std::fs::read_dir(recordings_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() { continue; }

        for file in std::fs::read_dir(entry.path())? {
            let file = file?;
            let path = file.path();
            if path.extension().map(|e| e == "wav").unwrap_or(false) {
                let relative = path.strip_prefix(recordings_dir)?
                    .to_string_lossy().to_string();
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
fn build_backend(config: &Config, backend_override: Option<&str>) -> Result<Box<dyn TranscriptionBackend>> {
    let backend_name = backend_override.unwrap_or(&config.transcription.backend);

    match backend_name {
        "local" => {
            // Model path: look in exe dir, then %APPDATA%\deskmic\models\
            let model_file = format!("ggml-{}.bin", config.transcription.model);
            // TODO: resolve actual model path
            Ok(Box::new(WhisperLocal::new(&model_file)?))
        }
        "azure" => {
            Ok(Box::new(AzureOpenAIBackend::new(&config.transcription.azure)?))
        }
        other => anyhow::bail!("Unknown transcription backend: {}", other),
    }
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

    let transcript_dir = recordings_dir.join("transcripts");
    std::fs::create_dir_all(&transcript_dir)?;

    for path in &pending {
        tracing::info!("Transcribing: {}", path.display());
        match backend.transcribe(path) {
            Ok(transcript) => {
                // Append to daily JSONL file
                let date_dir = path.parent().unwrap().file_name().unwrap().to_string_lossy();
                let jsonl_path = transcript_dir.join(format!("{}.jsonl", date_dir));
                let mut file = std::fs::OpenOptions::new()
                    .create(true).append(true).open(&jsonl_path)?;
                use std::io::Write;
                writeln!(file, "{}", serde_json::to_string(&transcript)?)?;

                // Mark as transcribed
                let relative = path.strip_prefix(recordings_dir)?
                    .to_string_lossy().to_string();
                state.mark_transcribed(relative);
                state.save(recordings_dir)?;

                tracing::info!("Transcribed: {} ({:.1}s)", transcript.file, transcript.duration_secs);
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

        let cpu_usage: f32 = sys.cpus().iter()
            .map(|c| c.cpu_usage())
            .sum::<f32>() / sys.cpus().len() as f32;

        if cpu_usage < idle_config.cpu_threshold_percent {
            tracing::info!("System idle (CPU: {:.1}%), processing...", cpu_usage);
            run_transcribe_oneshot(config, backend_override)?;
        } else {
            tracing::debug!("System busy (CPU: {:.1}%), waiting...", cpu_usage);
        }

        std::thread::sleep(std::time::Duration::from_secs(idle_config.idle_check_interval_secs));
    }
}
```

**Step 6: Wire into `src/main.rs`**

```rust
Commands::Transcribe { watch, backend } => {
    if watch {
        crate::transcribe::runner::run_transcribe_watch(&config, backend.as_deref())
    } else {
        crate::transcribe::runner::run_transcribe_oneshot(&config, backend.as_deref())
    }
}
```

**Step 7: Write tests for state tracking**

```rust
// src/transcribe/state.rs
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_state_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let mut state = TranscriptionState::default();
        state.mark_transcribed("2026-02-16/mic_14-30-00.wav".to_string());
        state.save(tmp.path()).unwrap();

        let loaded = TranscriptionState::load(tmp.path()).unwrap();
        assert!(loaded.is_transcribed("2026-02-16/mic_14-30-00.wav"));
        assert!(!loaded.is_transcribed("2026-02-16/teams_14-30-00.wav"));
    }

    #[test]
    fn test_empty_state_from_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let state = TranscriptionState::load(tmp.path()).unwrap();
        assert!(state.transcribed_files.is_empty());
    }
}
```

**Step 8: Compile and run tests**

```bash
cargo build
cargo test transcribe
```

**Step 9: Commit**

```bash
git add -A
git commit -m "feat: pluggable transcription pipeline with local Whisper and Azure OpenAI backends"
```

---

## Task 9: System Tray UI

**Files:**
- Create: `src/tray.rs`
- Modify: `src/recorder.rs`

**Step 1: Implement the system tray**

```rust
// src/tray.rs
use tray_icon::{TrayIconBuilder, Icon, TrayIconEvent};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::Result;

pub enum TrayAction {
    Pause,
    Resume,
    OpenFolder,
    OpenSettings,
    Quit,
}

pub fn run_tray(
    recordings_dir: std::path::PathBuf,
    config_path: Option<std::path::PathBuf>,
    shutdown: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) -> Result<()> {
    // Build menu
    let menu = Menu::new();
    let status_item = MenuItem::new("Status: Recording", false, None);
    let pause_item = MenuItem::new("Pause", true, None);
    let resume_item = MenuItem::new("Resume", true, None);
    let open_folder_item = MenuItem::new("Open Recordings", true, None);
    let settings_item = MenuItem::new("Settings", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    menu.append(&status_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&pause_item)?;
    menu.append(&resume_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&open_folder_item)?;
    menu.append(&settings_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;

    // Create a simple icon (16x16 red/green indicator)
    // In production, embed a proper .ico resource
    let icon = Icon::from_rgba(vec![255, 0, 0, 255].repeat(16 * 16), 16, 16)?;

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("deskmic - Recording")
        .with_icon(icon)
        .build()?;

    // Event loop — this must run on a thread with a Win32 message pump
    // On Windows, use a minimal message loop
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Process menu events
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == quit_item.id() {
                shutdown.store(true, Ordering::Relaxed);
                break;
            } else if event.id == pause_item.id() {
                paused.store(true, Ordering::Relaxed);
                status_item.set_text("Status: Paused");
            } else if event.id == resume_item.id() {
                paused.store(false, Ordering::Relaxed);
                status_item.set_text("Status: Recording");
            } else if event.id == open_folder_item.id() {
                let _ = std::process::Command::new("explorer")
                    .arg(&recordings_dir)
                    .spawn();
            } else if event.id == settings_item.id() {
                if let Some(ref path) = config_path {
                    let _ = std::process::Command::new("notepad")
                        .arg(path)
                        .spawn();
                }
            }
        }

        // Pump Win32 messages
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::*;
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    Ok(())
}
```

> **Note to implementer:** The Win32 message pump requires the `windows` crate with `Win32_UI_WindowsAndMessaging` feature. Add to Cargo.toml:
> ```toml
> [dependencies.windows]
> version = "0.61"
> features = ["Win32_UI_WindowsAndMessaging"]
> ```
> The exact tray-icon event handling may need adjustment. Consult the tray-icon examples.

**Step 2: Integrate tray into recorder**

In `src/recorder.rs`, spawn the tray on the main thread (it needs the Win32 message pump) and move the recording orchestration to a spawned thread. Or run tray on a dedicated thread with its own message loop.

**Step 3: Verify compilation**

```bash
cargo build
```

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: system tray UI with pause/resume and status"
```

---

## Task 10: Install/Uninstall & Status Commands

**Files:**
- Create: `src/commands.rs`
- Modify: `src/main.rs`

**Step 1: Implement install/uninstall (Startup folder)**

```rust
// src/commands.rs
use std::path::PathBuf;
use anyhow::Result;

/// Add a shortcut to the Windows Startup folder.
pub fn install_startup() -> Result<()> {
    let startup_dir = dirs::config_dir()
        .map(|d| d.join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("Startup"))
        .ok_or_else(|| anyhow::anyhow!("Could not find Startup folder"))?;

    let exe_path = std::env::current_exe()?;
    let shortcut_path = startup_dir.join("deskmic.lnk");

    // Use PowerShell to create .lnk shortcut
    let ps_script = format!(
        "$WshShell = New-Object -ComObject WScript.Shell; \
         $Shortcut = $WshShell.CreateShortcut('{}'); \
         $Shortcut.TargetPath = '{}'; \
         $Shortcut.WorkingDirectory = '{}'; \
         $Shortcut.Save()",
        shortcut_path.display(),
        exe_path.display(),
        exe_path.parent().unwrap().display(),
    );

    let output = std::process::Command::new("powershell")
        .args(["-Command", &ps_script])
        .output()?;

    if output.status.success() {
        println!("Installed to startup: {}", shortcut_path.display());
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create shortcut: {}", err)
    }
}

/// Remove the shortcut from the Startup folder.
pub fn uninstall_startup() -> Result<()> {
    let startup_dir = dirs::config_dir()
        .map(|d| d.join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("Startup"))
        .ok_or_else(|| anyhow::anyhow!("Could not find Startup folder"))?;

    let shortcut_path = startup_dir.join("deskmic.lnk");

    if shortcut_path.exists() {
        std::fs::remove_file(&shortcut_path)?;
        println!("Removed from startup: {}", shortcut_path.display());
    } else {
        println!("Not installed in startup");
    }

    Ok(())
}

/// Show current recording status.
pub fn show_status(recordings_dir: &std::path::Path) -> Result<()> {
    let (file_count, total_bytes) = crate::storage::get_storage_stats(recordings_dir)?;
    let total_mb = total_bytes as f64 / 1_048_576.0;

    println!("deskmic status:");
    println!("  Recordings dir: {}", recordings_dir.display());
    println!("  Total files:    {}", file_count);
    println!("  Total size:     {:.1} MB", total_mb);

    // Check if deskmic is already running (simple: check for lock file or process)
    // For now, just show file stats
    Ok(())
}
```

**Step 2: Wire into `src/main.rs`**

```rust
Commands::Install => crate::commands::install_startup(),
Commands::Uninstall => crate::commands::uninstall_startup(),
Commands::Status => crate::commands::show_status(&config.output.directory),
```

**Step 3: Test manually**

```bash
cargo run -- install
cargo run -- status
cargo run -- uninstall
```

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: install/uninstall startup commands and status reporting"
```

---

## Task 11: Resilience — Sleep/Wake & Device Recovery

**Files:**
- Modify: `src/audio/capture.rs`
- Modify: `src/audio/teams_capture.rs`
- Modify: `src/recorder.rs`

**Step 1: Add device invalidation detection to capture modules**

In both `MicCapture::read_frames()` and `TeamsCapture::read_frames()`, detect WASAPI errors that indicate the device was invalidated (sleep/wake, device change). Return a specific error variant that the pipeline can catch.

```rust
// src/audio/capture.rs — add error type
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CaptureError {
    #[error("Device invalidated (sleep/wake or device change)")]
    DeviceInvalidated,
    #[error("Capture error: {0}")]
    Other(#[from] anyhow::Error),
}
```

**Step 2: Update the outer recovery loop in `recorder.rs`**

The mic pipeline thread already has a recovery loop (Task 6). Ensure it:
- Clears the ring buffer on re-init
- Logs the recovery event
- Backs off exponentially on repeated failures (2s, 4s, 8s, max 30s)

**Step 3: Test by simulating device removal**

Manual test: start recording, disconnect USB headset, verify re-init, reconnect, verify capture resumes.

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: sleep/wake resilience and device recovery"
```

---

## Task 12: End-to-End Integration Test

**Files:**
- Create: `tests/integration.rs`

**Step 1: Write an integration test for the full pipeline**

This test creates a mock audio stream, feeds it through the pipeline, and verifies WAV files are created.

```rust
// tests/integration.rs
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn test_file_writer_creates_wav_on_speech() {
    use deskmic::audio::pipeline::AudioMessage;
    use deskmic::audio::file_writer::run_file_writer;
    use deskmic::config::OutputConfig;

    let tmp = TempDir::new().unwrap();
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
    sender.send(AudioMessage::SpeechStart {
        source: "mic".to_string(),
        samples: samples.clone(),
        sample_rate: 16000,
    }).unwrap();

    sender.send(AudioMessage::SpeechContinue {
        source: "mic".to_string(),
        samples: samples.clone(),
    }).unwrap();

    sender.send(AudioMessage::SpeechEnd {
        source: "mic".to_string(),
    }).unwrap();

    // Close channel to stop writer
    drop(sender);
    writer.join().unwrap();

    // Verify a WAV file was created
    let date_dir = tmp.path().join(chrono::Local::now().format("%Y-%m-%d").to_string());
    assert!(date_dir.exists(), "Date directory should exist");

    let files: Vec<_> = std::fs::read_dir(&date_dir).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "wav").unwrap_or(false))
        .collect();
    assert_eq!(files.len(), 1, "Should have exactly one WAV file");

    // Verify the WAV is valid
    let wav_path = files[0].path();
    let reader = hound::WavReader::open(&wav_path).unwrap();
    assert_eq!(reader.spec().sample_rate, 16000);
    assert_eq!(reader.spec().channels, 1);
    assert_eq!(reader.spec().bits_per_sample, 16);
}
```

**Step 2: Run integration tests**

```bash
cargo test --test integration
```

**Step 3: Commit**

```bash
git add -A
git commit -m "test: end-to-end integration test for recording pipeline"
```

---

## Task 13: README & Release Setup

**Files:**
- Create: `README.md`
- Create: `.github/workflows/build.yml` (optional CI)
- Create: `.gitignore`

**Step 1: Write README.md**

Include:
- What deskmic is (one paragraph)
- Quick start (download binary, run `deskmic`)
- Configuration reference
- CLI reference
- Transcription setup (model download, Azure config)
- Building from source
- Legal notice about recording consent
- License (MIT)

**Step 2: Write `.gitignore`**

```
/target
*.wav
*.bin
.deskmic-state.json
```

**Step 3: Commit**

```bash
git add -A
git commit -m "docs: README, gitignore, and project documentation"
```

---

## Implementation Order & Dependencies

```
Task 1: Scaffold & Config ──────────────────────┐
                                                 │
Task 2: Ring Buffer & VAD ───────────────────────┤
                                                 │
Task 3: Mic Capture Pipeline ────────────────────┤
                                                 │
Task 4: Teams Process Capture ───────────────────┤
                                                 │
Task 5: File Writer ─────────────────────────────┤
                                                 │
Task 6: Recording Orchestrator ──────────────────┤ (depends on 1-5)
                                                 │
Task 7: Storage Cleanup ─────────────────────────┤ (depends on 1)
                                                 │
Task 8: Transcription Pipeline ──────────────────┤ (depends on 1, 5)
                                                 │
Task 9: System Tray ─────────────────────────────┤ (depends on 6)
                                                 │
Task 10: Install/Status Commands ────────────────┤ (depends on 1, 7)
                                                 │
Task 11: Resilience ─────────────────────────────┤ (depends on 3, 4, 6)
                                                 │
Task 12: Integration Tests ──────────────────────┤ (depends on 1-6)
                                                 │
Task 13: README & Release ───────────────────────┘ (last)
```

**Parallelizable tasks:**
- Tasks 2, 3, 4, 5 can be worked on in parallel after Task 1
- Tasks 7 and 8 can be worked on in parallel after Task 1
- Task 9, 10, 11 after Task 6

**Estimated total effort:** ~3-4 days for an experienced Rust developer familiar with Windows audio APIs. The WASAPI integration (Tasks 3-4) is the highest-risk area — the `wasapi` crate's exact API may differ from the pseudocode above and will need adjustment.
