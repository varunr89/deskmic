pub mod chunk;
pub mod cleanup;
pub mod copy;
pub mod decode;
pub mod detect;
pub mod filename;
pub mod registry;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{Local, TimeZone};

use crate::config::Config;

pub struct IngestOptions {
    pub dry_run: bool,
    pub retry_failed: bool,
}

pub struct IngestSummary {
    pub considered: usize,
    pub ingested: usize,
    pub skipped_known: usize,
    pub failed: usize,
    pub deleted_from_device: usize,
}

pub fn run(config: &Config, opts: IngestOptions) -> Result<IngestSummary> {
    let mut summary = IngestSummary {
        considered: 0,
        ingested: 0,
        skipped_known: 0,
        failed: 0,
        deleted_from_device: 0,
    };

    if !config.recorder.enabled {
        tracing::info!("recorder ingestion disabled in config");
        return Ok(summary);
    }

    let device_root = match detect::find_volume_by_label(&config.recorder.volume_label) {
        Some(p) => p,
        None => {
            tracing::info!(
                "recorder not connected (label '{}' not found)",
                config.recorder.volume_label
            );
            return Ok(summary);
        }
    };

    let recordings_dir = &config.output.directory;
    let _ = copy::cleanup_stale_tmps(recordings_dir, 3600);

    let device_dir = device_root.join(
        config
            .recorder
            .device_subpath
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    );
    if !device_dir.exists() {
        tracing::info!("recorder folder missing: {}", device_dir.display());
        return Ok(summary);
    }

    let db_path = recorder_db_path(recordings_dir);
    let conn = registry::open(&db_path)?;

    for entry in std::fs::read_dir(&device_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !(name.ends_with(".mp3") || name.ends_with(".MP3")) {
            continue;
        }

        let parsed = match filename::parse(&name) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("skipping unrecognized filename {}: {}", name, e);
                continue;
            }
        };
        let meta = std::fs::metadata(&path)?;
        let size = meta.len() as i64;
        let mtime = meta
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        summary.considered += 1;

        if registry::is_known(&conn, &name, size, mtime)? {
            if !opts.retry_failed {
                summary.skipped_known += 1;
                continue;
            }
            summary.skipped_known += 1;
            continue;
        }

        if opts.dry_run {
            tracing::info!("[dry-run] would ingest: {}", name);
            continue;
        }

        match ingest_one(&conn, &path, &name, size, mtime, &parsed, recordings_dir, config) {
            Ok(_) => summary.ingested += 1,
            Err(e) => {
                tracing::error!("ingest failed for {}: {:?}", name, e);
                let _ = registry::insert(
                    &conn,
                    &registry::IngestRow {
                        device_filename: name.clone(),
                        device_size: size,
                        device_mtime: mtime,
                        local_path: String::new(),
                        start_ts: parsed.start.and_utc().timestamp(),
                        ingested_at: Local::now().timestamp(),
                        transcribed_at: None,
                        status: "failed".to_string(),
                        error_message: Some(format!("{e:?}")),
                    },
                );
                summary.failed += 1;
            }
        }
    }

    summary.deleted_from_device =
        cleanup::run(&conn, &device_dir, config.recorder.device_retention_days)?;

    Ok(summary)
}

fn ingest_one(
    conn: &rusqlite::Connection,
    src: &Path,
    name: &str,
    size: i64,
    mtime: i64,
    parsed: &filename::ParsedRecorderName,
    recordings_dir: &Path,
    config: &Config,
) -> Result<()> {
    let date = parsed.start.date();
    let date_dir = chunk::date_dir_for(date, recordings_dir);
    std::fs::create_dir_all(&date_dir)?;

    let hhmmss = parsed.start.format("%H%M%S").to_string();

    let final_mp3_name = format!("recorder_{}.mp3", hhmmss);
    let local_mp3 = copy::atomic_copy(src, &date_dir, &final_mp3_name)?;

    let audio = decode::decode_mp3(&local_mp3)
        .with_context(|| format!("decode {}", local_mp3.display()))?;
    let trimmed = chunk::vad_trim(
        &audio.samples,
        config.recorder.vad_silence_hangover_ms,
        0.02,
    );

    let plans = chunk::plan_chunks(&trimmed, config.recorder.chunk_target_minutes * 60);
    for (i, plan) in plans.iter().enumerate() {
        let base = chunk::chunk_basename(&hhmmss, i as u32);
        let wav_path = date_dir.join(format!("{}.wav", base));
        let json_path = date_dir.join(format!("{}.json", base));
        chunk::write_chunk_wav(&trimmed[plan.start_sample..plan.end_sample], &wav_path)?;
        chunk::write_sidecar(
            &json_path,
            &chunk::ChunkSidecar {
                recording_id: name.to_string(),
                chunk_index: i as u32,
                base_offset_secs: plan.base_offset_secs,
            },
        )?;
    }

    let row = registry::IngestRow {
        device_filename: name.to_string(),
        device_size: size,
        device_mtime: mtime,
        local_path: local_mp3.to_string_lossy().to_string(),
        start_ts: Local
            .from_local_datetime(&parsed.start)
            .single()
            .map(|dt| dt.timestamp())
            .unwrap_or_else(|| parsed.start.and_utc().timestamp()),
        ingested_at: Local::now().timestamp(),
        transcribed_at: None,
        status: "ok".to_string(),
        error_message: None,
    };
    registry::insert(conn, &row)?;
    Ok(())
}

pub fn recorder_db_path(recordings_dir: &Path) -> PathBuf {
    recordings_dir.join("deskmic-search.db")
}

#[doc(hidden)]
pub fn ingest_one_for_test(
    conn: &rusqlite::Connection,
    src: &Path,
    name: &str,
    size: i64,
    mtime: i64,
    parsed: &filename::ParsedRecorderName,
    recordings_dir: &Path,
    config: &Config,
) -> Result<()> {
    ingest_one(conn, src, name, size, mtime, parsed, recordings_dir, config)
}
