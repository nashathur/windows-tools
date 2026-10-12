//! Hidden v2 control window used by dockctl settings.

use std::cell::Cell;
use taskbar_dock::{ctl, wide};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

thread_local! { static MAIN: Cell<HWND> = const { Cell::new(0) }; }

/// A copied request, safe to use after WM_COPYDATA returns.
pub struct Request {
    /// The caller's reply window.
    pub sender: HWND,
    /// The settings control window used as the reply identity.
    pub reply_source: HWND,
    /// Protocol request identifier.
    pub id: u64,
    /// Decoded command arguments.
    pub args: Vec<String>,
}

/// Registers and creates the hidden settings control window.
pub fn create(main: HWND) -> HWND {
    MAIN.with(|slot| slot.set(main));
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide("TaskbarDockSettingsMain");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(proc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc);
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            instance,
            std::ptr::null(),
        )
    }
}

/// Copies a protocol reply to the waiting dockctl window.
pub fn reply(sender: HWND, source: HWND, id: u64, value: &serde_json::Value) {
    let json = serde_json::to_string(value)
        .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"serialization failed\"}".into());
    let bytes = ctl::encode_reply2(id, &json);
    let cds = COPYDATASTRUCT {
        dwData: ctl::REPLY2,
        cbData: bytes.len() as u32,
        lpData: bytes.as_ptr() as *mut _,
    };
    unsafe {
        SendMessageW(
            sender,
            WM_COPYDATA,
            source as WPARAM,
            &cds as *const _ as LPARAM,
        );
    }
}

/// Handles incoming v2 protocol bytes and copies them before returning from WM_COPYDATA.
unsafe extern "system" fn proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_COPYDATA {
        let cds = &*(lparam as *const COPYDATASTRUCT);
        if cds.dwData != ctl::REQUEST2 || cds.lpData.is_null() || cds.cbData == 0 {
            return 0;
        }
        let bytes =
            std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize).to_vec();
        let Ok((id, args)) = ctl::decode2(&bytes) else {
            return 0;
        };
        let request = Box::into_raw(Box::new(Request {
            sender: wparam as HWND,
            reply_source: window,
            id,
            args,
        }));
        let main = MAIN.with(Cell::get);
        if PostMessageW(main, super::win::WM_REMOTE, 0, request as LPARAM) == 0 {
            drop(Box::from_raw(request));
            return 0;
        }
        return 1;
    }
    DefWindowProcW(window, message, wparam, lparam)
}
