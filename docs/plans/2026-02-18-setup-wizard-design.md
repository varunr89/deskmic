# deskmic setup wizard

## Problem

Setting up deskmic requires multiple manual steps: downloading a Whisper model, creating a config file, entering Azure credentials, creating Task Scheduler entries, and registering for auto-start. A non-technical user cannot do this.

## Solution

A `deskmic setup` CLI wizard that walks through everything interactively in 4 steps.

## Flow

```
$ deskmic setup

  [1/4] Whisper Model
  Choose a transcription model:
    1. tiny.en  (~75MB)  - Fastest, lower accuracy
    2. base.en  (~142MB) - Good balance (recommended)
    3. small.en (~466MB) - Better accuracy, slower
  > 2
  Downloading ggml-base.en.bin... [########------] 67% (95/142 MB)

  [2/4] Configuration
  Config written to C:\Users\...\deskmic.toml

  [3/4] Email Summaries (optional)
  Would you like daily email summaries? (requires Azure OpenAI + ACS)
    1. Yes
    2. No, skip
  > 1
  [prompts for: endpoint, api key, deployment, ACS endpoint, ACS key, sender, recipient]
  Scheduled tasks created (daily 7AM, weekly Monday 7AM).

  [4/4] Auto-Start
  Add deskmic to Windows startup? (Y/n)
  > y

  Setup complete!
```

## Architecture

- Single new file: `src/setup.rs`
- New CLI subcommand: `deskmic setup`
- Reuses: `Config::generate_default_commented()`, `commands::install_startup()`
- Model download: `reqwest::blocking` GET from Hugging Face with progress via content-length
- Config update: write default config, then string-replace summarization values
- Task Scheduler: shell out to `schtasks.exe`
- No new crate dependencies

## Re-run behavior

- Model exists: ask to re-download (default: skip)
- Config exists: ask to overwrite (default: skip)
- Scheduled tasks: silently overwrite
- Startup shortcut: silently overwrite

## Error handling

- Each step independent; failures don't abort subsequent steps
- Download failure: print manual download URL
- Task Scheduler failure: warn but continue
- Input validation: URLs must start with https://, emails must contain @
