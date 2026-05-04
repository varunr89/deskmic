use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Copy `src` to `dst_dir/final_name` via a `.tmp` then atomic rename.
pub fn atomic_copy(src: &Path, dst_dir: &Path, final_name: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dst_dir)?;
    let final_path = dst_dir.join(final_name);
    let tmp_path = dst_dir.join(format!("{}.tmp", final_name));
    if tmp_path.exists() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    std::fs::copy(src, &tmp_path)
        .with_context(|| format!("copy {} -> {}", src.display(), tmp_path.display()))?;
    std::fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), final_path.display()))?;
    Ok(final_path)
}

/// Remove leftover `.tmp` files older than `max_age_secs` from a 2-deep tree
/// (recordings_root/date_dir/file).
pub fn cleanup_stale_tmps(root: &Path, max_age_secs: u64) -> Result<u32> {
    let mut count = 0;
    if !root.exists() {
        return Ok(0);
    }
    let now = std::time::SystemTime::now();
    for entry in walkdir_shallow(root)? {
        if entry.extension().and_then(|s| s.to_str()) == Some("tmp") {
            let meta = std::fs::metadata(&entry)?;
            if let Ok(modified) = meta.modified() {
                if let Ok(age) = now.duration_since(modified) {
                    if age.as_secs() > max_age_secs {
                        let _ = std::fs::remove_file(&entry);
                        count += 1;
                    }
                }
            }
        }
    }
    Ok(count)
}

fn walkdir_shallow(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for d in std::fs::read_dir(root)? {
        let d = d?;
        if d.file_type()?.is_dir() {
            for f in std::fs::read_dir(d.path())? {
                let f = f?;
                if f.file_type()?.is_file() {
                    out.push(f.path());
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn atomic_copy_writes_final_no_tmp() {
        let src_dir = TempDir::new().unwrap();
        let dst_dir = TempDir::new().unwrap();
        let src = src_dir.path().join("a.mp3");
        let mut f = std::fs::File::create(&src).unwrap();
        f.write_all(b"hello").unwrap();

        let out = atomic_copy(&src, dst_dir.path(), "recorder_X.mp3").unwrap();
        assert!(out.exists());
        assert!(!dst_dir.path().join("recorder_X.mp3.tmp").exists());
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
    }

    #[test]
    fn atomic_copy_overwrites_stale_tmp() {
        let src_dir = TempDir::new().unwrap();
        let dst_dir = TempDir::new().unwrap();
        let src = src_dir.path().join("a.mp3");
        std::fs::write(&src, b"data").unwrap();
        std::fs::write(dst_dir.path().join("recorder_X.mp3.tmp"), b"stale").unwrap();
        let out = atomic_copy(&src, dst_dir.path(), "recorder_X.mp3").unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"data");
    }
}
