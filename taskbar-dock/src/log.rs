use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_SIZE: u64 = 256 * 1024;

/// The settings-folder log path for a widget name.
pub fn path(name: &str) -> PathBuf {
    crate::settings::dir().join(format!("{name}.log"))
}

/// Serialises rotation and appends: a widget logs from its UI and worker threads, and two
/// threads rotating at once would delete the history the first one just kept.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Append one timestamped line. Logging errors are intentionally ignored.
pub fn append(path: &Path, line: &str) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > MAX_SIZE) {
        let old = path.with_extension("log.old");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(path, old);
    }

    let time = unsafe {
        let mut time = std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut time);
        time
    };
    let text = format!("{} {line}\r\n", stamp(
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond,
    ));

    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(text.as_bytes());
    }
}

fn stamp(year: u16, month: u16, day: u16, hour: u16, min: u16, sec: u16) -> String {
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("taskbar-dock-log-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn stamp_formats_local_time_components() {
        assert_eq!(stamp(2026, 3, 4, 5, 6, 7), "2026-03-04 05:06:07");
    }

    #[test]
    fn append_rotates_oversized_log_and_replaces_old_log() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("widget.log");
        let old = dir.join("widget.log.old");
        std::fs::write(&path, vec![b'x'; MAX_SIZE as usize + 1]).unwrap();
        std::fs::write(&old, b"stale").unwrap();

        append(&path, "after rotation");

        let rotated = std::fs::read(&old).unwrap();
        assert_eq!(rotated.len(), MAX_SIZE as usize + 1);
        assert_eq!(rotated[0], b'x');
        let current = std::fs::read_to_string(&path).unwrap();
        assert!(current.ends_with(" after rotation\r\n"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
