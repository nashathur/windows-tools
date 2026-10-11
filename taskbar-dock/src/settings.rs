//! The one settings file shared by all taskbar widgets: `%APPDATA%\TaskbarDock\settings.json`.
//!
//! ```json
//! { "dock": { ...shared style, see style.rs... }, "bus": { ... }, "weather": { ... } }
//! ```
//! Each widget reads and rewrites only its own section (and fills in missing `dock` keys), so
//! widgets never overwrite each other's settings. Writes are atomic (temp file + rename) and
//! [`update`] holds a lock shared by all widgets while it reads, changes and writes.
//! If the file isn't valid JSON (a half-finished hand edit) it is left alone.

use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

/// The folder shared by the widgets (settings, dock order).
pub fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("TaskbarDock")
}

pub fn path() -> PathBuf {
    dir().join("settings.json")
}

/// Last change of the settings file, to notice edits (poll it, e.g. once a second).
pub fn mtime() -> Option<SystemTime> {
    std::fs::metadata(path()).and_then(|m| m.modified()).ok()
}

/// The whole file: `{}` if it doesn't exist yet, `None` if it isn't valid JSON.
pub fn read() -> Option<Value> {
    match std::fs::read_to_string(path()) {
        Ok(text) => serde_json::from_str::<Value>(&text).ok().filter(|v| v.is_object()),
        Err(_) => Some(json!({})),
    }
}

/// One section (`"bus"`, `"weather"`, `"dock"`, ...), if present.
pub fn section(name: &str) -> Option<Value> {
    read()?.get(name).cloned()
}

/// Writes `text` to `p` atomically: a temp file named after this process (two widgets writing
/// at once never share one), then a rename over `p`. The rename is retried briefly, as an
/// editor or virus scanner may hold `p` for a moment.
pub fn write_atomic(p: &Path, text: &str) -> io::Result<()> {
    let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
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
/// widgets). At sign-in they all start together and fill in missing keys: without it one
/// widget's change could be lost. If it can't be had within 2 s, go on without it.
struct FileLock(HANDLE);

impl FileLock {
    fn take() -> FileLock {
        unsafe {
            let h = CreateMutexW(std::ptr::null(), 0, crate::wide("Local\\TaskbarDock.Settings").as_ptr());
            if h != 0 {
                let r = WaitForSingleObject(h, 2000);
                if r != WAIT_OBJECT_0 && r != WAIT_ABANDONED {
                    CloseHandle(h);
                    return FileLock(0);
                }
            }
            FileLock(h)
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
    let _lock = FileLock::take();
    let mut root = read().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "settings.json isn't valid JSON"))?;
    let before = root.clone();
    f(&mut root);
    if root != before {
        write(&root)?;
    }
    Ok(())
}
