//! The one settings file shared by all taskbar widgets: `%APPDATA%\TaskbarDock\settings.json`.
//!
//! ```json
//! { "dock": { ...shared style, see style.rs... }, "bus": { ... }, "weather": { ... } }
//! ```
//! Reads are pure. Writes are patches through [`update`], [`fill_missing`] and [`set_key`].
//! Writes are atomic (temp file + rename), and [`update`] holds a lock shared by all widgets
//! while it reads, changes and writes. If the lock is busy for 2 s, the write fails rather than
//! proceeding unlocked. An invalid file is left alone so the user can fix it.

use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

/// The folder shared by the widgets (settings, dock order).
pub fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("TaskbarDock")
}

pub fn path() -> PathBuf {
    dir().join("settings.json")
}

/// True when this process reads and writes a private copy of the settings file instead of the
/// real one: a process started from an MSIX-packaged app (the Claude desktop app) is redirected
/// that way even without a package identity of its own. Asks Windows where the open file really
/// is; false when the file doesn't exist yet or can't be opened.
pub fn redirected() -> bool {
    let asked = path();
    match final_path(&asked) {
        Some(real) => !same_path(&real, &asked.to_string_lossy()),
        None => false,
    }
}

/// Where `p` really is on disk (`GetFinalPathNameByHandleW`), or None if it can't be opened.
fn final_path(p: &Path) -> Option<String> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetFinalPathNameByHandleW, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    let name = crate::wide(&p.to_string_lossy());
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    unsafe {
        let h = CreateFileW(name.as_ptr(), FILE_READ_ATTRIBUTES, share, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, 0);
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let n = GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, 0);
        CloseHandle(h);
        if n == 0 || n as usize >= buf.len() {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    }
}

/// The same file, ignoring case and a `\\?\` prefix.
fn same_path(a: &str, b: &str) -> bool {
    let plain = |s: &str| s.strip_prefix("\\\\?\\").unwrap_or(s).to_lowercase();
    plain(a) == plain(b)
}

/// Last change of the settings file, to notice edits (poll it, e.g. once a second).
pub fn mtime() -> Option<SystemTime> {
    std::fs::metadata(path()).and_then(|m| m.modified()).ok()
}

/// The whole file: `{}` if it doesn't exist yet; an error if it can't be read or parsed.
pub fn read() -> io::Result<Value> {
    match std::fs::read_to_string(path()) {
        Ok(text) => {
            let value = serde_json::from_str::<Value>(&text)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "settings.json isn't valid JSON"))?;
            if !value.is_object() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "settings.json isn't valid JSON"));
            }
            Ok(value)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e),
    }
}

/// One section (`"bus"`, `"weather"`, `"dock"`, ...), if present.
pub fn section(name: &str) -> Option<Value> {
    read().ok()?.get(name).cloned()
}

/// Add defaults for absent keys only, recursively filling missing leaves in objects.
fn add_missing(current: &mut Value, defaults: &Value) -> bool {
    let (Some(current), Some(defaults)) = (current.as_object_mut(), defaults.as_object()) else { return false };
    let mut added = false;
    for (key, default) in defaults {
        match current.get_mut(key) {
            None => {
                current.insert(key.clone(), default.clone());
                added = true;
            }
            Some(value) if value.is_object() && default.is_object() => {
                added |= add_missing(value, default);
            }
            Some(_) => {}
        }
    }
    added
}

/// Fill absent keys in a section while preserving every value already in the file.
/// The locked update recomputes the patch, so a concurrent write between the initial check and
/// lock acquisition is preserved. A present non-object section is left untouched.
pub fn fill_missing(section: &str, defaults: &Value) -> io::Result<()> {
    let root = read()?;
    let missing = match root.get(section) {
        None => true,
        Some(current) if current.is_object() => {
            let mut current = current.clone();
            add_missing(&mut current, defaults)
        }
        Some(_) => false,
    };
    if !missing {
        return Ok(());
    }
    update(|root| match root.get_mut(section) {
        None => {
            root[section] = defaults.clone();
        }
        Some(current) if current.is_object() => {
            add_missing(current, defaults);
        }
        Some(_) => {}
    })
}

/// Set one plain key, creating or replacing a non-object section as an object.
pub fn set_key(section: &str, key: &str, value: Value) -> io::Result<()> {
    update(|root| {
        if !root[section].is_object() {
            root[section] = json!({});
        }
        root[section][key] = value;
    })
}

static TEMP_FILE_COUNTER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Gives each atomic write its own temp path, even within one process and thread.
fn atomic_temp_path(p: &Path) -> std::path::PathBuf {
    let thread = format!("{:?}", std::thread::current().id());
    let sequence = TEMP_FILE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    p.with_extension(format!("{}.{}.{}.tmp", std::process::id(), thread, sequence))
}

/// Writes `text` to `p` atomically: a unique temp file, then a rename over `p`. The rename is
/// retried briefly, as an editor or virus scanner may hold `p` for a moment.
pub fn write_atomic(p: &Path, text: &str) -> io::Result<()> {
    let tmp = atomic_temp_path(p);
    std::fs::write(&tmp, text)?;
    let mut tries = 0;
    loop {
        match std::fs::rename(&tmp, p) {
            Ok(()) => return Ok(()),
            Err(_) if tries < 10 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
}

/// Writes the whole file atomically. Use [`update`] to change it: that also keeps two widgets
/// from overwriting each other's change.
pub fn write(root: &Value) -> io::Result<()> {
    std::fs::create_dir_all(dir())?;
    let text = serde_json::to_string_pretty(root).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    write_atomic(&path(), &(text + "\n"))
}

/// Held while one process reads, changes and writes the file (a named mutex shared by all
/// widgets). At sign-in they all fill in missing keys under this lock. If it can't be had within
/// 2 s, the write fails.
struct FileLock(HANDLE);

/// The named mutex every widget takes around a write.
const LOCK_NAME: &str = "Local\\TaskbarDock.Settings";

impl FileLock {
    fn take(lock_name: &str) -> io::Result<FileLock> {
        unsafe {
            let h = CreateMutexW(std::ptr::null(), 0, crate::wide(lock_name).as_ptr());
            if h == 0 {
                return Err(io::Error::last_os_error());
            }
            let r = WaitForSingleObject(h, 2000);
            if r != WAIT_OBJECT_0 && r != WAIT_ABANDONED {
                let error = if r == WAIT_TIMEOUT {
                    io::Error::new(io::ErrorKind::TimedOut, "settings.json is busy (another widget holds its lock)")
                } else {
                    io::Error::last_os_error()
                };
                CloseHandle(h);
                return Err(error);
            }
            Ok(FileLock(h))
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                ReleaseMutex(self.0);
                CloseHandle(self.0);
            }
        }
    }
}

/// Read, change, write: `f` gets the whole file. Not written when `f` changed nothing.
/// Fails (without writing) if the file isn't valid JSON. One widget at a time ([`FileLock`]).
pub fn update(f: impl FnOnce(&mut Value)) -> io::Result<()> {
    update_with_lock(LOCK_NAME, f)
}

/// [`update`] under another lock name: for tests, so they never hold the widgets' lock.
#[doc(hidden)]
pub fn update_with_lock(lock_name: &str, f: impl FnOnce(&mut Value)) -> io::Result<()> {
    let _lock = FileLock::take(lock_name)?;
    let mut root = read()?;
    let before = root.clone();
    f(&mut root);
    if root != before {
        write(&root)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_path_ignores_case_and_prefix() {
        assert!(same_path("\\\\?\\C:\\Users\\a\\AppData\\Roaming\\TaskbarDock\\settings.json", "c:\\users\\a\\appdata\\roaming\\taskbardock\\settings.json"));
        assert!(!same_path(
            "\\\\?\\C:\\Users\\a\\AppData\\Local\\Packages\\Claude_x\\LocalCache\\Roaming\\TaskbarDock\\settings.json",
            "C:\\Users\\a\\AppData\\Roaming\\TaskbarDock\\settings.json"
        ));
    }

    #[test]
    fn atomic_temp_paths_are_unique_per_call() {
        let path = Path::new("settings.json");
        assert_ne!(atomic_temp_path(path), atomic_temp_path(path));
    }

    #[test]
    fn adds_missing_top_level_keys() {
        let mut current = json!({"kept": 1});
        assert!(add_missing(&mut current, &json!({"kept": 2, "new": "value"})));
        assert_eq!(current, json!({"kept": 1, "new": "value"}));
    }

    #[test]
    fn preserves_present_values_and_unknown_keys() {
        let mut current = json!({"null": null, "wrong": "kept", "extra": true});
        assert!(add_missing(&mut current, &json!({"null": 1, "wrong": {"child": 2}, "new": 3})));
        assert_eq!(current, json!({"null": null, "wrong": "kept", "extra": true, "new": 3}));
    }

    #[test]
    fn fills_only_missing_nested_leaves() {
        let mut current = json!({"tint_dark": {"quiet": 0.5}, "extra": 7});
        assert!(add_missing(&mut current, &json!({"tint_dark": {"quiet": 0.1, "hover": 0.2}})));
        assert_eq!(current, json!({"tint_dark": {"quiet": 0.5, "hover": 0.2}, "extra": 7}));
    }

    #[test]
    fn returns_false_when_nothing_is_missing() {
        let mut current = json!({"nested": {"a": 1}, "extra": true});
        assert!(!add_missing(&mut current, &json!({"nested": {"a": 2}})));
        assert_eq!(current, json!({"nested": {"a": 1}, "extra": true}));
    }
}
