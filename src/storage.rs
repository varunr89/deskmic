use std::path::Path;

use anyhow::Result;
use chrono::{Local, NaiveDate};

use crate::config::StorageConfig;

/// Deletes recording folders older than retention_days.
pub fn cleanup_old_recordings(recordings_dir: &Path, config: &StorageConfig) -> Result<u64> {
    let cutoff = Local::now().date_naive() - chrono::Duration::days(config.retention_days as i64);
    let mut bytes_freed: u64 = 0;

    if !recordings_dir.exists() {
        return Ok(0);
    }

    for entry in std::fs::read_dir(recordings_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if let Ok(folder_date) = NaiveDate::parse_from_str(&name_str, "%Y-%m-%d") {
            if folder_date < cutoff {
                let size = dir_size(&entry.path())?;
                std::fs::remove_dir_all(entry.path())?;
                bytes_freed += size;
                tracing::info!("Deleted old recordings: {} ({} bytes)", name_str, size);
            }
        }
    }
    Ok(bytes_freed)
}

/// Enforce max disk usage by deleting oldest folders first.
pub fn enforce_disk_limit(recordings_dir: &Path, max_bytes: u64) -> Result<()> {
    if !recordings_dir.exists() {
        return Ok(());
    }

    let current = dir_size(recordings_dir)?;
    if current <= max_bytes {
        return Ok(());
    }

    let mut folders: Vec<(NaiveDate, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(recordings_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Ok(date) = NaiveDate::parse_from_str(&name_str, "%Y-%m-%d") {
            folders.push((date, entry.path()));
        }
    }
    folders.sort_by_key(|(date, _)| *date);

    let mut remaining = current;
    for (date, path) in folders {
        if remaining <= max_bytes {
            break;
        }
        let size = dir_size(&path)?;
        std::fs::remove_dir_all(&path)?;
        remaining -= size;
        tracing::info!("Deleted {} to free space ({} bytes)", date, size);
    }
    Ok(())
}

fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                total += metadata.len();
            } else if metadata.is_dir() {
                total += dir_size(&entry.path())?;
            }
        }
    }
    Ok(total)
}

/// Returns (total_files, total_bytes) for the recordings directory.
pub fn get_storage_stats(recordings_dir: &Path) -> Result<(usize, u64)> {
    let mut count = 0;
    let mut bytes = 0;

    if !recordings_dir.exists() {
        return Ok((0, 0));
    }

    for entry in std::fs::read_dir(recordings_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            for file in std::fs::read_dir(entry.path())? {
                let file = file?;
                if file.file_type()?.is_file() {
                    count += 1;
                    bytes += file.metadata()?.len();
                }
            }
        }
    }
    Ok((count, bytes))
}

/// Run cleanup loop on a dedicated thread.
pub fn run_cleanup_loop(
    recordings_dir: std::path::PathBuf,
    config: StorageConfig,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let interval = std::time::Duration::from_secs(config.cleanup_interval_hours as u64 * 3600);

    run_cleanup_once(&recordings_dir, &config);

    while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
        let start = std::time::Instant::now();
        while start.elapsed() < interval && !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
        if !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            run_cleanup_once(&recordings_dir, &config);
        }
    }
}

fn run_cleanup_once(recordings_dir: &Path, config: &StorageConfig) {
    if let Err(e) = cleanup_old_recordings(recordings_dir, config) {
        tracing::error!("Cleanup error: {:?}", e);
    }
    if let Some(max_gb) = config.max_disk_usage_gb {
        let max_bytes = (max_gb * 1_073_741_824.0) as u64;
        if let Err(e) = enforce_disk_limit(recordings_dir, max_bytes) {
            tracing::error!("Disk limit enforcement error: {:?}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_config(retention_days: u32) -> StorageConfig {
        StorageConfig {
            retention_days,
            cleanup_interval_hours: 24,
            max_disk_usage_gb: None,
        }
    }

    /// Helper: create a date-named folder with a file of known size.
    fn create_date_folder(base: &Path, date: NaiveDate, data: &[u8]) -> std::path::PathBuf {
        let dir = base.join(date.format("%Y-%m-%d").to_string());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("test.wav"), data).unwrap();
        dir
    }

    #[test]
    fn test_cleanup_old_recordings() {
        let tmp = TempDir::new().unwrap();

        let old_date = Local::now().date_naive() - chrono::Duration::days(35);
        let old_dir = create_date_folder(tmp.path(), old_date, b"fake audio data");

        let recent_date = Local::now().date_naive();
        let recent_dir = create_date_folder(tmp.path(), recent_date, b"fake audio data");

        let config = make_config(30);
        let freed = cleanup_old_recordings(tmp.path(), &config).unwrap();

        assert!(!old_dir.exists(), "Old folder should be deleted");
        assert!(recent_dir.exists(), "Recent folder should be kept");
        assert_eq!(freed, b"fake audio data".len() as u64);
    }

    #[test]
    fn test_cleanup_nonexistent_directory_returns_zero() {
        let tmp = TempDir::new().unwrap();
        let nonexistent = tmp.path().join("does_not_exist");
        let config = make_config(30);
        let freed = cleanup_old_recordings(&nonexistent, &config).unwrap();
        assert_eq!(freed, 0);
    }

    #[test]
    fn test_cleanup_ignores_non_date_folders() {
        let tmp = TempDir::new().unwrap();

        // Create a folder that doesn't match the date pattern
        let weird_dir = tmp.path().join("not-a-date");
        fs::create_dir_all(&weird_dir).unwrap();
        fs::write(weird_dir.join("file.txt"), b"data").unwrap();

        let config = make_config(0); // zero retention = delete everything with a date
        cleanup_old_recordings(tmp.path(), &config).unwrap();

        assert!(weird_dir.exists(), "Non-date folders should be untouched");
    }

    #[test]
    fn test_cleanup_boundary_date_is_kept() {
        let tmp = TempDir::new().unwrap();

        // Folder exactly at the cutoff boundary (retention_days ago) should NOT be deleted.
        // cutoff = today - retention_days, and we only delete if folder_date < cutoff.
        let boundary_date = Local::now().date_naive() - chrono::Duration::days(30);
        let boundary_dir = create_date_folder(tmp.path(), boundary_date, b"boundary data");

        let config = make_config(30);
        cleanup_old_recordings(tmp.path(), &config).unwrap();

        assert!(
            boundary_dir.exists(),
            "Folder exactly at cutoff should be kept (not strictly less than)"
        );
    }

    #[test]
    fn test_enforce_disk_limit_deletes_oldest_first() {
        let tmp = TempDir::new().unwrap();

        let date1 = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let date2 = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
        let date3 = NaiveDate::from_ymd_opt(2025, 1, 3).unwrap();

        let dir1 = create_date_folder(tmp.path(), date1, &[0u8; 100]);
        let dir2 = create_date_folder(tmp.path(), date2, &[0u8; 100]);
        let dir3 = create_date_folder(tmp.path(), date3, &[0u8; 100]);

        // Total = 300 bytes, limit to 150 => must delete oldest until under 150
        enforce_disk_limit(tmp.path(), 150).unwrap();

        assert!(!dir1.exists(), "Oldest folder should be deleted first");
        assert!(!dir2.exists(), "Second oldest should also be deleted");
        assert!(dir3.exists(), "Newest folder should remain");
    }

    #[test]
    fn test_enforce_disk_limit_no_op_when_under() {
        let tmp = TempDir::new().unwrap();

        let date1 = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        let dir1 = create_date_folder(tmp.path(), date1, &[0u8; 50]);

        // Well under limit
        enforce_disk_limit(tmp.path(), 10000).unwrap();

        assert!(dir1.exists(), "Should not delete anything when under limit");
    }

    #[test]
    fn test_enforce_disk_limit_nonexistent_dir() {
        let tmp = TempDir::new().unwrap();
        let nonexistent = tmp.path().join("nope");
        let result = enforce_disk_limit(&nonexistent, 100);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_storage_stats_counts_files_and_bytes() {
        let tmp = TempDir::new().unwrap();

        let date1 = NaiveDate::from_ymd_opt(2025, 3, 1).unwrap();
        let date2 = NaiveDate::from_ymd_opt(2025, 3, 2).unwrap();

        let dir1 = create_date_folder(tmp.path(), date1, &[0u8; 100]);
        // Add a second file in dir1
        fs::write(dir1.join("extra.wav"), &[0u8; 50]).unwrap();

        create_date_folder(tmp.path(), date2, &[0u8; 200]);

        let (count, bytes) = get_storage_stats(tmp.path()).unwrap();
        assert_eq!(count, 3, "Should count 3 files total");
        assert_eq!(bytes, 350, "Should sum to 350 bytes");
    }

    #[test]
    fn test_get_storage_stats_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let (count, bytes) = get_storage_stats(tmp.path()).unwrap();
        assert_eq!(count, 0);
        assert_eq!(bytes, 0);
    }

    #[test]
    fn test_get_storage_stats_nonexistent_dir() {
        let tmp = TempDir::new().unwrap();
        let nonexistent = tmp.path().join("nonexistent");
        let (count, bytes) = get_storage_stats(&nonexistent).unwrap();
        assert_eq!(count, 0);
        assert_eq!(bytes, 0);
    }

    #[test]
    fn test_dir_size_recursive() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(tmp.path().join("a.txt"), &[0u8; 10]).unwrap();
        fs::write(sub.join("b.txt"), &[0u8; 20]).unwrap();

        let size = dir_size(tmp.path()).unwrap();
        assert_eq!(size, 30);
    }

    #[test]
    fn test_run_cleanup_once_integrates_both_cleanups() {
        let tmp = TempDir::new().unwrap();

        // Create an old folder (40 days old)
        let old_date = Local::now().date_naive() - chrono::Duration::days(40);
        let old_dir = create_date_folder(tmp.path(), old_date, &[0u8; 500]);

        // Create a recent folder
        let recent_date = Local::now().date_naive();
        let recent_dir = create_date_folder(tmp.path(), recent_date, &[0u8; 100]);

        let config = StorageConfig {
            retention_days: 30,
            cleanup_interval_hours: 24,
            max_disk_usage_gb: None,
        };

        run_cleanup_once(tmp.path(), &config);

        assert!(!old_dir.exists(), "Old folder should be cleaned up");
        assert!(recent_dir.exists(), "Recent folder should remain");
    }
}
