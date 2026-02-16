use anyhow::Result;

/// Add a shortcut to the Windows Startup folder.
#[cfg(target_os = "windows")]
pub fn install_startup() -> Result<()> {
    let startup_dir = dirs::config_dir()
        .map(|d| {
            d.join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup")
        })
        .ok_or_else(|| anyhow::anyhow!("Could not find Startup folder"))?;

    let exe_path = std::env::current_exe()?;
    let shortcut_path = startup_dir.join("deskmic.lnk");

    // Use PowerShell to create .lnk shortcut
    let ps_script = format!(
        "$WshShell = New-Object -ComObject WScript.Shell; \
         $Shortcut = $WshShell.CreateShortcut('{}'); \
         $Shortcut.TargetPath = '{}'; \
         $Shortcut.WorkingDirectory = '{}'; \
         $Shortcut.Save()",
        shortcut_path.display(),
        exe_path.display(),
        exe_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Could not determine executable directory"))?
            .display(),
    );

    let output = std::process::Command::new("powershell")
        .args(["-Command", &ps_script])
        .output()?;

    if output.status.success() {
        println!("Installed to startup: {}", shortcut_path.display());
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create shortcut: {}", err)
    }
}

#[cfg(not(target_os = "windows"))]
pub fn install_startup() -> Result<()> {
    anyhow::bail!("Install/uninstall is only supported on Windows")
}

/// Remove the shortcut from the Startup folder.
#[cfg(target_os = "windows")]
pub fn uninstall_startup() -> Result<()> {
    let startup_dir = dirs::config_dir()
        .map(|d| {
            d.join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup")
        })
        .ok_or_else(|| anyhow::anyhow!("Could not find Startup folder"))?;

    let shortcut_path = startup_dir.join("deskmic.lnk");

    if shortcut_path.exists() {
        std::fs::remove_file(&shortcut_path)?;
        println!("Removed from startup: {}", shortcut_path.display());
    } else {
        println!("Not installed in startup");
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn uninstall_startup() -> Result<()> {
    anyhow::bail!("Install/uninstall is only supported on Windows")
}

/// Show current recording status.
pub fn show_status(recordings_dir: &std::path::Path) -> Result<()> {
    let (file_count, total_bytes) = crate::storage::get_storage_stats(recordings_dir)?;
    let total_mb = total_bytes as f64 / 1_048_576.0;

    println!("deskmic status:");
    println!("  Recordings dir: {}", recordings_dir.display());
    println!("  Total files:    {}", file_count);
    println!("  Total size:     {:.1} MB", total_mb);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_show_status_empty_dir() {
        let tmp = TempDir::new().unwrap();
        // Should not error on an empty directory
        show_status(tmp.path()).unwrap();
    }

    #[test]
    fn test_show_status_nonexistent_dir() {
        let tmp = TempDir::new().unwrap();
        let nonexistent = tmp.path().join("nonexistent");
        // Should not error on a nonexistent directory (get_storage_stats returns (0, 0))
        show_status(&nonexistent).unwrap();
    }

    #[test]
    fn test_show_status_with_files() {
        let tmp = TempDir::new().unwrap();
        // Create a date folder with a file (mirrors storage layout)
        let date_dir = tmp.path().join("2025-06-01");
        std::fs::create_dir_all(&date_dir).unwrap();
        std::fs::write(date_dir.join("test.wav"), &[0u8; 1024]).unwrap();

        show_status(tmp.path()).unwrap();
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_install_startup_fails_on_non_windows() {
        let result = install_startup();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("only supported on Windows"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_uninstall_startup_fails_on_non_windows() {
        let result = uninstall_startup();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("only supported on Windows"));
    }
}
