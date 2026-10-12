//! Shared, validated settings writes for Block, dockctl and the settings window.
use crate::{
    describe::{self, Setting},
    registry::Entry,
};
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
/// The submitted setting value, either as text or as parsed JSON.
pub enum Input {
    /// Parse the user's entered text according to the setting description.
    Text(String),
    /// Validate an already parsed JSON value.
    Value(Value),
}
#[derive(Clone, Debug, PartialEq)]
/// A settings section with descriptions and defaults.
pub struct Target {
    /// The settings.json section name.
    pub section: String,
    /// The descriptions accepted by this target.
    pub settings: Vec<Setting>,
    /// The default values for described settings.
    pub defaults: Value,
}
#[derive(Clone, Debug, PartialEq)]
/// A validation failure or a settings-file write failure.
pub enum WriteError {
    /// The key or value is invalid.
    Invalid(String),
    /// The settings file could not be updated.
    NotSaved(String),
}
impl Target {
    /// Creates a target from a widget's parsed description.
    pub fn from_describe(d: &crate::block::Describe) -> Self {
        Self {
            section: d.section.clone(),
            settings: d.settings.clone(),
            defaults: d.defaults.clone(),
        }
    }
    /// Creates a target for shared dock settings.
    pub fn dock() -> Self {
        Self {
            section: "dock".into(),
            settings: crate::style::dock_settings(),
            defaults: crate::style::dock_defaults(),
        }
    }
    /// Builds a widget description from its spec and Block-owned rows.
    pub fn from_spec(spec: &crate::block::Spec) -> Self {
        Self::from_describe(&crate::block::describe(
            spec.section,
            spec.class,
            spec.title,
            spec.description,
            spec.settings,
            spec.defaults,
            spec.log_file.clone(),
        ))
    }
    /// Finds the description for one key.
    pub fn setting(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.key == key)
    }
}
#[derive(Clone, Debug, PartialEq)]
/// Reports the stored value and whether the write and widget reload succeeded.
pub struct Applied {
    /// The canonical value written, if validation succeeded.
    pub value: Option<Value>,
    /// Whether the settings file was updated.
    pub saved: Result<(), String>,
    /// Whether each running widget confirmed the reload.
    pub followed: Result<(), String>,
    /// Whether the submitted value failed validation.
    pub invalid: bool,
}
/// Validates a value and writes only the described setting key.
pub fn write(target: &Target, key: &str, input: Input) -> Result<Value, WriteError> {
    let setting = target
        .setting(key)
        .ok_or_else(|| WriteError::Invalid(format!("unknown key {key:?}")))?;
    let value = match input {
        Input::Text(s) => describe::parse(setting, &s),
        Input::Value(v) => describe::check(setting, &v),
    }
    .map_err(WriteError::Invalid)?;
    crate::settings::update(|root| {
        if !root[target.section.as_str()].is_object() {
            root[&target.section] = serde_json::json!({});
        }
        describe::set_at(&mut root[&target.section], key, value.clone());
    })
    .map_err(|e| WriteError::NotSaved(e.to_string()))?;
    Ok(value)
}
/// Writes the described default for one key.
pub fn write_default(target: &Target, key: &str) -> Result<Value, WriteError> {
    let value = describe::value_at(&target.defaults, key)
        .cloned()
        .ok_or_else(|| WriteError::Invalid(format!("unknown key {key:?}")))?;
    write(target, key, Input::Value(value))
}
/// Writes a setting and asks the listed running widgets to reload.
pub fn apply(target: &Target, key: &str, input: Input, follow: &[Entry]) -> Applied {
    apply_inner(target, key, input, follow)
}
/// Restores a setting's described default and asks widgets to reload.
pub fn reset(target: &Target, key: &str, follow: &[Entry]) -> Applied {
    let Some(v) = describe::value_at(&target.defaults, key).cloned() else {
        return Applied {
            value: None,
            saved: Err(format!("Invalid: unknown key {key:?}")),
            followed: Ok(()),
            invalid: true,
        };
    };
    apply_inner(target, key, Input::Value(v), follow)
}
/// Implements setting writes and records reload confirmations.
fn apply_inner(target: &Target, key: &str, input: Input, follow: &[Entry]) -> Applied {
    let value = match write(target, key, input) {
        Ok(v) => v,
        Err(WriteError::Invalid(e)) => {
            return Applied {
                value: None,
                saved: Err(format!("Invalid: {e}")),
                followed: Ok(()),
                invalid: true,
            }
        }
        Err(WriteError::NotSaved(e)) => {
            return Applied {
                value: None,
                saved: Err(e),
                followed: Ok(()),
                invalid: false,
            }
        }
    };
    let mut not_confirmed = Vec::new();
    for e in follow {
        let args = vec!["reload".to_string()];
        match crate::client::send(
            &e.class,
            crate::client::ExeMatch::Path(e.exe.clone()),
            &args,
            std::time::Duration::from_secs(5),
        ) {
            Ok(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => {}
            Err(crate::client::SendError::NotRunning) => {}
            _ => not_confirmed.push(e.name.clone()),
        }
    }
    Applied {
        value: Some(value),
        saved: Ok(()),
        followed: if not_confirmed.is_empty() {
            Ok(())
        } else {
            Err(format!("{} hasn't confirmed", not_confirmed.join(", ")))
        },
        invalid: false,
    }
}

/// The process and reply state of a registered widget for one status round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// A live widget answered its ping.
    Running,
    /// A matching process exists but has no main window or did not answer.
    NotResponding,
    /// No matching process or give-up record exists.
    Stopped,
    /// The launcher gave up restarting this widget and it is not running.
    KeepsCrashing,
    /// The process snapshot failed, so the widget's state could not be determined.
    Unknown,
}

/// A widget's observed state and optional process, build, and diagnostic details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The registered widget name.
    pub name: String,
    /// The result of this status round.
    pub state: State,
    /// The matching process id, when one exists.
    pub pid: Option<u32>,
    /// The build timestamp from a successful ping.
    pub build: Option<u64>,
    /// A short explanation for a non-running state.
    pub reason: Option<String>,
}

/// Describes a successful start and any watchdog cleanup warning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartResult {
    /// The responding widget process id.
    pub pid: u32,
    /// A warning when the widget started but its watchdog record could not be cleared.
    pub warning: Option<String>,
}

fn start_result(value: &Value, warning: Option<String>) -> Option<StartResult> {
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    crate::client::Ping::from_reply(value)
        .ok()
        .map(|ping| StartResult {
            pid: ping.pid,
            warning,
        })
}

fn watchdog_warning(result: std::io::Result<()>) -> Option<String> {
    result
        .err()
        .map(|error| format!("started, but watchdog record could not be cleared: {error}"))
}

fn unknown_status(name: &str, reason: &str) -> Status {
    Status {
        name: name.into(),
        state: State::Unknown,
        pid: None,
        build: None,
        reason: Some(reason.into()),
    }
}

/// Resolves one widget's state using the process-over-record precedence from contract G.
fn decide(
    running: Option<u32>,
    window: bool,
    reply: Option<crate::client::Ping>,
    record: Option<crate::watchdog::GaveUp>,
) -> State {
    if running.is_some() {
        if window && reply.is_some() {
            State::Running
        } else {
            State::NotResponding
        }
    } else if record.is_some() {
        State::KeepsCrashing
    } else {
        State::Stopped
    }
}

/// Takes one process snapshot and probes each matching widget window in a single reply round.
pub fn status_all(entries: &[Entry]) -> Vec<Status> {
    status_all_with_records(entries, crate::watchdog::read())
}

/// Takes one status round with caller-supplied records, for dockctl's redirected read-only mode.
#[doc(hidden)]
pub fn status_all_with_records(
    entries: &[Entry],
    records: HashMap<String, crate::watchdog::GaveUp>,
) -> Vec<Status> {
    let processes = match process_snapshot() {
        Ok(processes) => processes,
        Err(error) => {
            return entries
                .iter()
                .map(|entry| unknown_status(&entry.name, &error))
                .collect();
        }
    };
    let running: Vec<Option<u32>> = entries
        .iter()
        .map(|entry| {
            processes
                .iter()
                .find(|(_, path)| same_exe(path, &entry.exe))
                .map(|(pid, _)| *pid)
        })
        .collect();
    let windows: Vec<Option<isize>> = entries
        .iter()
        .zip(&running)
        .map(|(entry, pid)| pid.and_then(|pid| find_main_window(pid, &entry.class)))
        .collect();
    let mut batch = Vec::new();
    let mut batch_indices = Vec::new();
    for (index, window) in windows.iter().enumerate() {
        if let Some(window) = window {
            batch.push((*window, vec!["ping".to_string()]));
            batch_indices.push(index);
        }
    }
    let mut replies = vec![None; entries.len()];
    for (index, result) in batch_indices
        .into_iter()
        .zip(crate::client::send_all(&batch, Duration::from_millis(1500)))
    {
        if let Ok(value) = result {
            if value.get("ok").and_then(Value::as_bool) == Some(true) {
                replies[index] = crate::client::Ping::from_reply(&value).ok();
            }
        }
    }
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let record = records.get(&entry.name).cloned();
            let state = decide(
                running[index],
                windows[index].is_some(),
                replies[index],
                record.clone(),
            );
            let reason = match state {
                State::Running | State::Stopped => None,
                State::NotResponding if windows[index].is_none() => Some("no window".into()),
                State::NotResponding => Some("no answer in 1.5 s".into()),
                State::KeepsCrashing => {
                    record.map(|record| format!("exit code {:#010X}", record.exit_code as u32))
                }
                State::Unknown => None,
            };
            Status {
                name: entry.name.clone(),
                state,
                pid: running[index],
                build: replies[index].map(|ping| ping.build),
                reason,
            }
        })
        .collect()
}

/// Starts a registered widget, or returns the id of its existing process.
pub fn start(entry: &Entry) -> Result<StartResult, String> {
    let already_running = process_snapshot()?
        .into_iter()
        .any(|(_, path)| same_exe(&path, &entry.exe));
    if !already_running {
        let exe_text = entry.exe.to_string_lossy();
        let application = crate::wide(&exe_text);
        let directory = entry
            .exe
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let directory = crate::wide(&directory.to_string_lossy());
        unsafe {
            use windows_sys::Win32::System::Threading::{
                CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
            };
            let mut startup: STARTUPINFOW = std::mem::zeroed();
            startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            let mut process: PROCESS_INFORMATION = std::mem::zeroed();
            let created = CreateProcessW(
                application.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                0,
                std::ptr::null(),
                directory.as_ptr(),
                &startup,
                &mut process,
            );
            if created == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            windows_sys::Win32::Foundation::CloseHandle(process.hThread);
            windows_sys::Win32::Foundation::CloseHandle(process.hProcess);
        }
    }
    let warning = watchdog_warning(crate::watchdog::clear(&entry.name));
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = remaining.min(Duration::from_millis(250));
        match crate::client::send(
            &entry.class,
            crate::client::ExeMatch::Path(entry.exe.clone()),
            &["ping".to_string()],
            timeout,
        ) {
            Ok(value) => {
                if let Some(result) = start_result(&value, warning.clone()) {
                    return Ok(result);
                }
            }
            _ => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            std::thread::sleep(remaining.min(Duration::from_millis(250)));
        }
    }
    Err("started, not answering".into())
}

/// Asks a running widget to quit and waits up to five seconds for its process to end, so a
/// Start right after it doesn't find the closing process and report it as running.
pub fn quit(entry: &Entry) -> Result<(), String> {
    let pid = process_snapshot()?
        .into_iter()
        .find(|(_, path)| same_exe(path, &entry.exe))
        .map(|(pid, _)| pid)
        .ok_or_else(|| "not running".to_string())?;
    ask_quit(entry)?;
    if wait_for_exit(pid) {
        Ok(())
    } else {
        Err("didn't quit".into())
    }
}

/// Sends ctl `quit` and succeeds only when the widget acknowledges the request.
fn ask_quit(entry: &Entry) -> Result<(), String> {
    let value = crate::client::send(
        &entry.class,
        crate::client::ExeMatch::Path(entry.exe.clone()),
        &["quit".to_string()],
        Duration::from_secs(5),
    )
    .map_err(|error| error.to_string())?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("widget did not acknowledge quit")
            .to_string())
    }
}

/// Quits the widget, waits up to five seconds for its process to end, then starts it again.
pub fn restart(entry: &Entry) -> Result<u32, String> {
    let pid = process_snapshot()?
        .into_iter()
        .find(|(_, path)| same_exe(path, &entry.exe))
        .map(|(pid, _)| pid)
        .ok_or_else(|| "not running".to_string())?;
    ask_quit(entry)?;
    if !wait_for_exit(pid) {
        return Err("didn't quit".into());
    }
    start(entry).map(|result| result.pid)
}

/// Waits for a process to end with a bounded Windows wait and closes its process handle.
fn wait_for_exit(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle == 0 {
        return !process_snapshot()
            .unwrap_or_default()
            .iter()
            .any(|(running, _)| *running == pid);
    }
    let result = unsafe { WaitForSingleObject(handle, 5000) };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
    result == windows_sys::Win32::Foundation::WAIT_OBJECT_0
}

/// The full path of a running process whose executable has this file name (case ignored), for
/// widgets known only by file name (dockctl's built-in table when there is no registry).
pub fn running_exe(file_name: &str) -> Option<std::path::PathBuf> {
    let wanted = file_name.to_lowercase();
    process_snapshot()
        .ok()?
        .into_iter()
        .map(|(_, path)| std::path::PathBuf::from(path))
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().to_lowercase() == wanted)
        })
}

/// Reads all accessible process image paths from one Toolhelp snapshot.
fn process_snapshot() -> Result<Vec<(u32, String)>, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = Vec::new();
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        if let Some(path) = process_path(entry.th32ProcessID) {
            found.push((entry.th32ProcessID, path));
        }
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    Ok(found)
}

/// Opens one process long enough to query its full executable path.
fn process_path(pid: u32) -> Option<String> {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return None;
        }
        let mut path = [0u16; 2048];
        let mut length = path.len() as u32;
        let result = QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length);
        windows_sys::Win32::Foundation::CloseHandle(handle);
        if result == 0 {
            None
        } else {
            Some(String::from_utf16_lossy(&path[..length as usize]))
        }
    }
}

/// Compares canonical Windows executable paths without case or extended-path prefix differences.
fn same_exe(path: &str, exe: &std::path::Path) -> bool {
    let normalize = |path: &str| path.strip_prefix("\\\\?\\").unwrap_or(path).to_lowercase();
    normalize(path) == normalize(&exe.to_string_lossy())
}

/// Finds a top-level `<class>Main` window owned by a particular process.
fn find_main_window(pid: u32, class: &str) -> Option<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowThreadProcessId,
    };
    struct Search {
        pid: u32,
        class: Vec<u16>,
        found: isize,
    }
    unsafe extern "system" fn visit(window: isize, parameter: isize) -> i32 {
        let search = &mut *(parameter as *mut Search);
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        if pid != search.pid {
            return 1;
        }
        let mut name = [0u16; 256];
        let length = GetClassNameW(window, name.as_mut_ptr(), name.len() as i32);
        if length > 0 && name[..length as usize] == search.class[..search.class.len() - 1] {
            search.found = window;
            return 0;
        }
        1
    }
    let class = crate::wide(&format!("{class}Main"));
    let mut search = Search {
        pid,
        class,
        found: 0,
    };
    unsafe { EnumWindows(Some(visit), &mut search as *mut _ as isize) };
    (search.found != 0).then_some(search.found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    /// Checks dotted lookup and writes to a nested leaf only.
    fn nested_lookup_and_leaf_write() {
        let mut v = json!({"tint_dark":{"quiet":0.2,"hover":0.3}});
        assert_eq!(describe::value_at(&v, "tint_dark.quiet"), Some(&json!(0.2)));
        describe::set_at(&mut v, "tint_dark.active", json!(0.4));
        assert_eq!(
            v,
            json!({"tint_dark":{"quiet":0.2,"hover":0.3,"active":0.4}})
        );
    }
    #[test]
    /// Checks that dock settings include all tint leaves.
    fn described_key_lookup_includes_nested_dock_leaves() {
        let t = Target::dock();
        assert!(t.setting("tint_dark.quiet").is_some());
        assert!(t.setting("tint_light.hover").is_some());
        assert!(t.setting("missing").is_none());
    }

    #[test]
    fn status_decision_precedence_covers_every_row() {
        let ping = Some(crate::client::Ping { pid: 7, build: 11 });
        let record = Some(crate::watchdog::GaveUp {
            gave_up: 1,
            exit_code: 9,
        });
        assert_eq!(decide(Some(7), true, ping, record.clone()), State::Running);
        assert_eq!(
            decide(Some(7), false, None, record.clone()),
            State::NotResponding
        );
        assert_eq!(
            decide(Some(7), true, None, record.clone()),
            State::NotResponding
        );
        assert_eq!(decide(None, false, None, record), State::KeepsCrashing);
        assert_eq!(decide(None, false, None, None), State::Stopped);
    }

    #[test]
    fn unknown_status_keeps_snapshot_failure_reason() {
        let status = unknown_status("weather", "access denied");
        assert_eq!(status.state, State::Unknown);
        assert_eq!(status.reason.as_deref(), Some("access denied"));
        assert_eq!(status.pid, None);
    }

    #[test]
    fn starting_requires_a_valid_ping_and_reports_watchdog_warning() {
        let warning = Some("watchdog locked".to_string());
        assert_eq!(
            start_result(
                &serde_json::json!({"ok":true,"pid":17,"build":42}),
                warning.clone()
            ),
            Some(StartResult { pid: 17, warning })
        );
        assert_eq!(start_result(&serde_json::json!({"ok":true}), None), None);
        assert_eq!(
            start_result(&serde_json::json!({"ok":false,"pid":17,"build":42}), None),
            None
        );
        let warning = watchdog_warning(Err(std::io::Error::other("locked")));
        assert_eq!(
            warning.as_deref(),
            Some("started, but watchdog record could not be cleared: locked")
        );
        assert_eq!(watchdog_warning(Ok(())), None);
    }
}
