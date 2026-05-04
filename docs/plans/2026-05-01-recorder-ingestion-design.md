# Sony UX570 Recorder Ingestion — Design

**Date:** 2026-05-01
**Status:** Approved (brainstorm)
**Scope:** v1 ingestion of audio files from the Sony ICD-UX570 voice recorder into the existing deskmic transcription + storage pipeline.

## Goal

When the user plugs in their Sony UX570 voice recorder, deskmic should detect it, copy off any new MP3 recordings, transcribe and diarize them, write them into the same per-day JSONL transcripts as the live mic and Teams flows, and eventually clean up the device. No new mental model: a recorder file becomes the same kind of artifact as a Teams or mic recording, just with a different `source` and a different arrival path.

## Device facts (verified on the user's unit)

- Volume label: `IC RECORDER` (stable across reinsertion; drive letter is not).
- File location on device: `\REC_FILE\FOLDER01\`.
- Format: MP3 (this unit's current mode).
- Filename pattern: `YYMMDD_HHMM.mp3` (start time of the recording, recorder clock).
- Long recordings auto-split into `YYMMDD_HHMM.mp3`, `YYMMDD_HHMM_01.mp3`, `_02`, ...
- File `LastWriteTime` ≈ end of recording.
- Other on-device folders (`MUSIC\`, `LICENSE\`, `FOR_WINDOWS\`, `capability_02.xml`) are out of scope and never touched.
- Card capacity ~3.4 GB free; long meetings can hit ~220 MB.

## Decisions (from brainstorm)

| # | Decision | Choice |
|---|---|---|
| 1 | Trigger | Scheduled poll (default ~15 min) plus manual `deskmic ingest-recorder` |
| 2 | Retention on device | Leave for 7 days as a rolling backup, then auto-delete after successful transcription |
| 3 | Code location | New module + subcommand inside the existing deskmic binary, matching existing patterns |
| 4 | Speaker labels | Reuse the Teams pyannote diarization path: `Speaker 1`, `Speaker 2`, ... (no voice-ID for "You" in v1) |
| 5 | MP3 handling | Decode MP3 → 16 kHz mono WAV at ingest using `symphonia`; downstream pipeline stays uniform |
| 6 | Long files | VAD-trim silence, then chunk into ~10-min segments on silence boundaries |
| 7 | Blob sync | Transcripts only; raw audio is **not** uploaded |

## Architecture

```
[scheduled task / manual run]
        │
        ▼
deskmic ingest-recorder
        │
        ├─ detect "IC RECORDER" volume by label
        ├─ enumerate D:\REC_FILE\FOLDER01\*.mp3
        ├─ for each file not already in `recorder_ingest` registry:
        │     ├─ copy → recordings\YYYY-MM-DD\recorder_HHMMSS.mp3.tmp
        │     ├─ atomic rename → .mp3
        │     ├─ symphonia decode → 16 kHz mono f32 PCM
        │     ├─ VAD-trim silence (existing voice_activity_detector)
        │     ├─ chunk on silence boundaries → recorder_HHMMSS_chunkNN.wav
        │     │     plus recorder_HHMMSS_chunkNN.json sidecar (base_offset_secs, recording_id)
        │     └─ insert row in `recorder_ingest`
        │
        └─ device cleanup pass:
              for each row where ingested_at < now-7d AND transcribed_at is set:
                  if device file still exists with matching size+mtime: delete it
        ▼
[existing transcribe --watch picks up the new WAVs]
        │
        ├─ source = "recorder" (inferred from filename prefix)
        ├─ run pyannote diarization (same path as Teams)
        ├─ apply chunk base_offset_secs to per-segment start_secs/end_secs
        ├─ append rows to transcripts\YYYY-MM-DD.jsonl with
        │     source="recorder", speaker="Speaker N",
        │     recording_id=<original device filename>
        └─ when all chunks of a recording are written, stamp transcribed_at on the registry row
        ▼
[existing 2-hour blob sync uploads DB + transcripts JSONL as today]
```

## Components

### `recorder_ingest` module (new, in `src/`)

Submodules:

- `detect.rs` — locate the volume by label using `GetVolumeInformationW` (we already depend on the `windows` crate). Returns `Option<PathBuf>` for the device root.
- `registry.rs` — SQLite access for the new table; insert/update helpers; `mark_transcribed`.
- `copy.rs` — `.tmp → atomic rename` copy from device to local recordings dir, into the date folder derived from the parsed filename.
- `decode.rs` — MP3 → 16 kHz mono f32 PCM via `symphonia`.
- `chunk.rs` — VAD-trim and silence-boundary chunking; emits WAV via `hound` and a `.json` sidecar containing `{ recording_id, chunk_index, base_offset_secs }`.
- `cleanup.rs` — 7-day device retention pass.
- `mod.rs` — orchestrates the run; respects `--dry-run` and `--retry-failed`.

### Subcommand wiring

In `cli.rs`:

```
IngestRecorder {
    #[arg(long)] dry_run: bool,
    #[arg(long)] retry_failed: bool,
}
```

In `main.rs`: acquire `Global\deskmic-ingest-recorder` mutex (mirrors the existing single-instance pattern for `Record` and `Transcribe --watch`).

### Touchpoints in existing modules

These are the **only** edits to existing code:

- `src/transcribe/runner.rs` — when handling a file whose name starts with `recorder_`, set `source = "recorder"`, look for a `.json` sidecar, and if present add `base_offset_secs` to each segment's `start_secs`/`end_secs` and copy `recording_id` onto each transcript row.
- `src/transcribe/runner.rs` — extend the speaker-labeling branch: `mic → "You"`, `teams → diarize`, `recorder → diarize`, else → `"Others"`.
- `src/transcribe/runner.rs` — after the last chunk for a `recording_id` is written to JSONL, call `recorder_ingest::registry::mark_transcribed(recording_id)`.
- `src/storage.rs` — schema migration adds `recorder_ingest` table.
- `src/config.rs` — add `[recorder]` section.

No changes are needed in: blob sync, audio capture, summarization, search, tray, monitoring, watchdog.

## Data

### `recorder_ingest` table (new)

```sql
CREATE TABLE IF NOT EXISTS recorder_ingest (
  device_filename  TEXT    NOT NULL,
  device_size      INTEGER NOT NULL,
  device_mtime     INTEGER NOT NULL,
  local_path       TEXT    NOT NULL,
  start_ts         INTEGER NOT NULL,
  ingested_at      INTEGER NOT NULL,
  transcribed_at   INTEGER,
  status           TEXT    NOT NULL DEFAULT 'ok',  -- 'ok' | 'failed'
  error_message    TEXT,
  PRIMARY KEY (device_filename, device_size, device_mtime)
);
```

Composite primary key handles the rare case where the recorder reuses a filename after a factory reset: differing size or mtime → treated as a new recording.

### Chunk sidecar JSON

```json
{
  "recording_id": "260429_0909.mp3",
  "chunk_index": 3,
  "base_offset_secs": 1800.0
}
```

### Transcript JSONL row (additions)

Existing rows already carry `timestamp`, `source`, `duration_secs`, `file`, `text`, `speaker`, `start_secs`, `end_secs`. For recorder rows we additionally write:

- `source: "recorder"`
- `recording_id: "260429_0909.mp3"` — the original device filename, so a future cross-chunk re-clustering pass has the grouping it needs.

`speaker` is `"Speaker 1"` / `"Speaker 2"` / ... per chunk. Speaker IDs are **not stable across chunks** — see "Known limitations" below.

## Configuration

Add to the existing TOML:

```toml
[recorder]
enabled = true
volume_label = "IC RECORDER"
device_subpath = "REC_FILE/FOLDER01"
device_retention_days = 7
chunk_target_minutes = 10
vad_silence_hangover_ms = 500
poll_interval_minutes = 15   # consumed by the installer when registering the scheduled task
```

## Scheduled task

Mirrors the existing `deskmic-index-and-sync` and `deskmic-watchdog` tasks. Registered by an extension to the existing `install` flow (or a one-time PowerShell snippet, matching how the watchdog was created): runs `deskmic.exe ingest-recorder` every 15 minutes, hidden, allow-on-battery, no-network-required.

## Error handling

| Condition | Behavior |
|---|---|
| Recorder not connected | Info log "recorder not connected", exit 0. Normal case. |
| `REC_FILE\FOLDER01\` missing/empty | Info log, exit 0. |
| Another `ingest-recorder` already running | Mutex lost → exit 0 silently. |
| MP3 decode fails (corrupt file) | Mark registry row `status='failed'` with error message; don't retry on subsequent runs unless `--retry-failed` is passed; do **not** delete from device. |
| Recorder unplugged mid-copy | The atomic-rename pattern leaves at most a `.tmp` behind; next run cleans up stray `.tmp` files older than 1 hour before re-enumerating. |
| Disk full on local | Fail the copy with a clear error log; abort the run; device file untouched. |
| Chunk transcription fails | Existing transcribe pipeline handles per-file failure. `transcribed_at` stays null → device cleanup naturally won't delete the source until transcription eventually succeeds. |
| Filename doesn't match `YYMMDD_HHMM(_NN)?.mp3` | Skip with a warning; never delete from device. |

## Testing

**Unit:**
- Filename parser: `260429_0909.mp3` → (2026-04-29, 09:09, chunk None); `260429_0909_01.mp3` → (2026-04-29, 09:09, split=1); malformed names rejected.
- Symphonia decode produces expected sample count and rate on a fixture MP3.
- VAD trim drops a known silent region and preserves a known speech region (sample-accurate within tolerance).
- Chunker splits at silence, never mid-utterance, with the configured target length.
- Sidecar JSON round-trips.
- Registry: insert + lookup by composite key; "different size or mtime" treated as new row.

**Integration:**
- A temp directory simulates the device tree (`<tmp>\REC_FILE\FOLDER01\<fixture>.mp3`); the volume-detect layer is stubbed to return that path. End-to-end run produces:
  - WAV chunks + sidecars in the expected `recordings\YYYY-MM-DD\` folder
  - A registry row with the correct composite key
  - On a second run with the same fixture: no duplicate work
  - On a third run that mocks "transcription complete" by stamping `transcribed_at` and rewinds clock 8 days: device file is removed
- A second integration test verifies that a corrupt MP3 fixture marks `status='failed'` and is not retried on the next normal run, but is retried under `--retry-failed`.

**Manual smoke test (post-implementation, with the real device):**
1. Plug in the recorder.
2. Run `deskmic ingest-recorder --dry-run` — verify the log lists the expected files without writing anything.
3. Run `deskmic ingest-recorder` — verify WAV chunks land in `recordings\YYYY-MM-DD\`, registry rows appear, and the transcribe watcher produces JSONL rows with `source="recorder"`, `recording_id=...`, `speaker="Speaker N"`.
4. Wait for the 2-hour blob sync; confirm transcripts reach blob.
5. Manually backdate `ingested_at` on a row to 8 days ago; rerun; confirm the device file is removed.

## Known limitations (accepted for v1)

- **Speaker IDs not stable across chunks** of the same recording. `Speaker 1` in chunk 0 may be a different person than `Speaker 1` in chunk 1. Mitigated by writing `recording_id` on every row so a future global re-clustering pass has the grouping it needs.
- **No "You" identification.** All voices in recorder audio are `Speaker N`; the user is one of them but not labeled. Voice-ID is a separate, deferred feature.
- **Split files (`_01`, `_02`) are ingested as separate recordings.** No stitching.
- **WAV-mode recordings are not specifically tested.** Symphonia handles WAV trivially and the rest of the pipeline is format-agnostic post-decode, but this is not exercised by tests in v1.
- **Audio is not backed up to blob.** Once a recording rolls off both the device (7 days) and local disk (existing `recordings/` retention), only the transcript remains.
- **Detection is poll-based, not USB-event-driven.** Up to ~15 minutes of latency between plug-in and ingest start.

## Out of scope

- Voice-ID enrollment / "You" labeling
- Cross-chunk speaker re-clustering
- Split-file stitching
- USB plug-event detection
- Uploading raw audio to Azure Blob
- Other recorder brands / models

## New dependencies

- `symphonia` (with `mp3` feature) — pure Rust, MIT, decodes MP3 → PCM. No system libraries; no ffmpeg.

No other new crates; everything else (`hound`, `voice_activity_detector`, `rusqlite`, `windows`, `chrono`, `tracing`, `clap`, `anyhow`) is already in `Cargo.toml`.
