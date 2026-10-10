//! The flyout panel's window (Block's interim `Spec::panel` hook, until the panel shell): Block
//! decides when it opens and closes, shows, focuses and fades it; this creates, paints and
//! destroys it.

use super::*;
use std::cell::Cell;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::*;
use taskbar_dock::block::PanelCmd;

const POPUP_CLASS: PCWSTR = w!("TaskbarWeatherPopup");

thread_local! {
    /// The flyout while open (0: none).
    static POPUP: Cell<isize> = const { Cell::new(0) };
}

fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).unwrap_or_default().into() }
}

pub(crate) fn register() {
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(popup_proc),
            hInstance: hinstance(),
            lpszClassName: POPUP_CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            style: CS_DROPSHADOW,
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
}

/// [`taskbar_dock::block::Spec::panel`].
pub(crate) fn panel(cmd: PanelCmd) -> isize {
    match cmd {
        PanelCmd::Open { strip } => open(strip),
        PanelCmd::Close => {
            let p = POPUP.replace(0);
            if p != 0 {
                unsafe { let _ = DestroyWindow(hwnd(p)); }
            }
            0
        }
    }
}

/// New data or look: paint the open flyout again.
pub(crate) fn invalidate() {
    let p = POPUP.get();
    if p != 0 {
        unsafe { let _ = InvalidateRect(hwnd(p), None, false); }
    }
}

/// Creates the flyout (layered, not shown yet): centred on the strip, or without one (opened from
/// the tray icon) above the mouse; kept on its monitor (shared rule).
fn open(strip: isize) -> isize {
    let look = block::with(|a: &mut App, _| a.look.clone()).unwrap_or_else(Look::current);
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let scale = dx as f32 / 96.0;
        let w = (render::POPUP_W as f32 * scale) as i32;
        let h = (render::POPUP_H as f32 * scale) as i32;
        let (x, y) = if strip != 0 {
            taskbar_dock::panel::place(strip, w, h, &look, scale)
        } else {
            use taskbar_dock::panel::{position, RECT as R};
            let margin = (look.panel_margin * scale).round() as i32;
            let work = mi.rcWork;
            let at = R { left: pt.x, top: work.bottom, right: pt.x, bottom: work.bottom + 1 };
            let wk = R { left: work.left, top: work.top, right: work.right, bottom: work.bottom };
            position(at, w, h, wk, margin)
        };
        let Ok(p) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED, POPUP_CLASS, w!("Weather"),
            WS_POPUP, x, y, w, h, None, None, hinstance(), None) else { return 0 };
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(p, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const c_void, 4);
        let dark: BOOL = (!look.light).into();
        let _ = DwmSetWindowAttribute(p, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const c_void, 4);
        POPUP.set(p.0 as isize);
        p.0 as isize
    }
}

extern "system" fn popup_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let scale = GetDpiForWindow(h).max(96) as f32 / 96.0;
                let painted = block::with(|a: &mut App, _| {
                    render::paint_popup(h, a.weather.as_ref(), a.error.as_deref(), scale, &a.look);
                });
                if painted.is_none() {
                    return DefWindowProcW(h, msg, wp, lp);
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_ACTIVATE => {
                // only if still open: Block closing it also deactivates it
                if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE && POPUP.get() == h.0 as isize {
                    block::panel_closed(true);
                }
                LRESULT(0)
            }
            WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
                block::panel_closed(false);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                block::right_click_on_panel();
                LRESULT(0)
            }
            WM_DESTROY => {
                if POPUP.get() == h.0 as isize {
                    POPUP.set(0);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}
