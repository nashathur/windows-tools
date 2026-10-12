//! Background registry, status, settings-file and write workers.

use std::sync::mpsc::{self, Sender};
use std::time::Duration;
use taskbar_dock::{control, registry};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

/// A request handled serially by the settings writer.
#[derive(Clone)]
pub enum WriteJob {
    /// Apply a validated setting input.
    Apply {
        sequence: u64,
        target: control::Target,
        key: String,
        input: control::Input,
        follow: Vec<registry::Entry>,
    },
    /// Reset a described key to its default.
    Reset {
        sequence: u64,
        target: control::Target,
        key: String,
        follow: Vec<registry::Entry>,
    },
    /// Change a widget's sign-in startup entry.
    Startup {
        sequence: u64,
        name: String,
        exe: std::path::PathBuf,
        enabled: bool,
    },
    /// Removes a widget from the registry on the writer thread.
    Remove { name: String },
    /// Registers a widget executable on the writer thread.
    Register { exe: std::path::PathBuf },
}

/// A result posted back to the UI thread.
pub enum WorkResult {
    /// Registry entries and their cached descriptions.
    Registry(Vec<LoadedEntry>),
    /// One completed status poll.
    Status {
        round: u64,
        statuses: Vec<control::Status>,
    },
    /// A settings file read result.
    File {
        /// Parsed settings snapshot.
        result: Result<serde_json::Value, String>,
        /// File modification time around the read.
        mtime: Option<std::time::SystemTime>,
    },
    /// A completed setting write.
    Write {
        sequence: u64,
        section: String,
        key: String,
        result: Result<serde_json::Value, String>,
        followed: Result<(), String>,
    },
    /// A completed widget process action.
    Action {
        name: String,
        action: String,
        result: Result<u32, String>,
        warning: Option<String>,
    },
    /// Refreshed diagnostic details and masked raw settings for Developer.
    Developer {
        details: Vec<serde_json::Value>,
        raw: Option<String>,
    },
    /// A registry mutation result.
    RegistryMutation {
        operation: String,
        requested: String,
        result: Result<String, String>,
    },
}

/// One registry entry and its description result.
#[derive(Clone)]
pub struct LoadedEntry {
    /// The persistent registry entry.
    pub entry: registry::Entry,
    /// Parsed widget description, or a diagnostic reason.
    pub describe: Result<taskbar_dock::block::Describe, String>,
}

/// Starts one ordered writer thread and returns its send handle.
pub fn writer(window: isize) -> Sender<WriteJob> {
    let (tx, rx) = mpsc::channel::<WriteJob>();
    std::thread::spawn(move || {
        while let Ok(job) = rx.recv() {
            let mut batch = vec![job];
            while let Ok(next) = rx.try_recv() {
                batch.push(next);
            }
            for (job, replaced_sequences) in coalesce_jobs(batch) {
                match job {
                    WriteJob::Apply {
                        sequence,
                        target,
                        key,
                        input,
                        follow,
                    } => {
                        let result = control::apply(&target, &key, input, &follow);
                        log_write(
                            &key,
                            result.value.as_ref(),
                            result.saved.as_ref().err(),
                            target.setting(&key).is_some_and(|setting| {
                                taskbar_dock::describe::is_secret_kind(&setting.kind)
                            }),
                        );
                        post_write_result(
                            window,
                            sequence,
                            target.section,
                            key,
                            result.value.ok_or_else(|| {
                                result.saved.err().unwrap_or_else(|| "not saved".into())
                            }),
                            result.followed,
                            &replaced_sequences,
                        );
                    }
                    WriteJob::Reset {
                        sequence,
                        target,
                        key,
                        follow,
                    } => {
                        let result = control::reset(&target, &key, &follow);
                        log_write(
                            &key,
                            result.value.as_ref(),
                            result.saved.as_ref().err(),
                            target.setting(&key).is_some_and(|setting| {
                                taskbar_dock::describe::is_secret_kind(&setting.kind)
                            }),
                        );
                        post_write_result(
                            window,
                            sequence,
                            target.section,
                            key,
                            result.value.ok_or_else(|| {
                                result.saved.err().unwrap_or_else(|| "not saved".into())
                            }),
                            result.followed,
                            &replaced_sequences,
                        );
                    }
                    WriteJob::Startup {
                        sequence,
                        name,
                        exe,
                        enabled,
                    } => {
                        let result = taskbar_dock::startup::set_exe(&name, &exe, enabled)
                            .map(|_| serde_json::Value::Bool(enabled))
                            .map_err(|e| e.to_string());
                        log_write(
                            "startup",
                            result.as_ref().ok(),
                            result.as_ref().err(),
                            false,
                        );
                        post_write_result(
                            window,
                            sequence,
                            name,
                            "startup".into(),
                            result,
                            Ok(()),
                            &replaced_sequences,
                        );
                    }
                    WriteJob::Remove { name } => {
                        let result = registry::remove(&name)
                            .map(|_| name.clone())
                            .map_err(|error| error.to_string());
                        let _ = post(
                            window,
                            WorkResult::RegistryMutation {
                                operation: "remove".into(),
                                requested: name,
                                result,
                            },
                        );
                    }
                    WriteJob::Register { exe } => {
                        let result = registry::register(&exe)
                            .map(|entry| entry.name)
                            .map_err(|e| e.to_string());
                        let _ = post(
                            window,
                            WorkResult::RegistryMutation {
                                operation: "register".into(),
                                requested: String::new(),
                                result,
                            },
                        );
                    }
                }
            }
        }
    });
    tx
}

/// Reads widget details, recent logs, and masked settings on a worker thread.
pub fn read_developer(
    window: isize,
    entries: Vec<LoadedEntry>,
    statuses: Vec<control::Status>,
    include_raw: bool,
) {
    std::thread::spawn(move || {
        let raw = include_raw.then(|| {
            let settings = taskbar_dock::settings::read().unwrap_or_else(|_| serde_json::json!({}));
            super::model::masked_raw_settings(&settings, &entries)
        });
        let details = entries
            .iter()
            .map(|item| {
                let status = statuses
                    .iter()
                    .find(|status| status.name == item.entry.name);
                let build = status
                    .and_then(|status| status.build)
                    .or_else(|| modified_unix(&item.entry.exe));
                let log_lines = item
                    .describe
                    .as_ref()
                    .ok()
                    .and_then(|desc| desc.log_file.as_ref())
                    .and_then(|path| std::fs::read_to_string(path).ok())
                    .map(|text| {
                        let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
                        if lines.is_empty() {
                            lines.push("no log".into());
                        }
                        lines
                            .into_iter()
                            .rev()
                            .take(super::layout::LOG_LINES)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["no log".into()]);
                serde_json::json!({
                    "name": item.entry.name,
                    "exe": item.entry.exe.to_string_lossy(),
                    "build": build
                        .map(format_local_time)
                        .unwrap_or_else(|| "unknown".into()),
                    "pid": status
                        .and_then(|status| status.pid)
                        .map_or_else(|| "not running".into(), |pid| pid.to_string()),
                    "log_lines": log_lines,
                })
            })
            .collect();
        post(window, WorkResult::Developer { details, raw });
    });
}

/// Returns an executable's last modification time as Unix seconds.
fn modified_unix(path: &std::path::Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

/// Formats Unix seconds as local `yyyy-mm-dd hh:mm` time.
fn format_local_time(seconds: u64) -> String {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = seconds
        .saturating_add(11_644_473_600)
        .saturating_mul(10_000_000);
    let file_time = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    let mut local = utc;
    unsafe {
        if FileTimeToSystemTime(&file_time, &mut utc) == 0
            || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0
        {
            return "unknown".into();
        }
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
    )
}

/// Identifies a setting target for queued-write coalescing.
fn write_key(job: &WriteJob) -> Option<(String, String)> {
    match job {
        WriteJob::Apply { target, key, .. } | WriteJob::Reset { target, key, .. } => {
            Some((target.section.clone(), key.clone()))
        }
        WriteJob::Startup { name, .. } => Some((name.clone(), "startup".into())),
        WriteJob::Remove { .. } | WriteJob::Register { .. } => None,
    }
}

/// Returns a setting write's sequence, when it has one.
fn write_sequence(job: &WriteJob) -> Option<u64> {
    match job {
        WriteJob::Apply { sequence, .. }
        | WriteJob::Reset { sequence, .. }
        | WriteJob::Startup { sequence, .. } => Some(*sequence),
        WriteJob::Remove { .. } | WriteJob::Register { .. } => None,
    }
}

/// Coalesces each batch while retaining displaced sequences for the final outcome.
fn coalesce_jobs(jobs: Vec<WriteJob>) -> Vec<(WriteJob, Vec<u64>)> {
    let mut queued = Vec::new();
    for next in jobs {
        let key = write_key(&next);
        if let Some(position) = key.and_then(|key| {
            queued
                .iter()
                .position(|(job, _)| write_key(job).as_ref() == Some(&key))
        }) {
            let (replaced, mut sequences) =
                std::mem::replace(&mut queued[position], (next, Vec::new()));
            if let Some(sequence) = write_sequence(&replaced) {
                sequences.push(sequence);
            }
            queued[position].1 = sequences;
        } else {
            queued.push((next, Vec::new()));
        }
    }
    queued
}

/// Posts the same applied outcome for the write and each request it replaced.
fn post_write_result(
    window: isize,
    sequence: u64,
    section: String,
    key: String,
    result: Result<serde_json::Value, String>,
    followed: Result<(), String>,
    replaced_sequences: &[u64],
) {
    for result in write_results(sequence, section, key, result, followed, replaced_sequences) {
        post(window, result);
    }
}

/// Builds an applied outcome for the write and every request it replaced.
fn write_results(
    sequence: u64,
    section: String,
    key: String,
    result: Result<serde_json::Value, String>,
    followed: Result<(), String>,
    replaced_sequences: &[u64],
) -> Vec<WorkResult> {
    std::iter::once(sequence)
        .chain(replaced_sequences.iter().copied())
        .map(|sequence| WorkResult::Write {
            sequence,
            section: section.clone(),
            key: key.clone(),
            result: result.clone(),
            followed: followed.clone(),
        })
        .collect()
}

/// Loads the registry and cached descriptions away from the UI thread.
pub fn load_registry(window: isize) {
    std::thread::spawn(move || {
        let entries = registry::list()
            .into_iter()
            .map(|entry| {
                let describe = taskbar_dock::describe::cached(&entry.exe);
                LoadedEntry { entry, describe }
            })
            .collect();
        post(window, WorkResult::Registry(entries));
    });
}

/// Starts repeated two-second status rounds that never overlap.
pub fn status_loop(
    window: isize,
    entries: std::sync::Arc<std::sync::RwLock<Vec<registry::Entry>>>,
) -> Sender<()> {
    let (refresh, requests) = mpsc::channel();
    std::thread::spawn(move || {
        let mut round = 0u64;
        loop {
            let current = entries
                .read()
                .map(|entries| entries.clone())
                .unwrap_or_default();
            let statuses = control::status_all(&current);
            round = round.wrapping_add(1);
            post(window, WorkResult::Status { round, statuses });
            if requests
                .recv_timeout(Duration::from_secs(2))
                .is_err_and(|error| matches!(error, mpsc::RecvTimeoutError::Disconnected))
            {
                break;
            }
        }
    });
    refresh
}

/// Reads settings.json away from the UI thread.
pub fn read_file(window: isize) {
    std::thread::spawn(move || {
        let before = taskbar_dock::settings::mtime();
        let mut result = taskbar_dock::settings::read().map_err(|e| e.to_string());
        let mut after = taskbar_dock::settings::mtime();
        if before != after {
            result = taskbar_dock::settings::read().map_err(|e| e.to_string());
            after = taskbar_dock::settings::mtime();
        }
        post(
            window,
            WorkResult::File {
                result,
                mtime: after,
            },
        );
    });
}

/// Starts a widget process action away from the UI thread.
pub fn action(window: isize, entry: registry::Entry, action: String) {
    std::thread::spawn(move || {
        let (result, warning) = match action.as_str() {
            "start" => match taskbar_dock::control::start(&entry) {
                Ok(started) => (Ok(started.pid), started.warning),
                Err(error) => (Err(error), None),
            },
            "quit" => (taskbar_dock::control::quit(&entry).map(|_| 0), None),
            _ => (taskbar_dock::control::restart(&entry), None),
        };
        log_line(&format!(
            "{} {} {}{}",
            action,
            entry.name,
            result
                .as_ref()
                .map(|pid| pid.to_string())
                .unwrap_or_else(|error| format!("failed: {error}")),
            warning
                .as_deref()
                .map_or(String::new(), |warning| format!(" warning: {warning}"))
        ));
        post(
            window,
            WorkResult::Action {
                name: entry.name,
                action,
                result,
                warning,
            },
        );
    });
}

/// Appends one settings write record with secret values masked.
fn log_write(
    key: &str,
    value: Option<&serde_json::Value>,
    error: Option<&String>,
    described_secret: bool,
) {
    let value = value.map(|value| {
        if described_secret || taskbar_dock::ctl::is_secret(key) {
            serde_json::json!("***")
        } else {
            value.clone()
        }
    });
    log_line(&format!(
        "write {key} value={} error={}",
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".into()),
        error.cloned().unwrap_or_default()
    ));
}

/// Appends a line to settings-app.log and trims it to the last 128 KiB above 256 KiB.
pub(crate) fn log_line(line: &str) {
    use std::io::Write;
    let path = taskbar_dock::settings::dir().join("settings-app.log");
    let _ = std::fs::create_dir_all(taskbar_dock::settings::dir());
    if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > 256 * 1024) {
        if let Ok(bytes) = std::fs::read(&path) {
            let start = bytes.len().saturating_sub(128 * 1024);
            let cut = bytes[start..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |offset| start + offset + 1);
            let _ = std::fs::write(&path, &bytes[cut..]);
        }
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Posts a boxed worker result and frees it if Windows rejects the post.
fn post(window: isize, result: WorkResult) {
    let data = Box::into_raw(Box::new(result));
    if unsafe { PostMessageW(window, super::win::WM_WORK, 0, data as isize) } == 0 {
        unsafe {
            drop(Box::from_raw(data));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn startup(sequence: u64, enabled: bool) -> WriteJob {
        WriteJob::Startup {
            sequence,
            name: "weather".into(),
            exe: "weather.exe".into(),
            enabled,
        }
    }

    #[test]
    fn coalescing_keeps_newest_job_and_completes_each_replaced_sequence() {
        let queued = coalesce_jobs(vec![startup(1, true), startup(2, false), startup(3, true)]);

        assert_eq!(queued.len(), 1);
        assert!(matches!(queued[0].0, WriteJob::Startup { sequence: 3, .. }));
        assert_eq!(queued[0].1, [1, 2]);
        let completions = write_results(
            3,
            "weather".into(),
            "startup".into(),
            Ok(serde_json::json!(true)),
            Ok(()),
            &queued[0].1,
        );
        assert!(matches!(
            completions.as_slice(),
            [
                WorkResult::Write { sequence: 3, section, key, result: Ok(value), .. },
                WorkResult::Write { sequence: 1, result: Ok(first_value), .. },
                WorkResult::Write { sequence: 2, result: Ok(second_value), .. }
            ] if section == "weather"
                && key == "startup"
                && value == &serde_json::json!(true)
                && first_value == value
                && second_value == value
        ));
    }

    #[test]
    fn coalescing_does_not_merge_different_targets() {
        let queued = coalesce_jobs(vec![startup(1, true), {
            let mut job = startup(2, false);
            if let WriteJob::Startup { name, .. } = &mut job {
                *name = "bus".into();
            }
            job
        }]);

        assert_eq!(queued.len(), 2);
        assert!(queued.iter().all(|(_, replaced)| replaced.is_empty()));
    }
}
