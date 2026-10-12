//! settings::update from many writers at once loses nothing (the named-mutex lock), and the
//! file is never seen half-written. Runs in its own process with APPDATA pointed at a temp folder.

use serde_json::json;
use taskbar_dock::settings;

#[test]
fn concurrent_updates_lose_nothing() {
    let dir = std::env::temp_dir().join(format!("tdock-lock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("APPDATA", &dir);
    settings::write(&json!({ "n": 0 })).unwrap();

    let threads: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                for _ in 0..25 {
                    settings::update(|root| root["n"] = json!(root["n"].as_i64().unwrap() + 1))
                        .unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }

    assert_eq!(settings::read().unwrap()["n"], json!(200));
    // no temp files left behind
    let left: Vec<_> = std::fs::read_dir(settings::dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    assert_eq!(left, vec![std::ffi::OsString::from("settings.json")]);
    let _ = std::fs::remove_dir_all(&dir);
}
