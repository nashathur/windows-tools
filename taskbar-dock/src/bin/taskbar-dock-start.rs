//! Starts every widget on the `startup` list in settings.json (see src/startup.rs). Run at
//! sign-in by a shortcut in the Startup folder; also safe to run by hand (widgets refuse a
//! second instance). Problems go to `%APPDATA%\TaskbarDock\startup.log`.
//!
//! Then it stays (no window, wakes once a second) as a watchdog over the widgets it started: one
//! that crashes is started again after a growing wait, and given up on when it keeps crashing
//! (rules in `taskbar_dock::crash`). A widget that quits or is stopped is left alone; once none
//! is left to watch, the launcher ends.

#![windows_subsystem = "windows"]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use taskbar_dock::crash::{is_crash, Restarts};
use taskbar_dock::{settings, startup, watchdog};

fn log(line: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(settings::dir().join("startup.log"))
    {
        let _ = writeln!(f, "{now} {line}");
    }
}

fn spawn(name: &str, exe: &Path) -> Option<Child> {
    if let Err(err) = watchdog::clear(name) {
        log(&format!("{name}: could not clear watchdog record: {err}"));
    }
    let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    match Command::new(exe).current_dir(dir).spawn() {
        Ok(c) => Some(c),
        Err(err) => {
            log(&format!("{name}: {err}"));
            None
        }
    }
}

/// One widget being watched: running (`child`), or waiting to be started again (`restart_at`).
struct Watched {
    name: String,
    exe: PathBuf,
    child: Option<Child>,
    restart_at: Option<Instant>,
    restarts: Restarts,
}

fn main() {
    let started_at = Instant::now();
    let mut watched = Vec::new();
    for e in startup::list() {
        if !e.exe.is_file() {
            log(&format!("{}: {} not found", e.name, e.exe.display()));
            continue;
        }
        if let Some(child) = spawn(&e.name, &e.exe) {
            watched.push(Watched {
                name: e.name,
                exe: e.exe,
                child: Some(child),
                restart_at: None,
                restarts: Restarts::default(),
            });
        }
    }
    let names: Vec<&str> = watched.iter().map(|w| w.name.as_str()).collect();
    log(&format!(
        "started {}",
        if names.is_empty() {
            "nothing".to_string()
        } else {
            names.join(", ")
        }
    ));

    while !watched.is_empty() {
        std::thread::sleep(Duration::from_secs(1));
        let now = Instant::now();
        watched.retain_mut(|w| {
            if let Some(child) = &mut w.child {
                let Ok(Some(status)) = child.try_wait() else { return true }; // still running
                w.child = None;
                let code = status.code().unwrap_or(0);
                if !is_crash(code) {
                    return false; // quit or stopped: not ours to start again
                }
                match w.restarts.crashed(now.duration_since(started_at).as_secs()) {
                    Some(wait) => {
                        log(&format!("{} crashed (exit code {code:#010X}); starting it again in {} s", w.name, wait.as_secs()));
                        w.restart_at = Some(now + wait);
                    }
                    None => {
                        log(&format!("{} crashed (exit code {code:#010X}) again; it keeps crashing, not starting it again", w.name));
                        if let Err(err) = watchdog::record_gave_up(&w.name, code) {
                            log(&format!("{}: could not record watchdog give-up: {err}", w.name));
                        }
                        return false;
                    }
                }
            }
            if w.restart_at.is_some_and(|t| now >= t) {
                w.restart_at = None;
                w.child = spawn(&w.name, &w.exe);
                return w.child.is_some();
            }
            true
        });
    }
}
