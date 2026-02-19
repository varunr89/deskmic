# Audio Capture Reliability: Bug Fixes + Monitoring

**Date:** 2026-02-19
**Issues:** #13 (Teams PID thrashing), #14 (mic capture invalidation)

## Problem

Two bugs cause deskmic to silently stop recording:

1. **Teams PID thrashing (#13):** `find_teams_pid()` creates a fresh `sysinfo::System` snapshot each poll and returns whichever PID the `HashMap` iterator yields first. Teams runs multiple `ms-teams.exe` processes. Since iteration order is non-deterministic, each 5-second poll may return a different PID. The monitor sees a "PID change," tears down the pipeline, sets `active_pid = None`, and the cycle repeats — no audio is ever captured.

2. **Mic capture death (#14):** `MicCapture::read_frames()` returns `Ok(None)` for an empty audio buffer (normal WASAPI behavior). But `pipeline.rs` treats `None` as fatal and `break`s the loop. Then `recorder.rs` treats the resulting `Ok(())` as success and `break`s the retry loop too. Net effect: first empty buffer permanently kills mic capture.

Both failures are silent — no user-visible indication that recording has stopped.

## Design

### Bug Fix: Teams PID Stability

**Change in `teams_monitor.rs`:**

Current behavior: if `find_teams_pid()` returns a different PID than `active_pid`, tear down and restart.

New behavior: when `active_pid` is `Some(old_pid)` and `find_teams_pid()` returns `Some(new_pid)` where `old_pid != new_pid`:
- Check if `old_pid` is still alive (process exists)
- If alive: **ignore the new PID**, keep existing capture running
- If dead: tear down pipeline, set `active_pid = None`, let next poll start capture on the new PID

This means once we lock onto a Teams process, we stay on it until it exits. The first PID we find is as good as any — they all share the same audio session.

### Bug Fix: Mic Capture None Handling

**Change in `pipeline.rs`:**

Replace:
```rust
None => {
    tracing::warn!("...");
    break;  // fatal
}
```

With:
```rust
None => continue;  // empty buffer, normal — try again next cycle
```

**Change in `recorder.rs`:**

The outer retry loop already handles `Err(e)` correctly with exponential backoff. The issue is `Ok(()) => break` exits the loop on any clean pipeline shutdown. Change this so `Ok(())` also triggers a retry (the pipeline shouldn't exit cleanly during normal operation):

```rust
Ok(()) => {
    tracing::warn!("Mic pipeline exited unexpectedly, retrying...");
    // fall through to backoff/retry logic
}
```

### New: Pipeline Health Watchdog

**New thread in `recorder.rs`**, spawned alongside existing pipeline threads.

Every 10 seconds:
- Check if mic pipeline thread has exited (`JoinHandle::is_finished()`)
- Check if Teams monitor thread has exited
- Check if transcription child process has exited

If any has exited unexpectedly:
1. Log error
2. Fire Windows toast: "deskmic: Recording stopped unexpectedly — restarting"
3. Self-restart: `Command::new(std::env::current_exe()).spawn()`, then `std::process::exit(0)`

If self-restart fails:
- Toast: "deskmic: Recording failed and could not auto-restart. Please restart manually."
- Continue running (in degraded state) rather than exiting

### New: Recording Gap Timer

**New thread in `recorder.rs`** (or `monitoring.rs`).

Every 60 seconds:
- Look at today's date folder in the recordings directory
- Find the newest `.wav` file by modification time
- If no WAV exists and deskmic has been running > 30 minutes, OR newest WAV is > 30 minutes old: fire toast
- Toast: "deskmic: No audio recorded in the last 30 minutes. Recording may have stopped."
- Suppress duplicate toasts: only fire once per gap (reset when a new recording appears)

Threshold configurable:
```toml
[monitoring]
recording_gap_alert_mins = 30
```

### Windows Toast Notifications

Use the `windows` crate (already a dependency) for `ToastNotification` API. Requires an app user model ID — we can use `deskmic` or register one.

Helper function in new `src/monitoring.rs`:
```rust
pub fn send_toast(title: &str, body: &str) -> Result<()>
```

## Architecture

```
deskmic.exe (record)
+-- Mic pipeline thread          (existing, with None fix)
+-- Teams monitor thread         (existing, with PID stability fix)
+-- Transcription child process  (existing)
+-- Watchdog thread              (NEW: checks thread/process health every 10s)
+-- Gap timer thread             (NEW: checks newest WAV every 60s)
```

## Files Changed

| File | Change |
|------|--------|
| `src/audio/teams_monitor.rs` | PID stability: check if old PID alive before restarting |
| `src/audio/pipeline.rs` | `Ok(None)` -> `continue` instead of `break` |
| `src/recorder.rs` | Fix mic retry on `Ok(())`, spawn watchdog + gap timer threads |
| `src/monitoring.rs` | **New:** `run_watchdog()`, `run_gap_timer()`, `send_toast()` |
| `src/config.rs` | Add `[monitoring]` section with `recording_gap_alert_mins` |
| `src/lib.rs` | Add `pub mod monitoring` |

## Testing

- Unit tests for gap timer logic (mock file timestamps)
- Unit tests for PID stability (old PID alive vs dead)
- Manual smoke test: kill mic pipeline thread, verify toast + restart
- Manual smoke test: verify no false-positive toasts during normal silent periods
