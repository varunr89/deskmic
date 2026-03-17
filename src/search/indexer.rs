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
        println!(
            "No transcripts directory found at {}",
            transcript_dir.display()
        );
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

        tracing::info!("Indexed {} chunks from {}", chunks.len(), file_name);
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
