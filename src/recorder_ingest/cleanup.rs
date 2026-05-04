use std::path::Path;

use anyhow::Result;
use chrono::Local;
use rusqlite::Connection;

use super::registry;

pub fn run(conn: &Connection, device_dir: &Path, retention_days: u32) -> Result<usize> {
    let cutoff = Local::now().timestamp() - (retention_days as i64) * 86_400;
    let rows = registry::rows_eligible_for_device_cleanup(conn, cutoff)?;

    let mut deleted = 0;
    for row in rows {
        let path = device_dir.join(&row.device_filename);
        if !path.exists() {
            continue;
        }
        let meta = std::fs::metadata(&path)?;
        let size = meta.len() as i64;
        let mtime = meta
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if size != row.device_size || mtime != row.device_mtime {
            tracing::warn!(
                "skipping device cleanup of {}: size/mtime changed",
                row.device_filename
            );
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!("deleted from device: {}", row.device_filename);
                deleted += 1;
            }
            Err(e) => tracing::warn!("device delete failed for {}: {}", row.device_filename, e),
        }
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder_ingest::registry::{insert, mark_transcribed, open, IngestRow};
    use std::io::Write;
    use tempfile::TempDir;

    fn touch(path: &std::path::Path, bytes: &[u8]) -> (i64, i64) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(bytes).unwrap();
        let m = std::fs::metadata(path).unwrap();
        let mtime = m
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        (m.len() as i64, mtime)
    }

    #[test]
    fn deletes_only_old_transcribed_with_matching_metadata() {
        let dev = TempDir::new().unwrap();
        let db = TempDir::new().unwrap();
        let conn = open(&db.path().join("t.db")).unwrap();

        let (sz, mt) = touch(&dev.path().join("old.mp3"), b"old");
        insert(
            &conn,
            &IngestRow {
                device_filename: "old.mp3".into(),
                device_size: sz,
                device_mtime: mt,
                local_path: "x".into(),
                start_ts: 0,
                ingested_at: chrono::Local::now().timestamp() - 8 * 86400,
                transcribed_at: None,
                status: "ok".into(),
                error_message: None,
            },
        )
        .unwrap();
        mark_transcribed(&conn, "old.mp3", chrono::Local::now().timestamp() - 8 * 86400).unwrap();

        let n = run(&conn, dev.path(), 7).unwrap();
        assert_eq!(n, 1);
        assert!(!dev.path().join("old.mp3").exists());
    }
}
