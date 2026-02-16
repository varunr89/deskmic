# deskmic

Always-on Windows 11 audio recorder that captures microphone and Microsoft Teams process audio using WASAPI and Application Loopback Capture. Uses Silero VAD with a ring buffer to only save speech segments as WAV files. Includes an async batch transcription pipeline with pluggable backends (local whisper-rs and Azure OpenAI Whisper API). Lightweight, open-source, single portable `.exe`.

When recording, deskmic runs a system tray icon with controls to pause/resume recording, open the recordings folder, and quit. It automatically recovers from audio device changes and sleep/wake cycles with exponential backoff.

## Quick start

Download the latest `deskmic.exe` binary and run it:

```
deskmic
```

By default, deskmic records microphone + Teams audio to `%LOCALAPPDATA%\deskmic\recordings\`, organized by date. A default config is used if no config file exists.

To run on startup:

```
deskmic install
```

## Configuration

deskmic looks for a config file in this order:

1. Path passed via `--config <path>`
2. `deskmic.toml` next to the executable
3. `%APPDATA%\deskmic\config.toml`
4. Built-in defaults

Example `deskmic.toml` with all options and defaults:

```toml
[capture]
sample_rate = 16000
bit_depth = 16
channels = 1

[vad]
speech_threshold = 0.5
pre_speech_buffer_secs = 5.0
silence_threshold_secs = 3.0

[output]
directory = "C:\\Users\\YourName\\AppData\\Local\\deskmic\\recordings"
max_file_duration_mins = 30
organize_by_date = true

[targets]
processes = ["ms-teams.exe"]
mic_enabled = true

[storage]
retention_days = 30
cleanup_interval_hours = 6
# max_disk_usage_gb = 50.0  # optional, no limit by default

[transcription]
backend = "local"       # "local" or "azure"
model = "base.en"       # whisper model name or path

[transcription.azure]
endpoint = ""
api_key = ""
deployment = ""

[transcription.idle_watch]
cpu_threshold_percent = 20.0
idle_check_interval_secs = 30
```

## CLI reference

```
deskmic [OPTIONS] [COMMAND]
```

**Global options:**

| Option | Description |
|---|---|
| `-c, --config <path>` | Path to config file |
| `--version` | Print version |
| `-h, --help` | Print help |

**Commands:**

| Command | Description |
|---|---|
| `record` | Start recording (default if no subcommand) |
| `transcribe` | Transcribe pending audio files (one-shot) |
| `transcribe --watch` | Run transcription as idle-aware daemon |
| `transcribe --backend <name>` | Force a specific backend (`local` or `azure`) |
| `install` | Add deskmic to Windows Startup folder |
| `uninstall` | Remove deskmic from Windows Startup folder |
| `status` | Show recording status, disk usage, file count |

Running `deskmic` with no subcommand is equivalent to `deskmic record`.

## Transcription setup

### Local (whisper-rs)

1. Download a Whisper GGML model (e.g. `ggml-base.en.bin`) from [Hugging Face](https://huggingface.co/ggerganov/whisper.cpp/tree/main).
2. Set the model in your config:

```toml
[transcription]
backend = "local"
model = "C:\\path\\to\\ggml-base.en.bin"
```

3. Run `deskmic transcribe` or `deskmic transcribe --watch`.

### Azure OpenAI Whisper

1. Set up an Azure OpenAI resource with a Whisper deployment.
2. Configure your credentials:

```toml
[transcription]
backend = "azure"

[transcription.azure]
endpoint = "https://your-resource.openai.azure.com"
api_key = "your-api-key"
deployment = "whisper-1"
```

The API key can also be set via the `DESKMIC_AZURE_KEY` environment variable instead of putting it in the config file.

## Building from source

Requires:
- Rust toolchain (stable)
- Windows 11 SDK (for WASAPI and Application Loopback Capture APIs)

```
cargo build --release
```

The binary is at `target/release/deskmic.exe`.

**Note:** The project compiles on Linux/WSL2 for development (audio capture is stubbed out), but full functionality requires Windows 11.

## Legal notice

**Recording consent disclaimer:** Users are solely responsible for complying with all applicable local, state, and federal laws regarding the recording of audio conversations. Many jurisdictions have two-party (or all-party) consent laws that require all participants to consent before a conversation may be recorded. Use of this software to record conversations without proper consent may be illegal. The authors of deskmic accept no liability for misuse.

## License

MIT
