//! Patch semantics and read errors, isolated from the user's settings folder.

use serde_json::json;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use taskbar_dock::settings;
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[test]
fn settings_patch_contract() {
    let dir = std::env::temp_dir().join(format!("tdock-patch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("APPDATA", &dir);

    // A stale section copy cannot put an intervening user value back.
    settings::write(&json!({"bus": {"label": "old"}})).unwrap();
    let root = settings::read().unwrap();
    let stale_copy = root["bus"].clone();
    assert_eq!(stale_copy["label"], json!("old"));
    settings::set_key("bus", "label", json!("new")).unwrap();
    settings::fill_missing("bus", &json!({"label": "default", "dest": ""})).unwrap();
    assert_eq!(settings::read().unwrap()["bus"], json!({"label": "new", "dest": ""}));

    // A complete section is a true no-op, including the file contents and timestamp.
    let before = std::fs::read(settings::path()).unwrap();
    let mtime = std::fs::metadata(settings::path()).unwrap().modified().unwrap();
    settings::fill_missing("bus", &json!({"label": "ignored", "dest": "ignored"})).unwrap();
    assert_eq!(std::fs::read(settings::path()).unwrap(), before);
    assert_eq!(std::fs::metadata(settings::path()).unwrap().modified().unwrap(), mtime);

    std::fs::remove_file(settings::path()).unwrap();
    assert_eq!(settings::read().unwrap(), json!({}));
    std::fs::write(settings::path(), "{").unwrap();
    assert_eq!(settings::read().unwrap_err().kind(), io::ErrorKind::InvalidData);
    settings::write(&json!({"safe": true})).unwrap();
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(settings::path()).unwrap();
    assert!(settings::read().is_err());
    drop(held);

    // A private lock name keeps this test independent of live widgets and their mutex.
    settings::write(&json!({"n": 1})).unwrap();
    let name = format!("Local\\TaskbarDock.Settings.test-{}", std::process::id());
    let lock_name = wide(&name);
    let (tx, rx) = mpsc::channel();
    let holder = std::thread::spawn(move || unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, lock_name.as_ptr());
        assert_ne!(handle, 0);
        let result = WaitForSingleObject(handle, 1000);
        assert!(result == WAIT_OBJECT_0 || result == WAIT_ABANDONED);
        tx.send(()).unwrap();
        std::thread::sleep(Duration::from_secs(3));
        ReleaseMutex(handle);
        CloseHandle(handle);
    });
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let before = std::fs::read(settings::path()).unwrap();
    let start = Instant::now();
    let result = settings::update_with_lock(&name, |root| root["n"] = json!(2));
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_millis(2500));
    assert_eq!(std::fs::read(settings::path()).unwrap(), before);
    holder.join().unwrap();

    let _ = std::fs::remove_dir_all(&dir);
}
