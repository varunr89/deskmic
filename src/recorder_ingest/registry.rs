use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestRow {
    pub device_filename: String,
    pub device_size: i64,
    pub device_mtime: i64,
    pub local_path: String,
    pub start_ts: i64,
    pub ingested_at: i64,
    pub transcribed_at: Option<i64>,
    pub status: String,
    pub error_message: Option<String>,
}

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS recorder_ingest (
            device_filename TEXT    NOT NULL,
            device_size     INTEGER NOT NULL,
            device_mtime    INTEGER NOT NULL,
            local_path      TEXT    NOT NULL,
            start_ts        INTEGER NOT NULL,
            ingested_at     INTEGER NOT NULL,
            transcribed_at  INTEGER,
            status          TEXT    NOT NULL DEFAULT 'ok',
            error_message   TEXT,
            PRIMARY KEY (device_filename, device_size, device_mtime)
         );",
    )?;
    Ok(())
}

pub fn open(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    ensure_schema(&conn)?;
    Ok(conn)
}

pub fn is_known(conn: &Connection, filename: &str, size: i64, mtime: i64) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM recorder_ingest
         WHERE device_filename=?1 AND device_size=?2 AND device_mtime=?3",
        params![filename, size, mtime],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub fn insert(conn: &Connection, row: &IngestRow) -> Result<()> {
    conn.execute(
        "INSERT INTO recorder_ingest (device_filename, device_size, device_mtime,
            local_path, start_ts, ingested_at, transcribed_at, status, error_message)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            row.device_filename,
            row.device_size,
            row.device_mtime,
            row.local_path,
            row.start_ts,
            row.ingested_at,
            row.transcribed_at,
            row.status,
            row.error_message,
        ],
    )?;
    Ok(())
}

pub fn mark_failed(conn: &Connection, filename: &str, size: i64, mtime: i64, err: &str) -> Result<()> {
    conn.execute(
        "UPDATE recorder_ingest SET status='failed', error_message=?4
         WHERE device_filename=?1 AND device_size=?2 AND device_mtime=?3",
        params![filename, size, mtime, err],
    )?;
    Ok(())
}

pub fn mark_transcribed(conn: &Connection, recording_id: &str, ts: i64) -> Result<()> {
    conn.execute(
        "UPDATE recorder_ingest SET transcribed_at=?2
         WHERE device_filename=?1 AND transcribed_at IS NULL",
        params![recording_id, ts],
    )?;
    Ok(())
}

pub fn rows_eligible_for_device_cleanup(
    conn: &Connection,
    older_than_unix: i64,
) -> Result<Vec<IngestRow>> {
    let mut stmt = conn.prepare(
        "SELECT device_filename, device_size, device_mtime, local_path, start_ts,
                ingested_at, transcribed_at, status, error_message
         FROM recorder_ingest
         WHERE ingested_at < ?1 AND transcribed_at IS NOT NULL AND status='ok'",
    )?;
    let rows = stmt
        .query_map(params![older_than_unix], |r| {
            Ok(IngestRow {
                device_filename: r.get(0)?,
                device_size: r.get(1)?,
                device_mtime: r.get(2)?,
                local_path: r.get(3)?,
                start_ts: r.get(4)?,
                ingested_at: r.get(5)?,
                transcribed_at: r.get(6)?,
                status: r.get(7)?,
                error_message: r.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_row(name: &str, size: i64, mtime: i64) -> IngestRow {
        IngestRow {
            device_filename: name.to_string(),
            device_size: size,
            device_mtime: mtime,
            local_path: format!("recordings/2026-04-29/recorder_{}.mp3", name),
            start_ts: 1_700_000_000,
            ingested_at: 1_700_000_100,
            transcribed_at: None,
            status: "ok".to_string(),
            error_message: None,
        }
    }

    #[test]
    fn insert_and_lookup() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("test.db");
        let conn = open(&db).unwrap();
        let r = make_row("260429_0909.mp3", 100, 1234);
        insert(&conn, &r).unwrap();
        assert!(is_known(&conn, "260429_0909.mp3", 100, 1234).unwrap());
        assert!(!is_known(&conn, "260429_0909.mp3", 100, 9999).unwrap());
        assert!(!is_known(&conn, "260429_0909.mp3", 999, 1234).unwrap());
    }

    #[test]
    fn mark_transcribed_sets_timestamp() {
        let dir = TempDir::new().unwrap();
        let conn = open(&dir.path().join("t.db")).unwrap();
        insert(&conn, &make_row("a.mp3", 1, 1)).unwrap();
        mark_transcribed(&conn, "a.mp3", 1_700_000_500).unwrap();
        let rows = rows_eligible_for_device_cleanup(&conn, 1_800_000_000).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].transcribed_at, Some(1_700_000_500));
    }

    #[test]
    fn cleanup_filter_excludes_recent_or_untranscribed() {
        let dir = TempDir::new().unwrap();
        let conn = open(&dir.path().join("t.db")).unwrap();

        // old.mp3: ingested long ago, transcribed → eligible
        let mut old = make_row("old.mp3", 1, 1);
        old.ingested_at = 1_700_000_000;
        insert(&conn, &old).unwrap();
        mark_transcribed(&conn, "old.mp3", 1_700_000_500).unwrap();

        // new.mp3: ingested AFTER the cutoff, transcribed → NOT eligible (too recent)
        let mut new = make_row("new.mp3", 2, 2);
        new.ingested_at = 1_900_000_000;
        insert(&conn, &new).unwrap();
        mark_transcribed(&conn, "new.mp3", 1_900_000_000).unwrap();

        // notdone.mp3: ingested long ago but never transcribed → NOT eligible
        let mut nd = make_row("notdone.mp3", 3, 3);
        nd.ingested_at = 1_700_000_000;
        insert(&conn, &nd).unwrap();

        // cutoff: 1_800_000_000 — only old.mp3 qualifies
        let rows = rows_eligible_for_device_cleanup(&conn, 1_800_000_000).unwrap();
        let names: Vec<_> = rows.iter().map(|r| r.device_filename.as_str()).collect();
        assert_eq!(names, vec!["old.mp3"]);
    }

    #[test]
    fn mark_failed_persists_error() {
        let dir = TempDir::new().unwrap();
        let conn = open(&dir.path().join("t.db")).unwrap();
        insert(&conn, &make_row("a.mp3", 1, 1)).unwrap();
        mark_failed(&conn, "a.mp3", 1, 1, "decode error").unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recorder_ingest WHERE status='failed' AND error_message='decode error'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}
