//! Crash give-up records written by the sign-in launcher and read by process status.
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

/// A widget the launcher stopped restarting after repeated crashes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GaveUp {
    /// Unix time in seconds when the launcher gave up.
    pub gave_up: u64,
    /// The last process exit code.
    pub exit_code: i32,
}

const LOCK: &str = "Local\\TaskbarDock.Watchdog";

/// Records that the launcher gave up restarting a widget.
pub fn record_gave_up(name: &str, exit_code: i32) -> io::Result<()> {
    record_gave_up_with_lock(LOCK, name, exit_code)
}

/// Removes a widget's give-up record when the launcher starts it.
pub fn clear(name: &str) -> io::Result<()> {
    clear_with_lock(LOCK, name)
}

/// Reads all valid give-up records. Missing, unreadable, or invalid files produce no records.
pub fn read() -> HashMap<String, GaveUp> {
    let path = crate::settings::dir().join("watchdog.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return HashMap::new(),
        Err(error) => {
            eprintln!("could not read {}: {error}", path.display());
            return HashMap::new();
        }
    };
    let value = match serde_json::from_str::<Value>(&text) {
        Ok(value) if value.is_object() => value,
        Ok(_) => {
            eprintln!("{} is not a watchdog record object", path.display());
            return HashMap::new();
        }
        Err(error) => {
            eprintln!("could not parse {}: {error}", path.display());
            return HashMap::new();
        }
    };
    let mut records = HashMap::new();
    for (name, value) in value
        .as_object()
        .into_iter()
        .flat_map(|records| records.iter())
    {
        let record = value
            .get("gave_up")
            .and_then(Value::as_u64)
            .zip(value.get("exit_code").and_then(Value::as_i64))
            .and_then(|(gave_up, exit_code)| {
                Some(GaveUp {
                    gave_up,
                    exit_code: exit_code.try_into().ok()?,
                })
            });
        let Some(record) = record else {
            eprintln!("{} contains an invalid record for {name:?}", path.display());
            return HashMap::new();
        };
        records.insert(name.clone(), record);
    }
    records
}

/// Adds a give-up record to a JSON object.
fn add_record(root: &mut Value, name: &str, gave_up: u64, exit_code: i32) {
    if !root.is_object() {
        *root = json!({});
    }
    root[name] = json!({"gave_up": gave_up, "exit_code": exit_code});
}

/// Removes one give-up record from a JSON object.
fn clear_record(root: &mut Value, name: &str) {
    if let Some(records) = root.as_object_mut() {
        records.remove(name);
    }
}

/// Serializes read, change, and atomic write under a caller-selected mutex.
fn update_with_lock(lock: &str, change: impl FnOnce(&mut Value)) -> io::Result<()> {
    let _guard = WatchdogLock::take(lock)?;
    let path = crate::settings::dir().join("watchdog.json");
    let mut root = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error),
    };
    if !root.is_object() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "watchdog.json isn't an object",
        ));
    }
    let before = root.clone();
    change(&mut root);
    if root == before {
        return Ok(());
    }
    std::fs::create_dir_all(crate::settings::dir())?;
    let text = serde_json::to_string_pretty(&root)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
    crate::settings::write_atomic(&path, &(text + "\n"))
}

/// Records a give-up using a test-specific mutex when requested.
#[doc(hidden)]
pub fn record_gave_up_with_lock(lock: &str, name: &str, exit_code: i32) -> io::Result<()> {
    let gave_up = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    update_with_lock(lock, |root| add_record(root, name, gave_up, exit_code))
}

/// Clears a give-up using a test-specific mutex when requested.
#[doc(hidden)]
pub fn clear_with_lock(lock: &str, name: &str) -> io::Result<()> {
    update_with_lock(lock, |root| clear_record(root, name))
}

struct WatchdogLock(windows_sys::Win32::Foundation::HANDLE);

impl WatchdogLock {
    fn take(name: &str) -> io::Result<Self> {
        use windows_sys::Win32::Foundation::{
            CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
        };
        use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        unsafe {
            let handle = CreateMutexW(std::ptr::null(), 0, crate::wide(name).as_ptr());
            if handle == 0 {
                return Err(io::Error::last_os_error());
            }
            let result = WaitForSingleObject(handle, 2000);
            if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
                let error = if result == WAIT_TIMEOUT {
                    io::Error::new(io::ErrorKind::TimedOut, "watchdog.json is busy")
                } else {
                    io::Error::last_os_error()
                };
                CloseHandle(handle);
                return Err(error);
            }
            Ok(Self(handle))
        }
    }
}

impl Drop for WatchdogLock {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::ReleaseMutex;
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_clear_change_only_the_named_record() {
        let mut root = json!({"other": {"gave_up": 10, "exit_code": 1}});
        add_record(&mut root, "weather", 42, -1);
        assert_eq!(root["weather"], json!({"gave_up": 42, "exit_code": -1}));
        clear_record(&mut root, "weather");
        assert_eq!(root, json!({"other": {"gave_up": 10, "exit_code": 1}}));
    }
}
