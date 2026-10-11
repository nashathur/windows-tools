//! `dockctl <widget|all> <command> [args]`: command-line control of the widgets on Block, for
//! testing without the mouse (format and commands: `taskbar_dock::ctl`). Finds the widget's
//! hidden Block main window (`<class>Main`, owned by the widget's exe), sends the command,
//! prints the JSON reply. Only our own widgets' windows; no input is simulated.
//!
//! Exit code: 0 ok; 1 an error reply, no reply in time, or (single widget) not running / not on
//! Block; 2 usage.

use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use taskbar_dock::ctl::{self, Known, WIDGETS};
use taskbar_dock::wide;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// How long a widget may take to answer.
/// The default timeout for commands other than `do`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// The default timeout for widget `do` commands.
const DO_TIMEOUT: Duration = Duration::from_secs(120);
/// The next request ID, unique within this dockctl process.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static EXPECT: Cell<Option<(HWND, u64)>> = const { Cell::new(None) };
    static REPLY: RefCell<Option<String>> = const { RefCell::new(None) };
}

unsafe extern "system" fn proc_(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_COPYDATA {
        let cds = &*(l as *const COPYDATASTRUCT);
        if cds.dwData == ctl::REPLY2 && !cds.lpData.is_null() {
            let b = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
            if let Ok((id, text)) = ctl::decode_reply2(b) {
                let sender = w as HWND;
                if EXPECT.with(|expected| expected.get()) == Some((sender, id)) {
                    REPLY.with(|r| *r.borrow_mut() = Some(text));
                }
            }
            return 1;
        }
        return 0;
    }
    DefWindowProcW(h, m, w, l)
}

/// The exe file name of the process owning `h` (lower case).
unsafe fn exe_of(h: HWND) -> Option<(u32, String)> {
    let mut pid = 0;
    GetWindowThreadProcessId(h, &mut pid);
    let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if p == 0 {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(p, 0, buf.as_mut_ptr(), &mut len);
    CloseHandle(p);
    if ok == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    Some((pid, path.rsplit('\\').next().unwrap_or("").to_lowercase()))
}

/// A top-level window of `class` owned by `exe`.
unsafe fn find(class: &str, exe: &str) -> Option<(HWND, u32)> {
    let cls = wide(class);
    let mut after = 0;
    loop {
        let h = FindWindowExW(0, after, cls.as_ptr(), std::ptr::null());
        if h == 0 {
            return None;
        }
        if let Some((pid, name)) = exe_of(h) {
            if name == exe {
                return Some((h, pid));
            }
        }
        after = h;
    }
}

/// Is a process with this exe file name (lower case) running?
unsafe fn running(exe: &str) -> bool {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if snap == INVALID_HANDLE_VALUE {
        return false;
    }
    let mut e: PROCESSENTRY32W = std::mem::zeroed();
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = false;
    let mut more = Process32FirstW(snap, &mut e) != 0;
    while more && !found {
        let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
        found = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase() == exe;
        more = Process32NextW(snap, &mut e) != 0;
    }
    CloseHandle(snap);
    found
}

enum Outcome {
    Reply(Value),
    /// Not an error for `all`.
    Absent(&'static str),
    Failed(String),
}

unsafe fn ask(me: HWND, w: &Known, args: &[String], timeout: Duration) -> Outcome {
    let Some((target, pid)) = find(&format!("{}Main", w.class), w.exe) else {
        return Outcome::Absent(if running(w.exe) { "running, not on Block yet" } else { "not running" });
    };
    // a panel or menu opened by the command may take focus, as after a click
    AllowSetForegroundWindow(pid);
    let id = ((std::process::id() as u64) << 20).wrapping_add(NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let data = ctl::encode2(id, args);
    let cds = COPYDATASTRUCT { dwData: ctl::REQUEST2, cbData: data.len() as u32, lpData: data.as_ptr() as *mut _ };
    REPLY.with(|r| r.borrow_mut().take());
    let mut res = 0;
    let deadline = Instant::now() + timeout;
    let left = timeout.as_millis().max(1).min(u32::MAX as u128) as u32;
    EXPECT.with(|expected| expected.set(Some((target, id))));
    let outcome = (|| {
        let sent = SendMessageTimeoutW(target, WM_COPYDATA, me as WPARAM, &cds as *const _ as LPARAM, SMTO_ABORTIFHUNG, left, &mut res);
        if sent == 0 {
            return Outcome::Failed(timeout_error(timeout));
        }
        if let Some(v) = take_reply() {
            return parse_reply(v);
        }
        if res == 0 {
            return Outcome::Failed("this widget's build is older than dockctl: rebuild it".into());
        }
        loop {
            if let Some(v) = take_reply() {
                return parse_reply(v);
            }
            let now = Instant::now();
            if now >= deadline {
                return Outcome::Failed(timeout_error(timeout));
            }
            let left = (deadline - now).as_millis().max(1).min(u32::MAX as u128) as u32;
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, left, QS_ALLINPUT);
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    })();
    EXPECT.with(|expected| expected.set(None));
    outcome
}

fn timeout_error(timeout: Duration) -> String {
    format!("no answer within {} s (the command may still finish in the widget)", timeout.as_secs())
}

/// Takes the reply accepted by `proc_` for the current expectation.
fn take_reply() -> Option<String> {
    REPLY.with(|slot| slot.borrow_mut().take())
}

fn parse_reply(text: String) -> Outcome {
    Outcome::Reply(serde_json::from_str(&text).unwrap_or_else(|_| json!({ "ok": false, "error": "bad reply", "raw": text })))
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let usage = |msg: &str| -> ! {
        if !msg.is_empty() {
            eprintln!("dockctl: {msg}");
        }
        eprintln!("{}", ctl::USAGE);
        std::process::exit(2);
    };
    let (args, custom_timeout) = match parse_timeout(&raw) {
        Ok(v) => v,
        Err(e) => usage(&e),
    };
    let Some((who, cmd)) = args.split_first() else { usage("") };
    if let Err(e) = ctl::parse(cmd) {
        usage(&e);
    }
    let targets: Vec<&Known> = if who == "all" {
        WIDGETS.iter().collect()
    } else {
        match WIDGETS.iter().find(|w| w.name == who) {
            Some(w) => vec![w],
            None => usage(&format!("unknown widget {who:?}")),
        }
    };
    let me = unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let cls = wide("TaskbarDockCtl");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(proc_);
        wc.hInstance = hinst;
        wc.lpszClassName = cls.as_ptr();
        RegisterClassW(&wc);
        CreateWindowExW(0, cls.as_ptr(), cls.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, 0, hinst, std::ptr::null())
    };
    if me == 0 {
        eprintln!("dockctl: creating the reply window failed");
        std::process::exit(1);
    }
    let mut failed = false;
    let mut usage_failed = false;
    let mut all = serde_json::Map::new();
    for w in &targets {
        let timeout = custom_timeout.unwrap_or_else(|| if matches!(cmd, [do_cmd, ..] if do_cmd == "do") { DO_TIMEOUT } else { DEFAULT_TIMEOUT });
        let v = match unsafe { ask(me, w, cmd, timeout) } {
            Outcome::Reply(v) => {
                failed |= !v.get("ok").and_then(Value::as_bool).unwrap_or(false);
                usage_failed |= v.get("kind").and_then(Value::as_str) == Some("usage");
                v
            }
            Outcome::Absent(why) => {
                failed |= targets.len() == 1;
                json!({ "ok": false, "absent": why })
            }
            Outcome::Failed(e) => {
                failed = true;
                json!({ "ok": false, "error": e })
            }
        };
        all.insert(w.name.to_string(), v);
    }
    let out = if targets.len() == 1 { all.into_iter().next().map(|(_, v)| v).unwrap_or(Value::Null) } else { Value::Object(all) };
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    std::process::exit(exit_code(failed, usage_failed));
}

/// Returns dockctl's documented success, failure, or usage exit code.
fn exit_code(failed: bool, usage_failed: bool) -> i32 {
    if usage_failed { 2 } else if failed { 1 } else { 0 }
}

/// Removes leading `--timeout seconds` options from the command arguments.
fn parse_timeout(raw: &[String]) -> Result<(Vec<String>, Option<Duration>), String> {
    let mut args = Vec::new();
    let mut timeout = None;
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == "--timeout" {
            let value = raw.get(i + 1).ok_or_else(|| "--timeout needs seconds".to_string())?;
            let seconds: u64 = value.parse().map_err(|_| format!("--timeout: not a number: {value}"))?;
            if seconds == 0 {
                return Err("--timeout must be at least 1 second".into());
            }
            timeout = Some(Duration::from_secs(seconds));
            i += 2;
        } else {
            args.extend_from_slice(&raw[i..]);
            break;
        }
    }
    Ok((args, timeout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_is_removed_before_widget_name() {
        assert_eq!(parse_timeout(&["--timeout".into(), "9".into(), "bus".into(), "state".into()]).unwrap(), (vec!["bus".into(), "state".into()], Some(Duration::from_secs(9))));
        assert_eq!(parse_timeout(&["bus".into(), "state".into()]).unwrap(), (vec!["bus".into(), "state".into()], None));
        assert!(parse_timeout(&["--timeout".into(), "x".into(), "bus".into()]).is_err());
    }

    #[test]
    fn exit_codes_distinguish_usage_from_other_failures() {
        assert_eq!(exit_code(false, false), 0);
        assert_eq!(exit_code(true, false), 1);
        assert_eq!(exit_code(true, true), 2);
    }
}
