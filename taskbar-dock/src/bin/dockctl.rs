//! `dockctl <widget|all> <command> [args]`: command-line control of the widgets on Block, for
//! testing without the mouse (format and commands: `taskbar_dock::ctl`). Finds the widget's
//! hidden Block main window (`<class>Main`, owned by the widget's exe), sends the command,
//! prints the JSON reply. Only our own widgets' windows; no input is simulated.
//!
//! Exit code: 0 ok; 1 an error reply, no reply in time, or (single widget) not running / not on
//! Block; 2 usage.

use serde_json::{json, Value};
use std::cell::RefCell;
use taskbar_dock::ctl::{self, Known, WIDGETS};
use taskbar_dock::wide;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// How long a widget may take to answer.
const TIMEOUT_MS: u32 = 5000;

thread_local! {
    static REPLY: RefCell<Option<String>> = const { RefCell::new(None) };
}

unsafe extern "system" fn proc_(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_COPYDATA {
        let cds = &*(l as *const COPYDATASTRUCT);
        if cds.dwData == ctl::REPLY && !cds.lpData.is_null() {
            let b = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
            REPLY.with(|r| *r.borrow_mut() = Some(String::from_utf8_lossy(b).into_owned()));
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

unsafe fn ask(me: HWND, w: &Known, args: &[String]) -> Outcome {
    let Some((target, pid)) = find(&format!("{}Main", w.class), w.exe) else {
        return Outcome::Absent(if running(w.exe) { "running, not on Block yet" } else { "not running" });
    };
    // a panel or menu opened by the command may take focus, as after a click
    AllowSetForegroundWindow(pid);
    let data = ctl::encode(args);
    let cds = COPYDATASTRUCT { dwData: ctl::REQUEST, cbData: data.len() as u32, lpData: data.as_ptr() as *mut _ };
    REPLY.with(|r| r.borrow_mut().take());
    let mut res = 0;
    let sent = SendMessageTimeoutW(target, WM_COPYDATA, me as WPARAM, &cds as *const _ as LPARAM, SMTO_ABORTIFHUNG, TIMEOUT_MS, &mut res);
    let reply = REPLY.with(|r| r.borrow_mut().take());
    match (sent, reply) {
        (_, Some(text)) => Outcome::Reply(serde_json::from_str(&text).unwrap_or_else(|_| json!({ "ok": false, "error": "bad reply", "raw": text }))),
        (0, None) => Outcome::Failed(format!("no answer within {TIMEOUT_MS} ms")),
        (_, None) => Outcome::Failed("the window didn't reply (not a Block widget?)".into()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = |msg: &str| -> ! {
        if !msg.is_empty() {
            eprintln!("dockctl: {msg}");
        }
        eprintln!("{}", ctl::USAGE);
        std::process::exit(2);
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
    let mut all = serde_json::Map::new();
    for w in &targets {
        let v = match unsafe { ask(me, w, cmd) } {
            Outcome::Reply(v) => {
                failed |= !v.get("ok").and_then(Value::as_bool).unwrap_or(false);
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
    std::process::exit(if failed { 1 } else { 0 });
}
