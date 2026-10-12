//! Watchdog records use an isolated APPDATA folder and test-only mutex.
use taskbar_dock::{settings, watchdog};

#[test]
fn records_can_be_read_and_cleared_and_invalid_files_are_empty() {
    let dir = std::env::temp_dir().join(format!("tdock-watchdog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("APPDATA", &dir);
    let lock = "Local\\TaskbarDock.Watchdog.Test";

    watchdog::record_gave_up_with_lock(&lock, "weather", 7).unwrap();
    let records = watchdog::read();
    assert_eq!(records["weather"].exit_code, 7);
    assert!(records["weather"].gave_up > 0);

    watchdog::clear_with_lock(&lock, "weather").unwrap();
    assert!(watchdog::read().is_empty());

    let missing =
        std::env::temp_dir().join(format!("tdock-watchdog-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    std::fs::create_dir_all(&missing).unwrap();
    std::env::set_var("APPDATA", &missing);
    assert!(watchdog::read().is_empty());
    std::env::set_var("APPDATA", &dir);

    std::fs::create_dir_all(settings::dir()).unwrap();
    std::fs::write(settings::dir().join("watchdog.json"), "{").unwrap();
    assert!(watchdog::read().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&missing);
}
