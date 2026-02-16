# deskmic - Design Document

**Date:** 2026-02-16
**Status:** Draft

## Overview

`deskmic` is a lightweight, open-source, always-on Windows audio recorder written in Rust. It captures two audio sources independently — your microphone and Microsoft Teams process audio — and saves speech segments to disk as separate timestamped WAV files. It uses a ring buffer with voice activity detection so recordings start slightly before speech begins and stop after silence, ensuring no clipped words and no wasted disk space.

It also includes an async transcription pipeline that batch-processes recorded audio using pluggable backends (local whisper-rs or cloud APIs).

## What It Is NOT

- Not a transcription-first tool — recording reliability is the top priority
- Not cross-platform — Windows 11 only (WASAPI + Application Loopback Capture)
- Not a summarizer — transcripts are structured JSONL output for external tools to consume
- Not an installer-heavy product — single portable `.exe`

## Distribution

- Open-source on GitHub (MIT or Apache-2.0)
- Single binary, no installer required
- `cargo install deskmic` or download release binary

---

## Architecture

### High-Level Dataflow

```
Mic (WASAPI Input) ──→ Ring Buffer ──→ VAD ──→ File Writer ──→ WAV files
                                                     ↑
Teams (App Loopback) → Ring Buffer ──→ VAD ──────────┘
                                                     ↓
                                          Transcription Pipeline
                                                     ↓
                                              JSONL Transcripts
```

### Two Independent Capture Pipelines

Each audio source (mic, Teams) has its own:
- WASAPI capture stream
- 5-second ring buffer
- Silero VAD instance (neural network-based, ONNX model)
- Channel sender to the shared file writer

This ensures if Teams isn't running, only the mic pipeline is active.

### Thread Architecture

```
Main Thread
├── Parse config & CLI args
├── Initialize logging
├── Spawn system tray (UI thread)
│
├── Spawn Mic Pipeline Thread
│   └── Loop: read WASAPI frames → ring buffer → VAD → file writer channel
│
├── Spawn Teams Monitor Thread
│   └── Loop every 5s: scan for ms-teams.exe
│       ├── Found + not capturing → spawn Teams Pipeline Thread
│       └── Gone + capturing → signal Teams Pipeline to stop
│
├── Spawn Teams Pipeline Thread (dynamic, only when Teams is running)
│   └── Loop: read WASAPI loopback frames → ring buffer → VAD → file writer channel
│
├── Spawn File Writer Thread
│   └── Loop: receive audio chunks from channels → write to WAV files
│       ├── Open new file on speech start (with pre-buffer)
│       ├── Close file on silence timeout or max duration
│       └── Update WAV header on close
│
├── Spawn Cleanup Thread
│   └── Loop every cleanup_interval: delete old folders, check disk usage
│
└── Wait for shutdown signal (tray quit or Ctrl+C)
    └── Signal all threads → flush files → exit cleanly
```

### Key Design Choices

- **File Writer is a single dedicated thread** receiving from both pipelines via channels. Avoids two threads fighting over disk I/O and keeps file naming consistent.
- **Teams Pipeline is dynamically spawned/stopped** based on whether Teams is running. No wasted resources when not in a meeting.
- **Each pipeline owns its ring buffer and VAD instance** — no shared mutable state between capture threads.
- **No async runtime (no tokio).** Capture pipelines are real-time audio threads needing deterministic timing. Simple `std::sync::mpsc` channels connect threads.
- **Shutdown is graceful** — on quit signal, each pipeline flushes its ring buffer, the file writer closes open WAV files with correct headers, then the process exits.

---

## Audio Capture

### Microphone

- WASAPI input device in shared mode
- Default system microphone
- 16kHz, 16-bit PCM, mono

### Teams Process Audio

- Windows 11 Application Loopback Capture API (`ActivateAudioInterfaceAsync` with process-level loopback)
- Target process: `ms-teams.exe` (new Teams) or `Teams.exe` (classic)
- Requires process ID — obtained via `sysinfo` crate scanning
- 16kHz, 16-bit PCM, mono (resampled from source if needed)

### Ring Buffer + VAD

- Ring buffer holds ~5 seconds of audio in memory
- Silero VAD (neural network, small ONNX model) classifies 20ms frames as speech/non-speech
- Significantly more accurate than WebRTC VAD — ignores keyboard clicks, fan noise, coughing
- Probability threshold configurable (default 0.5, higher = more conservative)
- On speech detection: flush the ring buffer (pre-speech lead-in) and begin writing to disk
- On sustained silence (configurable, default 3 seconds): close the file
- Maximum file duration safety cap (default 60 minutes) forces file rotation

### Output Format

- Uncompressed WAV (16kHz, 16-bit PCM, mono)
- Separate files per source: `mic_HH-MM-SS.wav`, `teams_HH-MM-SS.wav`
- Organized by date: `recordings/YYYY-MM-DD/`

### File Safety

- WAV headers written at file creation with placeholder length
- On file close, header updated with actual length
- If process crashes mid-file, WAV is still playable (header has wrong length but most players handle this)

---

## Resilience

### Startup

- First run: user launches `deskmic.exe`, it offers to add itself to the Windows Startup folder (`shell:startup`)
- Subsequent boots: launches automatically at login, goes straight to system tray
- No admin privileges required

### Crash Recovery

- Outer loop in `main()` catches panics and audio device errors
- On failure: log the error, wait 2 seconds, re-initialize audio devices, resume
- Any in-progress WAV file gets properly closed before restart

### Sleep/Wake

- WASAPI streams become invalid after sleep
- Capture threads detect `AUDCLNT_E_DEVICE_INVALIDATED`, tear down, and re-initialize
- Ring buffers are cleared on wake (stale pre-sleep audio discarded)
- Recording resumes within ~1 second of wake

### Teams Detection

- On startup and every 5 seconds, scan for `ms-teams.exe` process
- If Teams is running: activate the Teams capture pipeline
- If Teams exits: gracefully close the Teams capture stream and finalize any open WAV
- If Teams starts later: automatically begin capturing

---

## Storage Management

### Folder Structure

```
~/deskmic-recordings/
  2026-02-16/
    mic_09-00-00.wav
    mic_09-30-15.wav
    teams_09-00-00.wav
    teams_10-15-30.wav
  2026-02-15/
    ...
  .deskmic-state.json       ← transcription state tracking
```

### Automatic Cleanup

- On startup and every `cleanup_interval_hours` (default 24), scan recordings directory
- Delete date folders older than `retention_days` (default 30)
- If `max_disk_usage_gb` is set and exceeded, delete oldest folders first until under limit
- Log what was deleted

---

## Transcription Pipeline

### Overview

Async batch transcription that processes recorded WAV files using pluggable backends. Runs separately from the recorder — either on-demand or as an idle-aware daemon.

### Pluggable Backend Trait

```rust
trait TranscriptionBackend {
    fn name(&self) -> &str;
    fn transcribe(&self, audio_path: &Path) -> Result<Transcript>;
}
```

Initial implementations:
- **`WhisperLocal`** — uses `whisper-rs` (Rust bindings to whisper.cpp). Runs on CPU or GPU. No network required.
- **`AzureOpenAI`** — calls Azure OpenAI Whisper API endpoint. Requires API key and endpoint configuration.

Future backends (pluggable trait makes these easy to add):
- Mistral
- Deepgram
- Google Speech-to-Text
- Any other STT API

### State Tracking

A `.deskmic-state.json` file in the recordings directory tracks which files have been transcribed. No database required.

### Output Format

JSONL transcripts stored alongside recordings or in a separate transcripts directory:

```json
{"timestamp": "2026-02-16T14:30:00", "source": "mic", "duration_secs": 45.2, "file": "mic_14-30-00.wav", "text": "..."}
{"timestamp": "2026-02-16T14:30:00", "source": "teams", "duration_secs": 45.2, "file": "teams_14-30-00.wav", "text": "..."}
```

### Execution Modes

- **One-shot:** `deskmic transcribe` — process all pending files, then exit
- **Idle-aware daemon:** `deskmic transcribe --watch` — monitor CPU usage, only process when system is idle (below configurable threshold), yield when load increases

---

## CLI Interface

```
deskmic                              # Start recording (default)
deskmic --config path.toml           # Use custom config
deskmic install                      # Add to Windows Startup folder
deskmic uninstall                    # Remove from Startup folder
deskmic status                       # Show if running, disk usage, file count
deskmic transcribe                   # Process all pending files, then exit
deskmic transcribe --watch           # Idle-aware daemon mode
deskmic transcribe --backend local   # Force local whisper-rs
deskmic transcribe --backend azure   # Force Azure OpenAI API
```

### System Tray (right-click menu)

- Status: Recording / Idle / Teams not detected
- Pause / Resume
- Open recordings folder
- Settings (opens config file in editor)
- Quit

---

## Configuration

File: `deskmic.toml` (beside the exe or `%APPDATA%\deskmic\config.toml`)

```toml
[capture]
sample_rate = 16000
bit_depth = 16
channels = 1

[vad]
pre_speech_buffer_secs = 5
silence_threshold_secs = 3
speech_threshold = 0.5        # Silero VAD probability threshold (0.0-1.0, higher = more conservative)

[output]
directory = "~/deskmic-recordings"
max_file_duration_mins = 60
organize_by_date = true

[targets]
processes = ["ms-teams.exe", "Teams.exe"]
mic_enabled = true

[storage]
retention_days = 30
cleanup_interval_hours = 24
max_disk_usage_gb = 50

[transcription]
backend = "local"             # "local" or "azure"
model = "base"                # whisper model: tiny/base/small/medium/large

[transcription.azure]
endpoint = ""
api_key = ""                  # or env var DESKMIC_AZURE_KEY
deployment = "whisper"

[transcription.idle_watch]
cpu_threshold_percent = 20
idle_check_interval_secs = 300
```

All settings have sensible defaults. The tool works out of the box without editing the config file.

---

## Crate Dependencies

| Concern | Crate | Rationale |
|---|---|---|
| Windows audio (WASAPI) | `wasapi` 0.22 | High-level WASAPI wrapper. Supports per-process Application Loopback Capture on Win11 via `AudioClient::new_application_loopback_client()` |
| VAD | `voice_activity_detector` 0.2 | Silero VAD V5 via bundled ONNX model. Permissive license, actively maintained. Pulls `ort` transitively |
| WAV writing | `hound` 3.5 | Simple, reliable WAV file writer. Supports 16-bit PCM |
| System tray | `tray-icon` 0.21 + `muda` | Maintained by Tauri team. Robust Windows support with Win32 event loop |
| Config | `toml` + `serde` | TOML config parsing |
| CLI | `clap` 4.5 (derive) | Argument parsing with subcommands |
| Logging | `tracing` + `tracing-subscriber` | Structured logging with file rotation |
| Process scanning | `sysinfo` 0.38 | Find Teams process by name/PID |
| Timestamps/paths | `chrono`, `dirs` | Filenames, platform directories |
| Local transcription | `whisper-rs` 0.15 | Rust bindings to whisper.cpp. Requires MSVC toolchain. Model files downloaded separately |
| HTTP (cloud APIs) | `reqwest` (blocking) | Azure OpenAI API calls |
| JSON | `serde_json` | Transcript JSONL output, state tracking |

---

## Out of Scope

These are explicitly not part of deskmic:

- **Weekly summarization / newsletter generation** — consume the JSONL transcripts with any LLM tool
- **Cross-platform support** — Windows 11 only
- **Real-time transcription** — transcription is always async/batch
- **Cloud upload of audio** — recordings stay local
- **Speaker diarization** — mic vs. Teams source separation is the attribution model
- **Video capture** — audio only

---

## Legal Considerations

- Users are responsible for compliance with local recording consent laws
- Washington State requires all-party consent for private conversations
- Recommended: post visible "recording in progress" notices, obtain written consent
- The system tray icon serves as a visual indicator that recording is active
