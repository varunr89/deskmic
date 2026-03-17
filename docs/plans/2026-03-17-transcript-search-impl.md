# Transcript Search Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `deskmic index` and `deskmic search` commands that embed transcript chunks via Azure OpenAI and store them in SQLite with sqlite-vec for vector similarity search.

**Architecture:** Gap-based chunking groups utterances into conversation segments. Embeddings are generated via Azure OpenAI `text-embedding-3-large` (3072 dimensions). SQLite stores chunks with metadata; sqlite-vec virtual table enables KNN search. Idempotency is achieved by tracking file mtimes and deterministic chunk IDs.

**Tech Stack:** Rust, rusqlite (bundled), sqlite-vec, reqwest (blocking), Azure OpenAI embeddings API, clap derive CLI.

---

## Task 1: Add Dependencies to Cargo.toml

**Files:**
- Modify: `Cargo.toml:8-42` (dependencies section)

**Step 1: Add rusqlite, sqlite-vec, and sha2 (already present) deps**

Add these lines to the `[dependencies]` section in `Cargo.toml`, after the existing `url = "2"` line (line 41):

```toml
# Search (vector index)
rusqlite = { version = "0.34", features = ["bundled"] }
sqlite-vec = "0.1.7"
```

Note: `sha2` is already in Cargo.toml (line 38) for HMAC auth. We'll reuse it for chunk ID hashing. `reqwest` is already present with `blocking` + `json` features.

**Step 2: Verify it compiles**

Run (PowerShell):
```powershell
$env:PATH = "C:\Users\varunramesh\.cargo\bin;C:\Program Files\CMake\bin;C:\Program Files\LLVM\bin;" + $env:PATH
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
cargo check
```
Expected: compiles successfully (warnings OK).

**Step 3: Commit**

```
git add Cargo.toml Cargo.lock
git commit -m "feat(search): add rusqlite and sqlite-vec dependencies"
```

---

## Task 2: Add SearchConfig to Config

**Files:**
- Modify: `src/config.rs:7-18` (Config struct)
- Modify: `src/config.rs:141-153` (Default impl)
- Modify: `src/config.rs:299-399` (generate_default_commented)

**Step 1: Write the failing test**

Add to the bottom of the `#[cfg(test)] mod tests` block in `src/config.rs` (before the closing `}`):

```rust
    #[test]
    fn test_search_config_defaults() {
        let config = Config::default();
        assert_eq!(config.search.embedding_deployment, "");
        assert_eq!(config.search.chunk_gap_secs, 60);
        assert_eq!(config.search.chunk_max_duration_secs, 300);
    }

    #[test]
    fn test_search_config_from_toml() {
        let toml_str = r#"
            [search]
            embedding_deployment = "text-embedding-3-large"
            chunk_gap_secs = 120
            chunk_max_duration_secs = 600
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.search.embedding_deployment, "text-embedding-3-large");
        assert_eq!(config.search.chunk_gap_secs, 120);
        assert_eq!(config.search.chunk_max_duration_secs, 600);
    }

    #[test]
    fn test_search_config_absent_uses_defaults() {
        let toml_str = r#"
            [capture]
            sample_rate = 16000
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.search.chunk_gap_secs, 60);
    }
```

**Step 2: Run tests to verify they fail**

Run (PowerShell):
```powershell
cargo test --lib config::tests::test_search_config -- --nocapture
```
Expected: FAIL — `Config` has no field `search`.

**Step 3: Implement SearchConfig**

Add the `SearchConfig` struct after `MonitoringConfig` (after line 129 in `src/config.rs`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// Azure OpenAI deployment name for embeddings (e.g. "text-embedding-3-large").
    pub embedding_deployment: String,
    /// Seconds of silence between utterances before starting a new chunk.
    pub chunk_gap_secs: u64,
    /// Maximum duration in seconds for a single chunk before splitting.
    pub chunk_max_duration_secs: u64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            embedding_deployment: String::new(),
            chunk_gap_secs: 60,
            chunk_max_duration_secs: 300,
        }
    }
}
```

Add the field to the `Config` struct (after `monitoring: MonitoringConfig`):

```rust
    #[serde(default)]
    pub search: SearchConfig,
```

Add to `Config::default()`:

```rust
            search: SearchConfig::default(),
```

Add `[search]` section to `generate_default_commented()` (after the `[monitoring]` section, before the closing `"#`):

```
[search]
# Azure OpenAI deployment name for text embeddings (used by 'deskmic index').
# This reuses the endpoint and api_key from [transcription.azure].
# embedding_deployment = "text-embedding-3-large"
# Seconds of silence between utterances that triggers a new conversation chunk.
chunk_gap_secs = 60
# Maximum duration in seconds for a single chunk before it is split.
chunk_max_duration_secs = 300
```

Also add `[search]` to `test_generate_default_commented_has_all_sections`:

```rust
        assert!(content.contains("[search]"));
```

**Step 4: Run tests to verify they pass**

Run (PowerShell):
```powershell
cargo test --lib config::tests -- --nocapture
```
Expected: ALL config tests PASS.

**Step 5: Commit**

```
git add src/config.rs
git commit -m "feat(search): add SearchConfig to config with chunk_gap and embedding_deployment"
```

---

## Task 3: Add CLI Subcommands (Index + Search)

**Files:**
- Modify: `src/cli.rs:19-53` (Commands enum)
- Modify: `src/main.rs:76,122-141` (needs_console + dispatch)
- Modify: `src/lib.rs:9` (add pub mod search)
- Create: `src/search/mod.rs` (stub)

**Step 1: Add Index and Search variants to Commands enum**

In `src/cli.rs`, add after the `Setup` variant (line 52):

```rust
    /// Build or update the transcript search index
    Index,

    /// Search transcripts by semantic similarity
    Search {
        /// The search query
        query: String,

        /// Filter results from this date (YYYY-MM-DD)
        #[arg(long)]
        from: Option<String>,

        /// Filter results to this date (YYYY-MM-DD)
        #[arg(long)]
        to: Option<String>,

        /// Filter by audio source (mic or teams)
        #[arg(long)]
        source: Option<String>,

        /// Maximum number of results to return
        #[arg(long, default_value = "10")]
        limit: usize,

        /// Output results as JSON
        #[arg(long)]
        json: bool,
    },
```

**Step 2: Update needs_console in main.rs**

In `src/main.rs`, line 76, `needs_console` already checks `!matches!(cli.command, None | Some(Commands::Record))` — Index and Search are not Record, so they'll get the console. No change needed.

**Step 3: Create stub search module**

Create `src/search/mod.rs`:

```rust
pub mod chunker;
pub mod db;
pub mod embeddings;
pub mod indexer;

use anyhow::Result;
use crate::config::Config;

/// Run the indexing pipeline: scan transcripts, chunk, embed, store.
pub fn run_index(config: &Config) -> Result<()> {
    indexer::run_index(config)
}

/// Search result returned by the search pipeline.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub date: String,
    pub start_time: String,
    pub end_time: String,
    pub source: String,
    pub score: f32,
    pub text: String,
    pub files: Vec<String>,
}

/// Search parameters.
pub struct SearchParams {
    pub query: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub source: Option<String>,
    pub limit: usize,
}

/// Run a semantic search query against the index.
pub fn run_search(config: &Config, params: SearchParams) -> Result<Vec<SearchResult>> {
    let db_path = config.output.directory.join("deskmic-search.db");
    let db = db::SearchDb::open(&db_path)?;

    let embedding = embeddings::EmbeddingClient::from_config(config)?
        .embed_single(&params.query)?;

    db.search(&embedding, &params)
}
```

Create `src/search/chunker.rs` (stub):

```rust
// Chunker — gap-based grouping of transcript utterances into conversation chunks.
```

Create `src/search/db.rs` (stub):

```rust
// SearchDb — SQLite + sqlite-vec database for chunk storage and vector search.
```

Create `src/search/embeddings.rs` (stub):

```rust
// EmbeddingClient — Azure OpenAI text-embedding-3-large client.
```

Create `src/search/indexer.rs` (stub):

```rust
// Indexer — orchestrates the full index pipeline.
```

**Step 4: Add `pub mod search` to lib.rs**

In `src/lib.rs`, add after line 9 (`pub mod setup;`):

```rust
pub mod search;
```

**Step 5: Add match arms in main.rs**

In `src/main.rs`, add after `Commands::Setup => deskmic::setup::run_setup(),` (line 140):

```rust
        Commands::Index => deskmic::search::run_index(&config),
        Commands::Search {
            query,
            from,
            to,
            source,
            limit,
            json,
        } => {
            let params = deskmic::search::SearchParams {
                query,
                from,
                to,
                source,
                limit,
            };
            let results = deskmic::search::run_search(&config, params)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                if results.is_empty() {
                    println!("No results found.");
                } else {
                    for r in &results {
                        println!(
                            "[{} {}-{}] ({}, score: {:.2})",
                            r.date, r.start_time, r.end_time, r.source, r.score
                        );
                        // Show first 200 chars of text
                        let preview = if r.text.len() > 200 {
                            format!("{}...", &r.text[..200])
                        } else {
                            r.text.clone()
                        };
                        println!("{}\n", preview);
                    }
                }
            }
            Ok(())
        }
```

**Step 6: Verify it compiles**

Run (PowerShell):
```powershell
cargo check
```
Expected: compiles (stubs are empty, mod.rs has signatures but bodies won't compile yet — we need to make the stubs minimal). Actually, since `mod.rs` references `indexer::run_index`, `db::SearchDb`, and `embeddings::EmbeddingClient` which don't exist yet, this won't compile. So we have two choices: (a) comment out the bodies, or (b) implement stubs that return `todo!()`. Let's use approach (b) — put `todo!()` stubs in each file so it compiles.

Update the stubs:

`src/search/chunker.rs`:
```rust
// Chunker — gap-based grouping of transcript utterances into conversation chunks.
// Implementation in Task 4.
```

`src/search/embeddings.rs`:
```rust
use anyhow::Result;
use crate::config::Config;

pub struct EmbeddingClient;

impl EmbeddingClient {
    pub fn from_config(_config: &Config) -> Result<Self> {
        todo!("EmbeddingClient::from_config")
    }

    pub fn embed_single(&self, _text: &str) -> Result<Vec<f32>> {
        todo!("EmbeddingClient::embed_single")
    }

    pub fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        todo!("EmbeddingClient::embed_batch")
    }
}
```

`src/search/db.rs`:
```rust
use std::path::Path;
use anyhow::Result;
use crate::search::{SearchParams, SearchResult};

pub struct SearchDb;

impl SearchDb {
    pub fn open(_path: &Path) -> Result<Self> {
        todo!("SearchDb::open")
    }

    pub fn search(&self, _embedding: &[f32], _params: &SearchParams) -> Result<Vec<SearchResult>> {
        todo!("SearchDb::search")
    }
}
```

`src/search/indexer.rs`:
```rust
use anyhow::Result;
use crate::config::Config;

pub fn run_index(_config: &Config) -> Result<()> {
    todo!("run_index")
}
```

**Step 7: Verify it compiles (with stubs)**

Run (PowerShell):
```powershell
cargo check
```
Expected: compiles with warnings about unused imports / dead code. No errors.

**Step 8: Commit**

```
git add src/cli.rs src/main.rs src/lib.rs src/search/
git commit -m "feat(search): add Index and Search CLI subcommands with todo stubs"
```

---

## Task 4: Implement Chunker (TDD)

**Files:**
- Modify: `src/search/chunker.rs`

This is the core pure-function module. No I/O, no dependencies on config or DB.

**Step 1: Write the Chunk struct and function signature**

Replace `src/search/chunker.rs` with:

```rust
use sha2::{Digest, Sha256};

use crate::transcribe::backend::Transcript;

/// A conversation chunk — a group of temporally-close utterances from the same source.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Deterministic ID: SHA-256 hex of "{date}|{source}|{start_file}".
    pub id: String,
    /// Date string (YYYY-MM-DD) from the transcript timestamp field.
    pub date: String,
    /// Audio source (e.g. "mic", "teams").
    pub source: String,
    /// Start time extracted from the first filename (e.g. "09-37-31").
    pub start_time: String,
    /// End time: start time of last utterance + its duration.
    pub end_time: String,
    /// Total audio duration of all constituent utterances in seconds.
    pub duration_secs: f64,
    /// Concatenated text of all utterances, space-separated.
    pub text: String,
    /// List of constituent WAV filenames.
    pub files: Vec<String>,
}

/// Extract the time portion from a filename like "mic_09-37-31.wav" -> "09-37-31".
/// Returns None if the filename doesn't match the expected pattern.
fn extract_time_from_filename(filename: &str) -> Option<String> {
    // Pattern: {source}_{HH-MM-SS}.wav
    let stem = filename.strip_suffix(".wav")?;
    let time_part = stem.split('_').last()?;
    // Validate it looks like HH-MM-SS
    let parts: Vec<&str> = time_part.split('-').collect();
    if parts.len() == 3
        && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit()))
    {
        Some(time_part.to_string())
    } else {
        None
    }
}

/// Convert "HH-MM-SS" to total seconds since midnight.
fn time_to_secs(time: &str) -> Option<f64> {
    let parts: Vec<&str> = time.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f64 = parts[0].parse().ok()?;
    let m: f64 = parts[1].parse().ok()?;
    let s: f64 = parts[2].parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

/// Convert total seconds since midnight to "HH-MM-SS".
fn secs_to_time(secs: f64) -> String {
    let total = secs.round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{:02}-{:02}-{:02}", h, m, s)
}

/// Compute the deterministic chunk ID from date, source, and first filename.
fn chunk_id(date: &str, source: &str, start_file: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}|{}|{}", date, source, start_file));
    format!("{:x}", hasher.finalize())
}

/// Chunk a list of transcripts (all from the same date/file) into conversation segments.
///
/// `chunk_gap_secs`: silence gap in seconds that triggers a new chunk.
/// `chunk_max_duration_secs`: maximum duration of a single chunk before splitting.
///
/// Transcripts are sorted by filename (chronological order) before chunking.
/// A new chunk also starts when the source changes.
pub fn chunk_transcripts(
    transcripts: &[Transcript],
    chunk_gap_secs: u64,
    chunk_max_duration_secs: u64,
) -> Vec<Chunk> {
    if transcripts.is_empty() {
        return Vec::new();
    }

    // Sort by filename for chronological order.
    let mut sorted: Vec<&Transcript> = transcripts.iter().collect();
    sorted.sort_by(|a, b| a.file.cmp(&b.file));

    let mut chunks = Vec::new();
    let mut current_texts: Vec<&str> = Vec::new();
    let mut current_files: Vec<String> = Vec::new();
    let mut current_source = String::new();
    let mut current_date = String::new();
    let mut current_duration: f64 = 0.0;
    let mut current_start_time: Option<String> = None;
    let mut prev_end_secs: Option<f64> = None;

    for t in &sorted {
        let time_str = extract_time_from_filename(&t.file);
        let start_secs = time_str.as_ref().and_then(|ts| time_to_secs(ts));

        // Decide whether to start a new chunk.
        let start_new = if current_texts.is_empty() {
            true
        } else if t.source != current_source {
            // Source changed.
            true
        } else if let (Some(prev_end), Some(cur_start)) = (prev_end_secs, start_secs) {
            let gap = cur_start - prev_end;
            gap > chunk_gap_secs as f64
        } else {
            false
        } || (!current_texts.is_empty()
            && current_duration >= chunk_max_duration_secs as f64);

        if start_new && !current_texts.is_empty() {
            // Finalize the current chunk.
            let end_secs = prev_end_secs.unwrap_or(0.0);
            chunks.push(Chunk {
                id: chunk_id(
                    &current_date,
                    &current_source,
                    current_files.first().unwrap(),
                ),
                date: current_date.clone(),
                source: current_source.clone(),
                start_time: current_start_time.clone().unwrap_or_default(),
                end_time: secs_to_time(end_secs),
                duration_secs: current_duration,
                text: current_texts.join(" "),
                files: current_files.clone(),
            });
            current_texts.clear();
            current_files.clear();
            current_duration = 0.0;
            current_start_time = None;
            prev_end_secs = None;
        }

        // Add utterance to current chunk.
        if current_texts.is_empty() {
            current_source = t.source.clone();
            current_date = t.timestamp.clone();
            current_start_time = time_str.clone();
        }
        current_texts.push(&t.text);
        current_files.push(t.file.clone());
        current_duration += t.duration_secs;
        if let Some(s) = start_secs {
            prev_end_secs = Some(s + t.duration_secs);
        }
    }

    // Finalize last chunk.
    if !current_texts.is_empty() {
        let end_secs = prev_end_secs.unwrap_or(0.0);
        chunks.push(Chunk {
            id: chunk_id(
                &current_date,
                &current_source,
                current_files.first().unwrap(),
            ),
            date: current_date,
            source: current_source,
            start_time: current_start_time.unwrap_or_default(),
            end_time: secs_to_time(end_secs),
            duration_secs: current_duration,
            text: current_texts.join(" "),
            files: current_files,
        });
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_transcript(source: &str, file: &str, text: &str, duration: f64) -> Transcript {
        Transcript {
            timestamp: "2026-03-16".to_string(),
            source: source.to_string(),
            duration_secs: duration,
            file: file.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn test_extract_time_from_filename() {
        assert_eq!(
            extract_time_from_filename("mic_09-37-31.wav"),
            Some("09-37-31".to_string())
        );
        assert_eq!(
            extract_time_from_filename("teams_14-02-05.wav"),
            Some("14-02-05".to_string())
        );
        assert_eq!(extract_time_from_filename("invalid.wav"), None);
        assert_eq!(extract_time_from_filename("mic_09-37.wav"), None);
    }

    #[test]
    fn test_time_conversions() {
        assert_eq!(time_to_secs("09-37-31"), Some(9.0 * 3600.0 + 37.0 * 60.0 + 31.0));
        assert_eq!(time_to_secs("00-00-00"), Some(0.0));
        assert_eq!(secs_to_time(0.0), "00-00-00");
        assert_eq!(secs_to_time(3661.0), "01-01-01");
    }

    #[test]
    fn test_chunk_id_is_deterministic() {
        let id1 = chunk_id("2026-03-16", "mic", "mic_09-37-31.wav");
        let id2 = chunk_id("2026-03-16", "mic", "mic_09-37-31.wav");
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn test_chunk_id_differs_for_different_inputs() {
        let id1 = chunk_id("2026-03-16", "mic", "mic_09-37-31.wav");
        let id2 = chunk_id("2026-03-16", "mic", "mic_10-00-00.wav");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_empty_transcripts_returns_empty() {
        let chunks = chunk_transcripts(&[], 60, 300);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_single_utterance_becomes_one_chunk() {
        let transcripts = vec![
            make_transcript("mic", "mic_09-37-31.wav", "Hello world", 5.0),
        ];
        let chunks = chunk_transcripts(&transcripts, 60, 300);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Hello world");
        assert_eq!(chunks[0].source, "mic");
        assert_eq!(chunks[0].date, "2026-03-16");
        assert_eq!(chunks[0].start_time, "09-37-31");
        assert_eq!(chunks[0].files, vec!["mic_09-37-31.wav"]);
        assert!((chunks[0].duration_secs - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_close_utterances_grouped_together() {
        let transcripts = vec![
            make_transcript("mic", "mic_09-37-31.wav", "Hello", 5.0),
            make_transcript("mic", "mic_09-37-40.wav", "world", 3.0),
        ];
        // Gap is 09:37:40 - (09:37:31 + 5) = 4 seconds < 60
        let chunks = chunk_transcripts(&transcripts, 60, 300);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Hello world");
        assert_eq!(chunks[0].files.len(), 2);
    }

    #[test]
    fn test_gap_splits_into_two_chunks() {
        let transcripts = vec![
            make_transcript("mic", "mic_09-37-31.wav", "Hello", 5.0),
            make_transcript("mic", "mic_09-40-00.wav", "Goodbye", 3.0),
        ];
        // Gap is 09:40:00 - (09:37:31 + 5) = ~144 secs > 60
        let chunks = chunk_transcripts(&transcripts, 60, 300);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text, "Hello");
        assert_eq!(chunks[1].text, "Goodbye");
    }

    #[test]
    fn test_source_change_splits_chunk() {
        let transcripts = vec![
            make_transcript("mic", "mic_09-37-31.wav", "Hello", 5.0),
            make_transcript("teams", "teams_09-37-40.wav", "Meeting start", 3.0),
        ];
        let chunks = chunk_transcripts(&transcripts, 60, 300);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].source, "mic");
        assert_eq!(chunks[1].source, "teams");
    }

    #[test]
    fn test_max_duration_splits_chunk() {
        // Create utterances that together exceed 300 seconds
        let mut transcripts = Vec::new();
        for i in 0..40 {
            let secs = i * 10;
            let m = secs / 60;
            let s = secs % 60;
            let file = format!("mic_09-{:02}-{:02}.wav", m, s);
            transcripts.push(make_transcript("mic", &file, &format!("word{}", i), 10.0));
        }
        let chunks = chunk_transcripts(&transcripts, 60, 300);
        // 40 utterances * 10s = 400s, should be split into at least 2 chunks
        assert!(chunks.len() >= 2, "Expected at least 2 chunks, got {}", chunks.len());
        // Each chunk should be <= 300 seconds
        for c in &chunks {
            assert!(
                c.duration_secs <= 310.0, // small tolerance for the last utterance
                "Chunk duration {} exceeds max",
                c.duration_secs
            );
        }
    }

    #[test]
    fn test_unsorted_input_is_sorted_by_filename() {
        let transcripts = vec![
            make_transcript("mic", "mic_09-38-00.wav", "second", 3.0),
            make_transcript("mic", "mic_09-37-31.wav", "first", 5.0),
        ];
        let chunks = chunk_transcripts(&transcripts, 60, 300);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "first second");
        assert_eq!(chunks[0].start_time, "09-37-31");
    }

    #[test]
    fn test_chunk_end_time_calculation() {
        let transcripts = vec![
            make_transcript("mic", "mic_09-37-31.wav", "Hello", 5.0),
            make_transcript("mic", "mic_09-37-40.wav", "world", 10.0),
        ];
        let chunks = chunk_transcripts(&transcripts, 60, 300);
        assert_eq!(chunks.len(), 1);
        // End = 09:37:40 + 10s = 09:37:50
        assert_eq!(chunks[0].end_time, "09-37-50");
    }
}
```

**Step 2: Run tests to verify they pass**

Run (PowerShell):
```powershell
cargo test --lib search::chunker::tests -- --nocapture
```
Expected: ALL chunker tests PASS.

**Step 3: Commit**

```
git add src/search/chunker.rs
git commit -m "feat(search): implement gap-based chunker with comprehensive tests"
```

---

## Task 5: Implement SearchDb (TDD)

**Files:**
- Modify: `src/search/db.rs`

**Step 1: Write the full implementation with tests**

Replace `src/search/db.rs` with:

```rust
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::search::chunker::Chunk;
use crate::search::{SearchParams, SearchResult};

/// Embedding dimensions for text-embedding-3-large.
pub const EMBEDDING_DIM: usize = 3072;

pub struct SearchDb {
    conn: Connection,
}

impl SearchDb {
    /// Open (or create) the search database at the given path.
    /// Initializes the schema and loads sqlite-vec.
    pub fn open(path: &Path) -> Result<Self> {
        // Load sqlite-vec as an auto extension before opening.
        unsafe {
            let rc = sqlite_vec::sqlite3_vec_init_loadable();
            if rc != 0 {
                // Already loaded is fine; only bail on real errors.
                tracing::debug!("sqlite-vec init returned {}", rc);
            }
        }

        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open search DB at {}", path.display()))?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        Self::init_schema(&conn)?;

        Ok(Self { conn })
    }

    /// Open an in-memory database (for testing).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        unsafe {
            let _ = sqlite_vec::sqlite3_vec_init_loadable();
        }

        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS indexed_files (
                file_name TEXT PRIMARY KEY,
                modified_at INTEGER NOT NULL,
                indexed_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chunks (
                id TEXT PRIMARY KEY,
                date TEXT NOT NULL,
                source TEXT NOT NULL,
                start_time TEXT NOT NULL,
                end_time TEXT NOT NULL,
                duration_secs REAL NOT NULL,
                text TEXT NOT NULL,
                files TEXT NOT NULL,
                embedding BLOB NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_chunks_date ON chunks(date);
            CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source);",
        )?;

        // Create sqlite-vec virtual table if it doesn't exist.
        // vec0 tables don't support IF NOT EXISTS, so check first.
        let vec_table_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='vec_chunks'",
            [],
            |row| row.get(0),
        )?;

        if !vec_table_exists {
            conn.execute_batch(&format!(
                "CREATE VIRTUAL TABLE vec_chunks USING vec0(embedding float[{}])",
                EMBEDDING_DIM
            ))?;
        }

        Ok(())
    }

    /// Check the stored mtime for a transcript file. Returns None if not indexed.
    pub fn get_file_mtime(&self, file_name: &str) -> Result<Option<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT modified_at FROM indexed_files WHERE file_name = ?1")?;
        let result = stmt.query_row(params![file_name], |row| row.get(0));
        match result {
            Ok(mtime) => Ok(Some(mtime)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Update the stored mtime for a transcript file.
    pub fn set_file_mtime(&self, file_name: &str, modified_at: i64) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "INSERT OR REPLACE INTO indexed_files (file_name, modified_at, indexed_at) VALUES (?1, ?2, ?3)",
            params![file_name, modified_at, now],
        )?;
        Ok(())
    }

    /// Delete all chunks for a given date (used before re-indexing a changed file).
    pub fn delete_chunks_for_date(&self, date: &str) -> Result<usize> {
        // First get the rowids of chunks to delete from vec table.
        let rowids: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT rowid FROM chunks WHERE date = ?1")?;
            let rows = stmt.query_map(params![date], |row| row.get(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        for rowid in &rowids {
            self.conn
                .execute("DELETE FROM vec_chunks WHERE rowid = ?1", params![rowid])?;
        }

        let deleted = self
            .conn
            .execute("DELETE FROM chunks WHERE date = ?1", params![date])?;

        Ok(deleted)
    }

    /// Insert a chunk with its embedding. Returns the rowid.
    pub fn insert_chunk(&self, chunk: &Chunk, embedding: &[f32]) -> Result<i64> {
        assert_eq!(
            embedding.len(),
            EMBEDDING_DIM,
            "Embedding must be {} dimensions",
            EMBEDDING_DIM
        );

        let files_json = serde_json::to_string(&chunk.files)?;
        let embedding_bytes = embedding_to_bytes(embedding);

        self.conn.execute(
            "INSERT OR REPLACE INTO chunks (id, date, source, start_time, end_time, duration_secs, text, files, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                chunk.id,
                chunk.date,
                chunk.source,
                chunk.start_time,
                chunk.end_time,
                chunk.duration_secs,
                chunk.text,
                files_json,
                embedding_bytes,
            ],
        )?;

        let rowid = self.conn.last_insert_rowid();

        // Insert into vec_chunks with the same rowid.
        self.conn.execute(
            "INSERT INTO vec_chunks (rowid, embedding) VALUES (?1, ?2)",
            params![rowid, embedding_bytes],
        )?;

        Ok(rowid)
    }

    /// Insert multiple chunks in a single transaction.
    pub fn insert_chunks(&self, chunks_with_embeddings: &[(&Chunk, &[f32])]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (chunk, embedding) in chunks_with_embeddings {
            self.insert_chunk(chunk, embedding)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Count total chunks in the database.
    pub fn count_chunks(&self) -> Result<usize> {
        let count: usize = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Search for similar chunks using vector similarity.
    pub fn search(&self, query_embedding: &[f32], params: &SearchParams) -> Result<Vec<SearchResult>> {
        let embedding_bytes = embedding_to_bytes(query_embedding);

        // Retrieve more candidates than needed to allow for post-filtering.
        let candidate_limit = params.limit * 3;

        // KNN query via sqlite-vec.
        let mut stmt = self.conn.prepare(
            "SELECT v.rowid, v.distance
             FROM vec_chunks v
             WHERE v.embedding MATCH ?1
             ORDER BY v.distance
             LIMIT ?2",
        )?;

        let candidates: Vec<(i64, f32)> = stmt
            .query_map(params![embedding_bytes, candidate_limit as i64], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut results = Vec::new();

        for (rowid, distance) in candidates {
            let row = self.conn.query_row(
                "SELECT id, date, source, start_time, end_time, text, files FROM chunks WHERE rowid = ?1",
                params![rowid],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            );

            let (_id, date, source, start_time, end_time, text, files_json) = match row {
                Ok(r) => r,
                Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                Err(e) => return Err(e.into()),
            };

            // Apply filters.
            if let Some(ref from) = params.from {
                if date < *from {
                    continue;
                }
            }
            if let Some(ref to) = params.to {
                if date > *to {
                    continue;
                }
            }
            if let Some(ref src) = params.source {
                if source != *src {
                    continue;
                }
            }

            let files: Vec<String> = serde_json::from_str(&files_json).unwrap_or_default();

            // Convert distance to similarity score (1 - distance for cosine).
            let score = 1.0 - distance;

            results.push(SearchResult {
                date,
                start_time,
                end_time,
                source,
                score,
                text,
                files,
            });

            if results.len() >= params.limit {
                break;
            }
        }

        Ok(results)
    }
}

/// Convert a slice of f32 to bytes (little-endian) for sqlite-vec.
fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::chunker::Chunk;

    fn make_test_chunk(id: &str, date: &str, source: &str, text: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            date: date.to_string(),
            source: source.to_string(),
            start_time: "09-37-31".to_string(),
            end_time: "09-42-00".to_string(),
            duration_secs: 269.0,
            text: text.to_string(),
            files: vec!["mic_09-37-31.wav".to_string()],
        }
    }

    fn make_test_embedding(seed: f32) -> Vec<f32> {
        // Create a simple embedding with a pattern for testing.
        let mut emb = vec![0.0f32; EMBEDDING_DIM];
        for (i, val) in emb.iter_mut().enumerate() {
            *val = ((i as f32 + seed) * 0.001).sin();
        }
        emb
    }

    #[test]
    fn test_open_in_memory() {
        let db = SearchDb::open_in_memory().unwrap();
        assert_eq!(db.count_chunks().unwrap(), 0);
    }

    #[test]
    fn test_file_mtime_not_indexed() {
        let db = SearchDb::open_in_memory().unwrap();
        assert_eq!(db.get_file_mtime("2026-03-16.jsonl").unwrap(), None);
    }

    #[test]
    fn test_file_mtime_roundtrip() {
        let db = SearchDb::open_in_memory().unwrap();
        db.set_file_mtime("2026-03-16.jsonl", 1710600000).unwrap();
        assert_eq!(
            db.get_file_mtime("2026-03-16.jsonl").unwrap(),
            Some(1710600000)
        );
    }

    #[test]
    fn test_file_mtime_update() {
        let db = SearchDb::open_in_memory().unwrap();
        db.set_file_mtime("2026-03-16.jsonl", 1000).unwrap();
        db.set_file_mtime("2026-03-16.jsonl", 2000).unwrap();
        assert_eq!(
            db.get_file_mtime("2026-03-16.jsonl").unwrap(),
            Some(2000)
        );
    }

    #[test]
    fn test_insert_and_count_chunk() {
        let db = SearchDb::open_in_memory().unwrap();
        let chunk = make_test_chunk("abc123", "2026-03-16", "mic", "Hello world");
        let emb = make_test_embedding(1.0);
        db.insert_chunk(&chunk, &emb).unwrap();
        assert_eq!(db.count_chunks().unwrap(), 1);
    }

    #[test]
    fn test_insert_multiple_chunks() {
        let db = SearchDb::open_in_memory().unwrap();
        let c1 = make_test_chunk("a", "2026-03-16", "mic", "Hello");
        let c2 = make_test_chunk("b", "2026-03-16", "teams", "Meeting");
        let e1 = make_test_embedding(1.0);
        let e2 = make_test_embedding(2.0);
        let pairs: Vec<(&Chunk, &[f32])> = vec![(&c1, e1.as_slice()), (&c2, e2.as_slice())];
        db.insert_chunks(&pairs).unwrap();
        assert_eq!(db.count_chunks().unwrap(), 2);
    }

    #[test]
    fn test_delete_chunks_for_date() {
        let db = SearchDb::open_in_memory().unwrap();
        let c1 = make_test_chunk("a", "2026-03-16", "mic", "Hello");
        let c2 = make_test_chunk("b", "2026-03-17", "mic", "Goodbye");
        let e1 = make_test_embedding(1.0);
        let e2 = make_test_embedding(2.0);
        db.insert_chunk(&c1, &e1).unwrap();
        db.insert_chunk(&c2, &e2).unwrap();
        assert_eq!(db.count_chunks().unwrap(), 2);

        let deleted = db.delete_chunks_for_date("2026-03-16").unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(db.count_chunks().unwrap(), 1);
    }

    #[test]
    fn test_search_returns_results() {
        let db = SearchDb::open_in_memory().unwrap();
        let c1 = make_test_chunk("a", "2026-03-16", "mic", "Hello world this is a test");
        let e1 = make_test_embedding(1.0);
        db.insert_chunk(&c1, &e1).unwrap();

        let params = SearchParams {
            query: "test".to_string(),
            from: None,
            to: None,
            source: None,
            limit: 10,
        };
        let results = db.search(&e1, &params).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "Hello world this is a test");
        assert!(results[0].score > 0.9); // Same vector = very high score.
    }

    #[test]
    fn test_search_date_filter() {
        let db = SearchDb::open_in_memory().unwrap();
        let c1 = make_test_chunk("a", "2026-03-15", "mic", "Yesterday");
        let c2 = make_test_chunk("b", "2026-03-16", "mic", "Today");
        let e1 = make_test_embedding(1.0);
        let e2 = make_test_embedding(1.1); // Similar embedding
        db.insert_chunk(&c1, &e1).unwrap();
        db.insert_chunk(&c2, &e2).unwrap();

        let params = SearchParams {
            query: "test".to_string(),
            from: Some("2026-03-16".to_string()),
            to: None,
            source: None,
            limit: 10,
        };
        let results = db.search(&e1, &params).unwrap();
        assert!(results.iter().all(|r| r.date >= "2026-03-16"));
    }

    #[test]
    fn test_search_source_filter() {
        let db = SearchDb::open_in_memory().unwrap();
        let c1 = make_test_chunk("a", "2026-03-16", "mic", "Mic audio");
        let c2 = make_test_chunk("b", "2026-03-16", "teams", "Teams audio");
        let e1 = make_test_embedding(1.0);
        let e2 = make_test_embedding(1.1);
        db.insert_chunk(&c1, &e1).unwrap();
        db.insert_chunk(&c2, &e2).unwrap();

        let params = SearchParams {
            query: "test".to_string(),
            from: None,
            to: None,
            source: Some("mic".to_string()),
            limit: 10,
        };
        let results = db.search(&e1, &params).unwrap();
        assert!(results.iter().all(|r| r.source == "mic"));
    }

    #[test]
    fn test_search_limit() {
        let db = SearchDb::open_in_memory().unwrap();
        for i in 0..5 {
            let chunk = make_test_chunk(
                &format!("c{}", i),
                "2026-03-16",
                "mic",
                &format!("Chunk {}", i),
            );
            let emb = make_test_embedding(i as f32);
            db.insert_chunk(&chunk, &emb).unwrap();
        }

        let params = SearchParams {
            query: "test".to_string(),
            from: None,
            to: None,
            source: None,
            limit: 3,
        };
        let results = db.search(&make_test_embedding(0.0), &params).unwrap();
        assert!(results.len() <= 3);
    }

    #[test]
    fn test_idempotent_reindex() {
        let db = SearchDb::open_in_memory().unwrap();
        let chunk = make_test_chunk("a", "2026-03-16", "mic", "Hello");
        let emb = make_test_embedding(1.0);

        // First index
        db.insert_chunk(&chunk, &emb).unwrap();
        db.set_file_mtime("2026-03-16.jsonl", 1000).unwrap();
        assert_eq!(db.count_chunks().unwrap(), 1);

        // Re-index: delete + re-insert
        db.delete_chunks_for_date("2026-03-16").unwrap();
        db.insert_chunk(&chunk, &emb).unwrap();
        db.set_file_mtime("2026-03-16.jsonl", 2000).unwrap();
        assert_eq!(db.count_chunks().unwrap(), 1);
    }
}
```

**Step 2: Run tests to verify they pass**

Run (PowerShell):
```powershell
cargo test --lib search::db::tests -- --nocapture
```
Expected: ALL db tests PASS.

**Step 3: Commit**

```
git add src/search/db.rs
git commit -m "feat(search): implement SearchDb with SQLite + sqlite-vec and comprehensive tests"
```

---

## Task 6: Implement EmbeddingClient

**Files:**
- Modify: `src/search/embeddings.rs`

**Step 1: Write the implementation with tests**

Replace `src/search/embeddings.rs` with:

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    input: Vec<String>,
    model: String,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
    usage: Option<EmbeddingUsage>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

#[derive(Debug, Deserialize)]
struct EmbeddingUsage {
    prompt_tokens: u64,
    total_tokens: u64,
}

pub struct EmbeddingClient {
    endpoint: String,
    api_key: String,
    deployment: String,
    client: reqwest::blocking::Client,
}

impl EmbeddingClient {
    /// Create a new embedding client from config.
    /// Uses the Azure OpenAI endpoint/api_key from [transcription.azure]
    /// and the deployment name from [search].
    pub fn from_config(config: &Config) -> Result<Self> {
        let azure = &config.transcription.azure;
        let search = &config.search;

        let endpoint = if azure.endpoint.is_empty() {
            anyhow::bail!(
                "Azure OpenAI endpoint not configured. \
                 Set [transcription.azure] endpoint in deskmic.toml"
            );
        } else {
            azure.endpoint.trim_end_matches('/').to_string()
        };

        let api_key = if !azure.api_key.is_empty() {
            azure.api_key.clone()
        } else {
            std::env::var("DESKMIC_AZURE_KEY")
                .context("Azure API key not configured. Set [transcription.azure] api_key or DESKMIC_AZURE_KEY")?
        };

        let deployment = if search.embedding_deployment.is_empty() {
            anyhow::bail!(
                "Embedding deployment not configured. \
                 Set [search] embedding_deployment in deskmic.toml"
            );
        } else {
            search.embedding_deployment.clone()
        };

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        Ok(Self {
            endpoint,
            api_key,
            deployment,
            client,
        })
    }

    /// Embed a single text string. Returns a Vec<f32> of embedding dimensions.
    pub fn embed_single(&self, text: &str) -> Result<Vec<f32>> {
        let results = self.embed_batch(&[text])?;
        results
            .into_iter()
            .next()
            .context("Empty response from embedding API")
    }

    /// Embed a batch of text strings (up to 16 per API call).
    /// Returns embeddings in the same order as the input texts.
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_embeddings = Vec::with_capacity(texts.len());

        // Azure OpenAI allows up to 16 inputs per request.
        for batch in texts.chunks(16) {
            let request = EmbeddingRequest {
                input: batch.iter().map(|s| s.to_string()).collect(),
                model: self.deployment.clone(),
            };

            let url = format!(
                "{}/openai/deployments/{}/embeddings?api-version=2024-06-01",
                self.endpoint, self.deployment
            );

            tracing::debug!(
                "Embedding batch of {} texts via {}",
                batch.len(),
                self.deployment
            );

            let response = self
                .client
                .post(&url)
                .header("api-key", &self.api_key)
                .json(&request)
                .send()
                .context("Failed to send embedding request")?;

            let status = response.status();
            if !status.is_success() {
                let error_body = response
                    .text()
                    .unwrap_or_else(|_| "unable to read response body".to_string());
                anyhow::bail!(
                    "Azure OpenAI embeddings returned HTTP {}: {}",
                    status.as_u16(),
                    error_body
                );
            }

            let emb_response: EmbeddingResponse = response
                .json()
                .context("Failed to parse embedding response")?;

            if let Some(usage) = &emb_response.usage {
                tracing::debug!(
                    "Embedding usage: prompt_tokens={}, total_tokens={}",
                    usage.prompt_tokens,
                    usage.total_tokens
                );
            }

            // Sort by index to ensure correct ordering.
            let mut sorted_data = emb_response.data;
            sorted_data.sort_by_key(|d| d.index);

            for d in sorted_data {
                all_embeddings.push(d.embedding);
            }
        }

        Ok(all_embeddings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AzureConfig, SearchConfig};

    #[test]
    fn test_from_config_missing_endpoint() {
        let mut config = Config::default();
        config.search.embedding_deployment = "text-embedding-3-large".to_string();
        let result = EmbeddingClient::from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("endpoint"));
    }

    #[test]
    fn test_from_config_missing_deployment() {
        let mut config = Config::default();
        config.transcription.azure = AzureConfig {
            endpoint: "https://example.openai.azure.com".to_string(),
            api_key: "test-key".to_string(),
            deployment: "whisper".to_string(),
        };
        // embedding_deployment is empty by default
        let result = EmbeddingClient::from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("deployment"));
    }

    #[test]
    fn test_from_config_success() {
        let mut config = Config::default();
        config.transcription.azure = AzureConfig {
            endpoint: "https://example.openai.azure.com".to_string(),
            api_key: "test-key".to_string(),
            deployment: "whisper".to_string(),
        };
        config.search = SearchConfig {
            embedding_deployment: "text-embedding-3-large".to_string(),
            chunk_gap_secs: 60,
            chunk_max_duration_secs: 300,
        };
        let client = EmbeddingClient::from_config(&config);
        assert!(client.is_ok());
    }

    #[test]
    fn test_embedding_request_serialization() {
        let request = EmbeddingRequest {
            input: vec!["Hello world".to_string()],
            model: "text-embedding-3-large".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("Hello world"));
        assert!(json.contains("text-embedding-3-large"));
    }

    #[test]
    fn test_embedding_response_deserialization() {
        let json = r#"{
            "data": [
                {"embedding": [0.1, 0.2, 0.3], "index": 0}
            ],
            "usage": {"prompt_tokens": 5, "total_tokens": 5}
        }"#;
        let resp: EmbeddingResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].embedding, vec![0.1, 0.2, 0.3]);
        assert_eq!(resp.data[0].index, 0);
        assert_eq!(resp.usage.unwrap().prompt_tokens, 5);
    }
}
```

**Step 2: Run tests to verify they pass**

Run (PowerShell):
```powershell
cargo test --lib search::embeddings::tests -- --nocapture
```
Expected: ALL embedding tests PASS.

**Step 3: Commit**

```
git add src/search/embeddings.rs
git commit -m "feat(search): implement EmbeddingClient for Azure OpenAI text-embedding-3-large"
```

---

## Task 7: Implement Indexer

**Files:**
- Modify: `src/search/indexer.rs`

**Step 1: Write the implementation**

Replace `src/search/indexer.rs` with:

```rust
use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::search::chunker::{chunk_transcripts, Chunk};
use crate::search::db::SearchDb;
use crate::search::embeddings::EmbeddingClient;
use crate::transcribe::backend::Transcript;

/// Run the full indexing pipeline.
pub fn run_index(config: &Config) -> Result<()> {
    let recordings_dir = &config.output.directory;
    let transcript_dir = recordings_dir.join("transcripts");
    let db_path = recordings_dir.join("deskmic-search.db");

    if !transcript_dir.exists() {
        println!("No transcripts directory found at {}", transcript_dir.display());
        return Ok(());
    }

    let db = SearchDb::open(&db_path)?;
    let embedder = EmbeddingClient::from_config(config)?;

    // Scan for .jsonl files.
    let mut jsonl_files: Vec<_> = std::fs::read_dir(&transcript_dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                Some(path)
            } else {
                None
            }
        })
        .collect();
    jsonl_files.sort();

    let mut total_new_chunks = 0usize;
    let mut total_files_indexed = 0usize;
    let total_existing = db.count_chunks()?;

    for jsonl_path in &jsonl_files {
        let file_name = jsonl_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        // Check mtime for idempotency.
        let metadata = std::fs::metadata(jsonl_path)
            .with_context(|| format!("Failed to stat {}", jsonl_path.display()))?;
        let mtime = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        let stored_mtime = db.get_file_mtime(file_name)?;
        if stored_mtime == Some(mtime) {
            tracing::debug!("Skipping {} (unchanged)", file_name);
            continue;
        }

        tracing::info!("Indexing {} (mtime changed or new)", file_name);

        // Load transcripts from this file.
        let transcripts = load_jsonl(jsonl_path)?;
        if transcripts.is_empty() {
            tracing::debug!("No transcripts in {}", file_name);
            db.set_file_mtime(file_name, mtime)?;
            continue;
        }

        // Extract date from filename (e.g. "2026-03-16.jsonl" -> "2026-03-16").
        let date = file_name.strip_suffix(".jsonl").unwrap_or(file_name);

        // Delete old chunks for this date (idempotent re-index).
        let deleted = db.delete_chunks_for_date(date)?;
        if deleted > 0 {
            tracing::info!("Deleted {} old chunks for {}", deleted, date);
        }

        // Chunk transcripts.
        let chunks = chunk_transcripts(
            &transcripts,
            config.search.chunk_gap_secs,
            config.search.chunk_max_duration_secs,
        );

        if chunks.is_empty() {
            db.set_file_mtime(file_name, mtime)?;
            continue;
        }

        // Embed chunks in batches.
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let embeddings = embedder.embed_batch(&texts)?;

        if embeddings.len() != chunks.len() {
            anyhow::bail!(
                "Embedding count mismatch: {} chunks but {} embeddings",
                chunks.len(),
                embeddings.len()
            );
        }

        // Insert into database.
        let pairs: Vec<(&Chunk, &[f32])> = chunks
            .iter()
            .zip(embeddings.iter())
            .map(|(c, e)| (c, e.as_slice()))
            .collect();
        db.insert_chunks(&pairs)?;

        // Update mtime.
        db.set_file_mtime(file_name, mtime)?;

        total_new_chunks += chunks.len();
        total_files_indexed += 1;

        tracing::info!(
            "Indexed {} chunks from {}",
            chunks.len(),
            file_name
        );
    }

    let total_chunks = db.count_chunks()?;
    println!(
        "Indexed {} new chunks from {} transcripts ({} total chunks)",
        total_new_chunks, total_files_indexed, total_chunks
    );

    Ok(())
}

/// Load transcripts from a JSONL file.
fn load_jsonl(path: &Path) -> Result<Vec<Transcript>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let mut transcripts = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Transcript>(trimmed) {
            Ok(t) => transcripts.push(t),
            Err(e) => {
                tracing::warn!(
                    "Failed to parse line {} of {}: {}",
                    line_num + 1,
                    path.display(),
                    e
                );
            }
        }
    }

    Ok(transcripts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_jsonl_valid() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("2026-03-16.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":"2026-03-16","source":"mic","duration_secs":5.0,"file":"mic_09-37-31.wav","text":"Hello world"}
{"timestamp":"2026-03-16","source":"mic","duration_secs":3.0,"file":"mic_09-37-40.wav","text":"Testing"}
"#,
        )
        .unwrap();
        let transcripts = load_jsonl(&path).unwrap();
        assert_eq!(transcripts.len(), 2);
        assert_eq!(transcripts[0].text, "Hello world");
        assert_eq!(transcripts[1].text, "Testing");
    }

    #[test]
    fn test_load_jsonl_skips_invalid_lines() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bad.jsonl");
        std::fs::write(
            &path,
            r#"{"timestamp":"2026-03-16","source":"mic","duration_secs":5.0,"file":"mic_09-37-31.wav","text":"Good"}
not valid json
{"timestamp":"2026-03-16","source":"mic","duration_secs":3.0,"file":"mic_09-37-40.wav","text":"Also good"}
"#,
        )
        .unwrap();
        let transcripts = load_jsonl(&path).unwrap();
        assert_eq!(transcripts.len(), 2);
    }

    #[test]
    fn test_load_jsonl_empty_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("empty.jsonl");
        std::fs::write(&path, "").unwrap();
        let transcripts = load_jsonl(&path).unwrap();
        assert!(transcripts.is_empty());
    }
}
```

**Step 2: Run tests to verify they pass**

Run (PowerShell):
```powershell
cargo test --lib search::indexer::tests -- --nocapture
```
Expected: ALL indexer tests PASS.

**Step 3: Commit**

```
git add src/search/indexer.rs
git commit -m "feat(search): implement indexer pipeline with idempotent re-indexing"
```

---

## Task 8: Wire Up mod.rs (Remove Stubs)

**Files:**
- Modify: `src/search/mod.rs`

**Step 1: Update mod.rs to use real implementations**

Replace `src/search/mod.rs` with the final version (the one from Task 3 should already be correct since the stubs are now real implementations). Verify it compiles.

The `mod.rs` from Task 3 already calls `indexer::run_index`, `db::SearchDb::open`, and `embeddings::EmbeddingClient::from_config` which are all implemented now. No changes needed.

**Step 2: Verify full build**

Run (PowerShell):
```powershell
cargo check
```
Expected: compiles successfully.

**Step 3: Run all tests**

Run (PowerShell):
```powershell
cargo test
```
Expected: ALL tests pass (existing 123 + new chunker + db + embeddings + indexer tests).

**Step 4: Commit (if any changes needed)**

```
git add src/search/mod.rs
git commit -m "feat(search): finalize search module wiring"
```

---

## Task 9: Deploy text-embedding-3-large on Azure

**Files:** None (Azure CLI commands only)

**Step 1: Deploy the model**

Run from WSL:
```bash
az cognitiveservices account deployment create \
    --name varunroai \
    --resource-group varunr_test \
    --deployment-name text-embedding-3-large \
    --model-name text-embedding-3-large \
    --model-version "1" \
    --model-format OpenAI \
    --sku-capacity 120 \
    --sku-name Standard
```

**Step 2: Verify deployment**

Run from WSL:
```bash
az cognitiveservices account deployment list \
    --name varunroai \
    --resource-group varunr_test \
    --output table
```
Expected: Shows `text-embedding-3-large` deployment in `Succeeded` state.

**Step 3: Update deskmic.toml config**

Add to the production config at `C:\Users\varunramesh\AppData\Local\deskmic\deskmic.toml`:

```toml
[search]
embedding_deployment = "text-embedding-3-large"
chunk_gap_secs = 60
chunk_max_duration_secs = 300
```

No commit needed (config file is not in the repo).

---

## Task 10: End-to-End Test

**Step 1: Build release binary**

Run (PowerShell):
```powershell
$env:PATH = "C:\Users\varunramesh\.cargo\bin;C:\Program Files\CMake\bin;C:\Program Files\LLVM\bin;" + $env:PATH
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
cargo build --release
```

**Step 2: Run index**

Run (PowerShell):
```powershell
.\target\release\deskmic.exe -c C:\Users\varunramesh\AppData\Local\deskmic\deskmic.toml index
```
Expected: Output like `Indexed 47 new chunks from 21 transcripts (47 total chunks)`.

**Step 3: Run search**

Run (PowerShell):
```powershell
.\target\release\deskmic.exe -c C:\Users\varunramesh\AppData\Local\deskmic\deskmic.toml search "deployment pipeline"
```
Expected: Returns matching transcript chunks with scores.

**Step 4: Test JSON output**

Run (PowerShell):
```powershell
.\target\release\deskmic.exe -c C:\Users\varunramesh\AppData\Local\deskmic\deskmic.toml search "meeting" --json --limit 3
```
Expected: JSON array of results.

**Step 5: Test filters**

Run (PowerShell):
```powershell
.\target\release\deskmic.exe -c C:\Users\varunramesh\AppData\Local\deskmic\deskmic.toml search "test" --source mic --from 2026-03-15
```
Expected: Only mic results from March 15 onwards.

**Step 6: Test idempotency**

Run index again:
```powershell
.\target\release\deskmic.exe -c C:\Users\varunramesh\AppData\Local\deskmic\deskmic.toml index
```
Expected: `Indexed 0 new chunks from 0 transcripts (47 total chunks)` (all files unchanged).

---

## Task 11: Create PR

**Step 1: Commit any remaining changes**

Ensure all files are committed on the `feature/transcript-search` branch.

**Step 2: Push and create PR**

Run from WSL in the worktree:
```bash
git push -u origin feature/transcript-search
gh pr create --title "feat: transcript search with vector embeddings" --body "$(cat <<'EOF'
## Summary
- Adds `deskmic index` command: scans transcript JSONL files, chunks utterances by conversation gaps, embeds via Azure OpenAI text-embedding-3-large, stores in SQLite with sqlite-vec
- Adds `deskmic search "<query>"` command: embeds query, runs KNN vector search, supports --from/--to/--source/--limit/--json flags
- Idempotent indexing: tracks file mtimes, only re-indexes changed files
- New `[search]` config section with embedding_deployment, chunk_gap_secs, chunk_max_duration_secs

## Testing
- Unit tests for chunker (gap splitting, max duration, source boundaries, sort order)
- Integration tests for SearchDb (CRUD, vector search, filtering, idempotency)
- Unit tests for EmbeddingClient (config validation, serialization)
- Unit tests for indexer (JSONL loading)
- End-to-end tested against real transcripts
EOF
)"
```

**Step 3: Merge**

After checks pass:
```bash
gh pr merge --squash
```
