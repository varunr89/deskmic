use std::path::PathBuf;

#[cfg(target_os = "windows")]
pub fn find_volume_by_label(label: &str) -> Option<PathBuf> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;

    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        let mut name_buf = [0u16; 260];
        let mut serial: u32 = 0;
        let mut max_comp: u32 = 0;
        let mut flags: u32 = 0;
        let ok = unsafe {
            GetVolumeInformationW(
                PCWSTR(wide.as_ptr()),
                Some(&mut name_buf),
                Some(&mut serial),
                Some(&mut max_comp),
                Some(&mut flags),
                None,
            )
            .is_ok()
        };
        if !ok {
            continue;
        }
        let nul = name_buf.iter().position(|&c| c == 0).unwrap_or(name_buf.len());
        let found = String::from_utf16_lossy(&name_buf[..nul]);
        if found.eq_ignore_ascii_case(label) {
            return Some(PathBuf::from(root));
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
pub fn find_volume_by_label(_label: &str) -> Option<PathBuf> {
    None
}
