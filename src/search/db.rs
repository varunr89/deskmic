use crate::search::chunker::Chunk;
use crate::search::{SearchParams, SearchResult};
use anyhow::{Context, Result};
use rusqlite::{ffi::sqlite3_auto_extension, params, Connection};
use std::path::Path;

/// Dimensionality of the embedding vectors (text-embedding-3-large).
pub const EMBEDDING_DIM: usize = 3072;

/// Persistent search database backed by SQLite + sqlite-vec.
pub struct SearchDb {
    conn: Connection,
}

impl SearchDb {
    /// Open (or create) a search database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        // Register sqlite-vec as an auto-extension before opening.
        unsafe {
            sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        let conn = Connection::open(path)
            .with_context(|| format!("failed to open search db at {}", path.display()))?;

        conn.execute_batch("PRAGMA journal_mode = WAL;")
            .context("failed to set WAL mode")?;

        Self::init_schema(&conn)?;

        Ok(Self { conn })
    }

    /// Open an in-memory database for tests.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        unsafe {
            sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        let conn = Connection::open_in_memory().context("failed to open in-memory db")?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    /// Create all required tables and indexes.
    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS indexed_files (
                file_name   TEXT PRIMARY KEY,
                modified_at INTEGER NOT NULL,
                indexed_at  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chunks (
                id             TEXT PRIMARY KEY,
                date           TEXT NOT NULL,
                source         TEXT NOT NULL,
                start_time     TEXT NOT NULL,
                end_time       TEXT NOT NULL,
                duration_secs  REAL NOT NULL,
                text           TEXT NOT NULL,
                files          TEXT NOT NULL,
                embedding      BLOB NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_chunks_date   ON chunks(date);
            CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source);",
        )
        .context("failed to create core tables")?;

        // vec0 virtual tables do not support IF NOT EXISTS — check sqlite_master.
        let vec_table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='vec_chunks'",
                [],
                |row| row.get(0),
            )
            .context("failed to check for vec_chunks table")?;

        if !vec_table_exists {
            conn.execute_batch(&format!(
                "CREATE VIRTUAL TABLE vec_chunks USING vec0(embedding float[{}])",
                EMBEDDING_DIM
            ))
            .context("failed to create vec_chunks virtual table")?;
        }

        Ok(())
    }

    // ── File-level mtime tracking ─────────────────────────────────────

    /// Get the last-known modified time of a file, or `None` if never indexed.
    pub fn get_file_mtime(&self, file_name: &str) -> Result<Option<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT modified_at FROM indexed_files WHERE file_name = ?1")?;
        let mut rows = stmt.query(params![file_name])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// Record (or update) the modified time and current timestamp for a file.
    pub fn set_file_mtime(&self, file_name: &str, modified_at: i64) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO indexed_files (file_name, modified_at, indexed_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(file_name) DO UPDATE SET modified_at = ?2, indexed_at = ?3",
            params![file_name, modified_at, now],
        )?;
        Ok(())
    }

    // ── Chunk CRUD ────────────────────────────────────────────────────

    /// Delete all chunks (and their vectors) for a given date.
    /// Returns the number of chunks deleted.
    pub fn delete_chunks_for_date(&self, date: &str) -> Result<usize> {
        // First collect the rowids of chunks to delete from the vec table.
        let mut stmt = self
            .conn
            .prepare("SELECT rowid FROM chunks WHERE date = ?1")?;
        let rowids: Vec<i64> = stmt
            .query_map(params![date], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        for rowid in &rowids {
            self.conn
                .execute("DELETE FROM vec_chunks WHERE rowid = ?1", params![rowid])?;
        }

        let deleted = self
            .conn
            .execute("DELETE FROM chunks WHERE date = ?1", params![date])?;

        Ok(deleted)
    }

    /// Insert a single chunk and its embedding. Returns the rowid.
    pub fn insert_chunk(&self, chunk: &Chunk, embedding: &[f32]) -> Result<i64> {
        let files_json = serde_json::to_string(&chunk.files)?;
        let emb_bytes = embedding_to_bytes(embedding);

        self.conn.execute(
            "INSERT OR REPLACE INTO chunks
                (id, date, source, start_time, end_time, duration_secs, text, files, embedding)
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
                emb_bytes,
            ],
        )?;

        let rowid: i64 = self.conn.query_row(
            "SELECT rowid FROM chunks WHERE id = ?1",
            params![chunk.id],
            |row| row.get(0),
        )?;

        self.conn.execute(
            "INSERT OR REPLACE INTO vec_chunks (rowid, embedding) VALUES (?1, ?2)",
            params![rowid, emb_bytes],
        )?;

        Ok(rowid)
    }

    /// Batch-insert chunks with embeddings inside a single transaction.
    pub fn insert_chunks(&self, chunks_with_embeddings: &[(&Chunk, &[f32])]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        for (chunk, embedding) in chunks_with_embeddings {
            let files_json = serde_json::to_string(&chunk.files)?;
            let emb_bytes = embedding_to_bytes(embedding);

            tx.execute(
                "INSERT OR REPLACE INTO chunks
                    (id, date, source, start_time, end_time, duration_secs, text, files, embedding)
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
                    emb_bytes,
                ],
            )?;

            let rowid: i64 = tx.query_row(
                "SELECT rowid FROM chunks WHERE id = ?1",
                params![chunk.id],
                |row| row.get(0),
            )?;

            tx.execute(
                "INSERT OR REPLACE INTO vec_chunks (rowid, embedding) VALUES (?1, ?2)",
                params![rowid, emb_bytes],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Count the total number of chunks in the database.
    pub fn count_chunks(&self) -> Result<usize> {
        let count: usize = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        Ok(count)
    }

    // ── Search ────────────────────────────────────────────────────────

    /// Run a KNN search against the vector index, then post-filter by metadata.
    pub fn search(
        &self,
        query_embedding: &[f32],
        params: &SearchParams,
    ) -> Result<Vec<SearchResult>> {
        let candidate_limit = params.limit * 3;
        let emb_bytes = embedding_to_bytes(query_embedding);

        let mut stmt = self.conn.prepare(
            "SELECT rowid, distance
             FROM vec_chunks
             WHERE embedding MATCH ?1
             ORDER BY distance
             LIMIT ?2",
        )?;

        let candidates: Vec<(i64, f32)> = stmt
            .query_map(rusqlite::params![emb_bytes, candidate_limit], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut results = Vec::new();

        for (rowid, distance) in candidates {
            if results.len() >= params.limit {
                break;
            }

            let row = self.conn.query_row(
                "SELECT date, source, start_time, end_time, text, files
                 FROM chunks
                 WHERE rowid = ?1",
                rusqlite::params![rowid],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            );

            let (date, source, start_time, end_time, text, files_json) = match row {
                Ok(r) => r,
                Err(_) => continue, // orphan vec entry
            };

            // Post-filter: date range
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

            // Post-filter: source
            if let Some(ref src) = params.source {
                if source != *src {
                    continue;
                }
            }

            let files: Vec<String> = serde_json::from_str(&files_json).unwrap_or_default();
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
        }

        Ok(results)
    }
}

/// Convert an f32 embedding slice to little-endian bytes for sqlite-vec.
fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(embedding.len() * 4);
    for &val in embedding {
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    bytes
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::chunker::Chunk;

    fn make_test_embedding(seed: f32) -> Vec<f32> {
        let mut emb = vec![0.0f32; EMBEDDING_DIM];
        for (i, val) in emb.iter_mut().enumerate() {
            *val = ((i as f32 + seed) * 0.001).sin();
        }
        emb
    }

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

    #[test]
    fn test_open_in_memory() {
        let db = SearchDb::open_in_memory().unwrap();
        assert_eq!(db.count_chunks().unwrap(), 0);
    }

    #[test]
    fn test_file_mtime_not_indexed() {
        let db = SearchDb::open_in_memory().unwrap();
        assert_eq!(db.get_file_mtime("nonexistent.json").unwrap(), None);
    }

    #[test]
    fn test_file_mtime_roundtrip() {
        let db = SearchDb::open_in_memory().unwrap();
        db.set_file_mtime("transcript.json", 1234567890).unwrap();
        assert_eq!(
            db.get_file_mtime("transcript.json").unwrap(),
            Some(1234567890)
        );
    }

    #[test]
    fn test_file_mtime_update() {
        let db = SearchDb::open_in_memory().unwrap();
        db.set_file_mtime("transcript.json", 100).unwrap();
        db.set_file_mtime("transcript.json", 200).unwrap();
        assert_eq!(db.get_file_mtime("transcript.json").unwrap(), Some(200));
    }

    #[test]
    fn test_insert_and_count_chunk() {
        let db = SearchDb::open_in_memory().unwrap();
        let chunk = make_test_chunk("c1", "2026-03-16", "mic", "hello world");
        let emb = make_test_embedding(1.0);
        db.insert_chunk(&chunk, &emb).unwrap();
        assert_eq!(db.count_chunks().unwrap(), 1);
    }

    #[test]
    fn test_insert_multiple_chunks() {
        let db = SearchDb::open_in_memory().unwrap();
        let c1 = make_test_chunk("c1", "2026-03-16", "mic", "first chunk");
        let c2 = make_test_chunk("c2", "2026-03-16", "teams", "second chunk");
        let e1 = make_test_embedding(1.0);
        let e2 = make_test_embedding(2.0);

        db.insert_chunks(&[(&c1, &e1), (&c2, &e2)]).unwrap();
        assert_eq!(db.count_chunks().unwrap(), 2);
    }

    #[test]
    fn test_delete_chunks_for_date() {
        let db = SearchDb::open_in_memory().unwrap();
        let c1 = make_test_chunk("c1", "2026-03-16", "mic", "day one");
        let c2 = make_test_chunk("c2", "2026-03-17", "mic", "day two");
        let e1 = make_test_embedding(1.0);
        let e2 = make_test_embedding(2.0);

        db.insert_chunks(&[(&c1, &e1), (&c2, &e2)]).unwrap();
        assert_eq!(db.count_chunks().unwrap(), 2);

        let deleted = db.delete_chunks_for_date("2026-03-16").unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(db.count_chunks().unwrap(), 1);
    }

    #[test]
    fn test_search_returns_results() {
        let db = SearchDb::open_in_memory().unwrap();
        let chunk = make_test_chunk("c1", "2026-03-16", "mic", "important meeting notes");
        let emb = make_test_embedding(1.0);
        db.insert_chunk(&chunk, &emb).unwrap();

        let params = SearchParams {
            query: "meeting".to_string(),
            from: None,
            to: None,
            source: None,
            limit: 10,
        };

        let results = db.search(&emb, &params).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "important meeting notes");
        // Same vector should yield high score (distance ~0, score ~1).
        assert!(results[0].score > 0.99, "score was {}", results[0].score);
    }

    #[test]
    fn test_search_date_filter() {
        let db = SearchDb::open_in_memory().unwrap();
        let c1 = make_test_chunk("c1", "2026-03-15", "mic", "old stuff");
        let c2 = make_test_chunk("c2", "2026-03-17", "mic", "new stuff");
        let emb = make_test_embedding(1.0);

        db.insert_chunks(&[(&c1, &emb), (&c2, &emb)]).unwrap();

        let params = SearchParams {
            query: "stuff".to_string(),
            from: Some("2026-03-16".to_string()),
            to: None,
            source: None,
            limit: 10,
        };

        let results = db.search(&emb, &params).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].date, "2026-03-17");
    }

    #[test]
    fn test_search_source_filter() {
        let db = SearchDb::open_in_memory().unwrap();
        let c1 = make_test_chunk("c1", "2026-03-16", "mic", "mic audio");
        let c2 = make_test_chunk("c2", "2026-03-16", "teams", "teams audio");
        let emb = make_test_embedding(1.0);

        db.insert_chunks(&[(&c1, &emb), (&c2, &emb)]).unwrap();

        let params = SearchParams {
            query: "audio".to_string(),
            from: None,
            to: None,
            source: Some("teams".to_string()),
            limit: 10,
        };

        let results = db.search(&emb, &params).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "teams");
    }

    #[test]
    fn test_search_limit() {
        let db = SearchDb::open_in_memory().unwrap();

        // Insert 5 chunks.
        let mut pairs = Vec::new();
        let chunks: Vec<Chunk> = (0..5)
            .map(|i| {
                make_test_chunk(
                    &format!("c{}", i),
                    "2026-03-16",
                    "mic",
                    &format!("chunk {}", i),
                )
            })
            .collect();
        let embeddings: Vec<Vec<f32>> = (0..5).map(|i| make_test_embedding(i as f32)).collect();

        for i in 0..5 {
            pairs.push((&chunks[i], embeddings[i].as_slice()));
        }
        db.insert_chunks(&pairs).unwrap();

        let query_emb = make_test_embedding(0.0);
        let params = SearchParams {
            query: "chunk".to_string(),
            from: None,
            to: None,
            source: None,
            limit: 2,
        };

        let results = db.search(&query_emb, &params).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_idempotent_reindex() {
        let db = SearchDb::open_in_memory().unwrap();
        let chunk = make_test_chunk("c1", "2026-03-16", "mic", "reindexable");
        let emb = make_test_embedding(1.0);

        // First index pass.
        db.insert_chunk(&chunk, &emb).unwrap();
        assert_eq!(db.count_chunks().unwrap(), 1);

        // Simulate re-index: delete + re-insert.
        db.delete_chunks_for_date("2026-03-16").unwrap();
        assert_eq!(db.count_chunks().unwrap(), 0);

        db.insert_chunk(&chunk, &emb).unwrap();
        assert_eq!(db.count_chunks().unwrap(), 1);
    }
}
