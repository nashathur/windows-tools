//! Bringing an app's window to the front: the strip's cover / logo opens the app it shows
//! (rule "Cover opens the app" in CLAUDE.md). A widget only says which windows are the app's;
//! finding the main window, restoring it and the fallback live here.

use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongW, GetWindowTextLengthW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, GWL_EXSTYLE, GW_OWNER, SW_RESTORE,
    SW_SHOWNORMAL, WS_EX_TOOLWINDOW,
};

/// Full path of the program that owns window `h`.
pub fn exe_path(h: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if p == 0 {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(p, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(p);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Is `path` the program named `exe` (file name only, any case), e.g. "discord.exe"?
pub fn is_exe(path: &str, exe: &str) -> bool {
    path.rsplit(['\\', '/'])
        .next()
        .is_some_and(|f| f.eq_ignore_ascii_case(exe))
}

struct Search<'a> {
    is_app: &'a dyn Fn(HWND) -> bool,
    found: Option<HWND>,
}

unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
    let s = &mut *(lp as *mut Search);
    let owned = GetWindow(h, GW_OWNER) != 0;
    let tool = (GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW) != 0;
    // a main window: visible, with a title, not a popup of another window or a tool window
    if IsWindowVisible(h) != 0 && !owned && !tool && GetWindowTextLengthW(h) > 0 && (s.is_app)(h) {
        s.found = Some(h);
        return 0;
    }
    1
}

/// The first main window for which `is_app` is true, if any.
pub fn find(is_app: &dyn Fn(HWND) -> bool) -> Option<HWND> {
    let mut search = Search {
        is_app,
        found: None,
    };
    unsafe { EnumWindows(Some(each), &mut search as *mut _ as LPARAM) };
    search.found
}

/// Brings the app's main window to the front (restored if minimised). With no window (the app
/// sits in the tray) the shell opens `fallback` (a `shell:AppsFolder\<id>` or a `scheme://` link),
/// which makes a running app show itself.
pub fn raise(is_app: &dyn Fn(HWND) -> bool, fallback: &str) {
    unsafe {
        match find(is_app) {
            Some(h) => {
                if IsIconic(h) != 0 {
                    ShowWindow(h, SW_RESTORE);
                }
                SetForegroundWindow(h);
            }
            None => {
                let target = crate::wide(fallback);
                let open = crate::wide("open");
                ShellExecuteW(
                    0,
                    open.as_ptr(),
                    target.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    SW_SHOWNORMAL,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_exe;

    #[test]
    fn exe_names() {
        assert!(is_exe(
            r"C:\Users\me\AppData\Local\Discord\app-1.0.9\Discord.exe",
            "discord.exe"
        ));
        assert!(!is_exe(r"C:\x\DiscordPTB.exe", "discord.exe"));
        assert!(!is_exe(r"C:\x\notdiscord.exe", "discord.exe"));
    }
}
