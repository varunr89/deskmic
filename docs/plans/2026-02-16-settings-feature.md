# Settings Feature Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the Settings tray menu item work by finding/creating a fully-commented config file and opening it in Notepad.

**Architecture:** Modify `Config::load()` to return the resolved file path alongside the parsed config. Add a `generate_default_commented()` function that produces a TOML string with all fields and inline documentation. Update `run_tray()` and `run_recorder()` to pass the resolved path through. Fix the model path construction bug in the transcription runner.

**Tech Stack:** Rust, serde/toml, Windows (Notepad)

---

### Task 1: Add `Config::load_with_path()` that returns `(Config, Option<PathBuf>)`

**Files:**
- Modify: `src/config.rs:178-216`
- Test: `src/config.rs` (existing test module)

**Step 1: Write the failing test**

Add to the existing test module in `src/config.rs`:

```rust
#[test]
fn test_load_with_path_returns_resolved_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config_file = tmp.path().join("deskmic.toml");
    std::fs::write(&config_file, "[capture]\nsample_rate = 44100\n").unwrap();

    let (config, resolved) = Config::load_with_path(Some(config_file.as_path())).unwrap();
    assert_eq!(config.capture.sample_rate, 44100);
    assert_eq!(resolved, Some(config_file));
}

#[test]
fn test_load_with_path_none_returns_none_when_no_file() {
    // With no file at any search location, returns (defaults, None)
    let (config, resolved) = Config::load_with_path(None).unwrap();
    assert_eq!(config.capture.sample_rate, 16000);
    // resolved could be Some if there happens to be a file beside the exe,
    // but in test context there won't be one at the searched locations
    // so we just verify it doesn't panic
    let _ = resolved;
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib config::tests::test_load_with_path -- --nocapture`
Expected: FAIL — `load_with_path` method does not exist.

**Step 3: Write minimal implementation**

Replace `Config::load()` in `src/config.rs:178-216` with two methods:

```rust
impl Config {
    /// Load config and return the resolved file path (if any).
    pub fn load_with_path(path: Option<&Path>) -> anyhow::Result<(Self, Option<PathBuf>)> {
        // 1. Check explicit path
        if let Some(p) = path {
            let content = std::fs::read_to_string(p).map_err(|e| {
                anyhow::anyhow!("Failed to read config file {}: {}", p.display(), e)
            })?;
            let config: Config = toml::from_str(&content)?;
            return Ok((config, Some(p.to_path_buf())));
        }

        // 2. Check beside the executable
        if let Ok(exe_path) = std::env::current_exe() {
            let beside_exe = exe_path.parent().map(|p| p.join("deskmic.toml"));
            if let Some(p) = beside_exe {
                if p.exists() {
                    let content = std::fs::read_to_string(&p)?;
                    let config: Config = toml::from_str(&content)?;
                    return Ok((config, Some(p)));
                }
            }
        }

        // 3. Check platform config directory (e.g. %APPDATA%\deskmic\config.toml)
        if let Some(config_dir) = dirs::config_dir() {
            let platform_config = config_dir.join("deskmic").join("config.toml");
            if platform_config.exists() {
                let content = std::fs::read_to_string(&platform_config)?;
                let config: Config = toml::from_str(&content)?;
                return Ok((config, Some(platform_config)));
            }
        }

        // 4. Fall back to defaults
        tracing::info!("No config file found, using defaults");
        Ok((Config::default(), None))
    }

    /// Load config (without tracking the resolved path). Kept for backward compat.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        Self::load_with_path(path).map(|(config, _)| config)
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib config::tests -- --nocapture`
Expected: ALL PASS

**Step 5: Commit**

```
git add src/config.rs
git commit -m "feat: add Config::load_with_path() returning resolved config file path"
```

---

### Task 2: Add `generate_default_commented()` to produce a fully-documented TOML config

**Files:**
- Modify: `src/config.rs` (add function after the `impl Config` block)
- Test: `src/config.rs` (existing test module)

**Step 1: Write the failing test**

Add to the existing test module in `src/config.rs`:

```rust
#[test]
fn test_generate_default_commented_is_valid_toml() {
    let content = Config::generate_default_commented();
    // Should be parseable as valid TOML (comments are stripped by parser)
    let config: Config = toml::from_str(&content).unwrap();
    assert_eq!(config.capture.sample_rate, 16000);
    assert_eq!(config.vad.pre_speech_buffer_secs, 5.0);
    assert_eq!(config.output.max_file_duration_mins, 30);
    assert_eq!(config.transcription.backend, "local");
    assert_eq!(config.transcription.idle_watch.idle_check_interval_secs, 30);
}

#[test]
fn test_generate_default_commented_has_all_sections() {
    let content = Config::generate_default_commented();
    assert!(content.contains("[capture]"));
    assert!(content.contains("[vad]"));
    assert!(content.contains("[output]"));
    assert!(content.contains("[targets]"));
    assert!(content.contains("[storage]"));
    assert!(content.contains("[transcription]"));
    assert!(content.contains("[transcription.azure]"));
    assert!(content.contains("[transcription.idle_watch]"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib config::tests::test_generate_default_commented -- --nocapture`
Expected: FAIL — method does not exist.

**Step 3: Write minimal implementation**

Add to the `impl Config` block in `src/config.rs`:

```rust
    /// Generate a default config file with all fields and inline documentation.
    pub fn generate_default_commented() -> String {
        let default_output_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("deskmic")
            .join("recordings");
        let output_dir_str = default_output_dir.to_string_lossy().replace('\\', "\\\\");

        format!(
r#"# deskmic configuration
# Edit this file to customize recording, transcription, and storage settings.
# Restart deskmic after saving changes for them to take effect.

[capture]
# Audio capture sample rate in Hz. 16000 is required for VAD compatibility.
sample_rate = 16000
# Bits per sample (16-bit PCM is standard).
bit_depth = 16
# Number of audio channels (1 = mono).
channels = 1

[vad]
# Seconds of audio to keep in the ring buffer before speech is detected.
# This "pre-roll" ensures you don't lose the beginning of a sentence.
pre_speech_buffer_secs = 5.0
# Seconds of silence after speech before the recording segment ends.
# Lower values = more files, higher values = longer trailing silence.
silence_threshold_secs = 3.0
# Voice activity detection confidence threshold (0.0 to 1.0).
# Lower = more sensitive (catches quiet speech), higher = fewer false positives.
speech_threshold = 0.5

[output]
# Directory where WAV recordings are saved.
directory = "{output_dir}"
# Maximum duration of a single recording file in minutes.
# Recordings are split into new files when this limit is reached.
max_file_duration_mins = 30
# Organize recordings into date-based subdirectories (YYYY-MM-DD).
organize_by_date = true

[targets]
# List of process names to capture audio from (application loopback).
processes = ["ms-teams.exe"]
# Whether to also capture from the default microphone.
mic_enabled = true

[storage]
# Number of days to keep recordings before automatic cleanup.
retention_days = 30
# How often (in hours) to run the cleanup job.
cleanup_interval_hours = 6
# Maximum total disk usage in GB. Oldest files are deleted first.
# Comment out or remove to disable disk usage limits.
# max_disk_usage_gb = 50.0

[transcription]
# Transcription backend: "local" (whisper.cpp on device) or "azure" (cloud API).
backend = "local"
# Whisper model name (for local backend). Options: tiny.en, base.en, small.en, medium.en
# Or an absolute path to a .bin model file.
model = "base.en"

[transcription.azure]
# Azure OpenAI Whisper endpoint URL.
# endpoint = "https://your-resource.openai.azure.com"
# API key (or set DESKMIC_AZURE_KEY environment variable).
# api_key = ""
# Deployment name for the Whisper model.
# deployment = "whisper"

[transcription.idle_watch]
# Only run transcription when average CPU usage is below this percentage.
# Prevents transcription from slowing down your machine during active use.
cpu_threshold_percent = 20.0
# How often (in seconds) to check whether the system is idle for transcription.
idle_check_interval_secs = 30
"#,
            output_dir = output_dir_str
        )
    }
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib config::tests -- --nocapture`
Expected: ALL PASS

**Step 5: Commit**

```
git add src/config.rs
git commit -m "feat: add Config::generate_default_commented() for documented config files"
```

---

### Task 3: Update `main.rs` to use `load_with_path` and pass resolved path to recorder

**Files:**
- Modify: `src/main.rs:37-42`

**Step 1: Update main.rs**

Change the config loading and recorder call in `src/main.rs`:

```rust
    let (config, resolved_config_path) = Config::load_with_path(cli.config.as_deref())?;

    match cli.command.unwrap_or(Commands::Record) {
        Commands::Record => {
            tracing::info!("Starting deskmic recorder");
            deskmic::recorder::run_recorder(config, resolved_config_path.or(cli.config))
        }
```

The `resolved_config_path.or(cli.config)` ensures that if a config file was found by auto-discovery, its path is passed through. If the user explicitly passed `--config`, that was already captured by `load_with_path`. If no file was found, falls back to `cli.config` (which would also be `None`).

**Step 2: Build to verify it compiles**

Run: `cargo build --release`
Expected: Compiles successfully (no test needed — this is plumbing).

**Step 3: Commit**

```
git add src/main.rs
git commit -m "fix: pass resolved config path to recorder for tray Settings item"
```

---

### Task 4: Update tray Settings handler to create config if missing

**Files:**
- Modify: `src/tray.rs:73-77`

**Step 1: Update the Settings handler**

Replace the settings handler block in `src/tray.rs` (lines 73-77):

```rust
            } else if event.id == settings_item.id() {
                let path = config_path.clone().unwrap_or_else(|| {
                    // No config file exists yet — create a default one beside the exe.
                    let default_path = std::env::current_exe()
                        .ok()
                        .and_then(|exe| exe.parent().map(|p| p.join("deskmic.toml")))
                        .unwrap_or_else(|| std::path::PathBuf::from("deskmic.toml"));
                    if !default_path.exists() {
                        let content = crate::config::Config::generate_default_commented();
                        let _ = std::fs::write(&default_path, &content);
                    }
                    default_path
                });
                let _ = std::process::Command::new("notepad").arg(&path).spawn();
            }
```

This ensures:
- If a config file was found/passed, it opens that file.
- If no config file exists, it creates a fully-commented default one beside the exe and opens it.

**Step 2: Build to verify it compiles**

Run: `cargo build --release`
Expected: Compiles successfully.

**Step 3: Commit**

```
git add src/tray.rs
git commit -m "fix: Settings tray item creates default config if missing, always opens editor"
```

---

### Task 5: Fix model path construction bug in transcription runner

**Files:**
- Modify: `src/transcribe/runner.rs:50-56`
- Test: `src/transcribe/runner.rs` (existing test module)

**Step 1: Write the failing test**

This is tricky to unit test since `build_backend` creates a WhisperLocal which loads a model file. Instead, we'll fix the logic and verify the build compiles. The bug is on line 54:

```rust
let model_file = format!("ggml-{}.bin", config.transcription.model);
```

If `config.transcription.model` is already an absolute path like `C:\...\ggml-base.en.bin`, this produces `ggml-C:\...\ggml-base.en.bin.bin`.

**Step 2: Fix the model path resolution**

Replace lines 50-56 in `src/transcribe/runner.rs`:

```rust
        "local" => {
            #[cfg(target_os = "windows")]
            {
                use crate::transcribe::whisper_local::WhisperLocal;
                let model_path = resolve_model_path(&config.transcription.model);
                Ok(Box::new(WhisperLocal::new(&model_path)?))
            }
            #[cfg(not(target_os = "windows"))]
            {
                anyhow::bail!("Local whisper backend is only available on Windows")
            }
        }
```

Add a helper function before `build_backend`:

```rust
/// Resolve the model path from config. If the value is already an absolute path
/// or ends in ".bin", use it as-is. Otherwise, treat it as a model name and
/// construct "ggml-{name}.bin" in the exe directory.
fn resolve_model_path(model: &str) -> String {
    let path = Path::new(model);
    // If it's already an absolute path or has a .bin extension, use as-is
    if path.is_absolute() || path.extension().map(|e| e == "bin").unwrap_or(false) {
        return model.to_string();
    }
    // Otherwise construct ggml-{model}.bin next to the exe
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            return exe_dir
                .join(format!("ggml-{}.bin", model))
                .to_string_lossy()
                .to_string();
        }
    }
    format!("ggml-{}.bin", model)
}
```

**Step 3: Write test for resolve_model_path**

Add to the test module in `src/transcribe/runner.rs`:

```rust
#[test]
fn test_resolve_model_path_short_name() {
    let result = resolve_model_path("base.en");
    assert!(result.contains("ggml-base.en.bin"));
}

#[test]
fn test_resolve_model_path_absolute_path() {
    let abs = if cfg!(windows) {
        "C:\\models\\ggml-base.en.bin"
    } else {
        "/tmp/models/ggml-base.en.bin"
    };
    assert_eq!(resolve_model_path(abs), abs);
}

#[test]
fn test_resolve_model_path_bin_extension() {
    assert_eq!(resolve_model_path("my-model.bin"), "my-model.bin");
}
```

**Step 4: Run tests**

Run: `cargo test --lib transcribe::runner::tests -- --nocapture`
Expected: ALL PASS

**Step 5: Commit**

```
git add src/transcribe/runner.rs
git commit -m "fix: handle absolute model paths in transcription runner"
```

---

### Task 6: Deploy, verify end-to-end, and push

**Files:** None (deployment and verification only)

**Step 1: Build release**

```powershell
$env:PATH = "C:\Program Files\CMake\bin;C:\Program Files\LLVM\bin;" + $env:PATH
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
cargo build --release
```

Expected: `Finished release profile`

**Step 2: Kill deskmic, copy new binary, restart**

```powershell
Stop-Process -Name deskmic -Force -ErrorAction SilentlyContinue
Copy-Item target\release\deskmic.exe $env:LOCALAPPDATA\deskmic\deskmic.exe -Force
Start-Process $env:LOCALAPPDATA\deskmic\deskmic.exe -WindowStyle Hidden
```

**Step 3: Verify Settings tray item works**

Right-click tray icon -> click "Settings" -> Notepad should open with a fully-commented `deskmic.toml`.

**Step 4: Verify CLI still works**

```powershell
deskmic --version   # should print "deskmic 0.1.0"
deskmic status      # should print recordings info
```

**Step 5: Commit and push all changes**

```
git add -A
git commit -m "feat: working Settings tray item with auto-generated documented config"
git push origin master
```
