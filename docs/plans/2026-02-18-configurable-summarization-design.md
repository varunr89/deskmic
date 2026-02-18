# Configurable summarization prompt + flexible date ranges

## Problem

1. The summarization system prompt is hard-coded in `src/summarize/prompt.rs`. Users can't tune it without recompiling.
2. `deskmic summarize` only supports `--period daily` (yesterday) and `--period weekly` (last 7 days). There's no way to re-summarize a specific date or arbitrary range, making prompt iteration painful. Adding monthly/quarterly would require more enum variants.

## Solution

### 1. Configurable system prompt

Add `system_prompt` to `[summarization]` in `deskmic.toml`:

```toml
[summarization]
# System prompt for the LLM summarizer. Use {date_label} as a placeholder.
# Leave empty to use the built-in default.
# system_prompt = ""
```

- Empty string (default) → use the built-in prompt from `prompt.rs`
- Non-empty → use the user's prompt, substituting `{date_label}` at runtime

### 2. Flexible date range CLI

Replace `--period` with a single positional argument:

```
deskmic summarize daily                   # yesterday
deskmic summarize weekly                  # last 7 days
deskmic summarize 2026-02-15              # specific day
deskmic summarize 2026-02-10..2026-02-16  # date range (inclusive)
```

Keywords `daily` and `weekly` are aliases that resolve to concrete dates. Everything flows through one function:

```rust
fn resolve_date_range(arg: &str) -> Result<(Vec<NaiveDate>, String, String)>
```

Returns `(dates, label, file_suffix)` just like the current `resolve_dates()`.

Parsing rules:
- `daily` → yesterday
- `weekly` → last 7 days
- `YYYY-MM-DD` → that single date
- `YYYY-MM-DD..YYYY-MM-DD` → inclusive range (max 90 days as sanity check)
- Anything else → error with usage hint

The `SummarizePeriod` enum and `--period` flag are removed.

## Files changed

| File | Change |
|------|--------|
| `src/config.rs` | Add `system_prompt: String` to `SummarizationConfig`, default `""` |
| `src/config.rs` | Add `system_prompt` to `generate_default_commented()` |
| `src/cli.rs` | Remove `SummarizePeriod` enum. Change `Summarize` to take positional `range: String` (default `"daily"`) |
| `src/summarize/prompt.rs` | `build_prompt()` takes `custom_system_prompt: &str` param; uses it if non-empty |
| `src/summarize/runner.rs` | Replace `resolve_dates(period)` with `resolve_date_range(arg)`. Pass `system_prompt` to `build_prompt()`. |
| `src/main.rs` | Update dispatch: pass `range` string instead of `period` |
| `src/setup.rs` | Update schtasks commands: `summarize daily` / `summarize weekly` (no `--period`) |

## Error handling

- Invalid date format → clear error: `Invalid date range. Expected: daily, weekly, YYYY-MM-DD, or YYYY-MM-DD..YYYY-MM-DD`
- Range > 90 days → error (likely a mistake)
- End date before start date → error
- Custom prompt missing `{date_label}` → warn but proceed (it's optional context)

## Breaking change

CLI syntax changes from `--period daily` to positional `daily`. Only consumers are the two Task Scheduler entries created by `deskmic setup`, which are updated in the same PR.
