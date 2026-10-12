//! Send v2 control requests to a running Block window.
use crate::{ctl, wide};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Debug, PartialEq, Eq)]
/// Identifies a widget executable by its full path or file name.
pub enum ExeMatch {
    /// Match the complete executable path, ignoring case.
    Path(std::path::PathBuf),
    /// Match the executable's file name, ignoring case.
    FileName(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
/// Describes why a control request could not be completed.
pub enum SendError {
    /// No matching widget process is running.
    NotRunning,
    /// A widget process exists but does not answer control requests.
    RunningNotAnswering,
    /// The widget did not reply before the deadline.
    Timeout(Duration),
    /// The request failed for another reason.
    Failed(String),
}
impl std::fmt::Display for SendError {
    /// Formats the error for dockctl's JSON response.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning => write!(f, "not running"),
            Self::RunningNotAnswering => write!(f, "running, not answering"),
            Self::Timeout(d) => write!(
                f,
                "no answer within {} s (the command may still finish in the widget)",
                d.as_secs()
            ),
            Self::Failed(s) => f.write_str(s),
        }
    }
}
impl std::error::Error for SendError {}
thread_local! {
    static EXPECT: Cell<Option<(HWND, u64)>> = const { Cell::new(None) };
    static REPLY: RefCell<Option<String>> = const { RefCell::new(None) };
    static REPLY_WINDOW: Cell<HWND> = const { Cell::new(0) };
}
static NEXT: AtomicU64 = AtomicU64::new(1);
/// Receives a matching WM_COPYDATA reply from the target widget.
unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_COPYDATA {
        let cds = &*(l as *const COPYDATASTRUCT);
        if cds.dwData == ctl::REPLY2 && !cds.lpData.is_null() {
            let b = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
            // Decode and own the reply bytes before WM_COPYDATA returns.
            if let Ok((id, text)) = ctl::decode_reply2(b) {
                let sender = w as HWND;
                // A late reply from another window or request must not satisfy this send.
                if EXPECT.with(|e| e.get()) == Some((sender, id)) {
                    REPLY.with(|r| *r.borrow_mut() = Some(text));
                }
            }
            return 1;
        }
        return 0;
    }
    DefWindowProcW(h, m, w, l)
}
/// Creates or reuses this thread's hidden reply window.
fn reply_window() -> Result<HWND, SendError> {
    REPLY_WINDOW.with(|slot| {
        let existing = slot.get();
        if existing != 0 {
            return Ok(existing);
        }
        unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let name = wide("TaskbarDockClientReply");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(wndproc);
            wc.hInstance = inst;
            wc.lpszClassName = name.as_ptr();
            RegisterClassW(&wc);
            let h = CreateWindowExW(
                0,
                name.as_ptr(),
                name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                0,
                inst,
                std::ptr::null(),
            );
            if h == 0 {
                Err(SendError::Failed("creating the reply window failed".into()))
            } else {
                slot.set(h);
                Ok(h)
            }
        }
    })
}
/// Gets a window's owning process id and full executable path.
unsafe fn owner(h: HWND) -> Option<(u32, String)> {
    let mut pid = 0;
    GetWindowThreadProcessId(h, &mut pid);
    process_path(pid).map(|path| (pid, path))
}

/// Gets a process's full executable path.
unsafe fn process_path(pid: u32) -> Option<String> {
    let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if p == 0 {
        return None;
    }
    let mut b = [0u16; 2048];
    let mut n = b.len() as u32;
    let ok = QueryFullProcessImageNameW(p, 0, b.as_mut_ptr(), &mut n);
    CloseHandle(p);
    if ok == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&b[..n as usize]))
}
/// Checks whether an executable path matches the requested identity.
fn matches(exe: &ExeMatch, path: &str) -> bool {
    match exe {
        ExeMatch::Path(p) => normalize_path(&p.to_string_lossy()) == normalize_path(path),
        ExeMatch::FileName(n) => path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(path)
            .eq_ignore_ascii_case(n),
    }
}
/// Normalizes a Windows path for case-insensitive comparison.
fn normalize_path(p: &str) -> String {
    p.strip_prefix("\\\\?\\").unwrap_or(p).to_lowercase()
}
/// Finds a top-level window of the requested class and executable.
unsafe fn find(class: &str, exe: &ExeMatch) -> Option<(HWND, u32)> {
    let cls = wide(class);
    let mut after = 0;
    loop {
        let h = FindWindowExW(0, after, cls.as_ptr(), std::ptr::null());
        if h == 0 {
            return None;
        }
        if let Some((pid, path)) = owner(h) {
            if matches(exe, &path) {
                return Some((h, pid));
            }
        }
        after = h;
    }
}
/// Checks whether a process with the requested executable identity is running.
unsafe fn process_running(exe: &ExeMatch) -> bool {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if snap == INVALID_HANDLE_VALUE {
        return false;
    }
    let mut e: PROCESSENTRY32W = std::mem::zeroed();
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut more = Process32FirstW(snap, &mut e) != 0;
    let mut found = false;
    while more && !found {
        found = process_path(e.th32ProcessID).is_some_and(|path| matches(exe, &path));
        more = Process32NextW(snap, &mut e) != 0;
    }
    CloseHandle(snap);
    found
}
/// Sends a v2 request and waits for its matching reply until the deadline.
pub fn send(
    class: &str,
    exe: ExeMatch,
    args: &[String],
    timeout: Duration,
) -> Result<Value, SendError> {
    unsafe {
        let me = reply_window()?;
        let Some((target, pid)) = find(&format!("{class}Main"), &exe) else {
            return Err(if process_running(&exe) {
                SendError::RunningNotAnswering
            } else {
                SendError::NotRunning
            });
        };
        // Opening a panel or menu may need to focus the target, as after a click.
        AllowSetForegroundWindow(pid);
        let id =
            ((std::process::id() as u64) << 20).wrapping_add(NEXT.fetch_add(1, Ordering::Relaxed));
        let bytes = ctl::encode2(id, args);
        let cds = COPYDATASTRUCT {
            dwData: ctl::REQUEST2,
            cbData: bytes.len() as u32,
            lpData: bytes.as_ptr() as *mut _,
        };
        REPLY.with(|r| r.borrow_mut().take());
        EXPECT.with(|e| e.set(Some((target, id))));
        let deadline = Instant::now() + timeout;
        let mut result = 0;
        let sent = SendMessageTimeoutW(
            target,
            WM_COPYDATA,
            me as WPARAM,
            &cds as *const _ as LPARAM,
            SMTO_ABORTIFHUNG,
            timeout.as_millis().max(1).min(u32::MAX as u128) as u32,
            &mut result,
        );
        // A fast reply can arrive before SendMessageTimeoutW reports its result.
        if let Some(text) = REPLY.with(|r| r.borrow_mut().take()) {
            EXPECT.with(|e| e.set(None));
            return serde_json::from_str(&text)
                .map_err(|error| SendError::Failed(format!("bad reply: {error}")));
        }
        if sent == 0 {
            EXPECT.with(|e| e.set(None));
            return Err(SendError::Timeout(timeout));
        }
        if result == 0 {
            EXPECT.with(|e| e.set(None));
            return Err(SendError::Failed(
                "this widget's build is older than dockctl: rebuild it".into(),
            ));
        }
        loop {
            if let Some(text) = REPLY.with(|r| r.borrow_mut().take()) {
                EXPECT.with(|e| e.set(None));
                return serde_json::from_str(&text)
                    .map_err(|e| SendError::Failed(format!("bad reply: {e}")));
            }
            let now = Instant::now();
            if now >= deadline {
                EXPECT.with(|e| e.set(None));
                return Err(SendError::Timeout(timeout));
            }
            MsgWaitForMultipleObjects(
                0,
                std::ptr::null(),
                0,
                (deadline - now).as_millis().max(1).min(u32::MAX as u128) as u32,
                QS_ALLINPUT,
            );
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
