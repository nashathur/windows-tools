//! The panel shell: the main (click / hover) panel's window, Block's for every widget. The
//! Content sizes and draws it ([`Content::panel_size`], [`Content::panel_draw`]) and gets its
//! mouse; `win.rs` decides when it opens and closes, shows, focuses and fades it. This creates,
//! places, paints and destroys it, and reports what happens on it (Esc, lost focus, right click).

use super::*;
use std::cell::Cell;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU};

thread_local! {
    /// A TME_LEAVE is pending for the open panel (the mouse is on it).
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    /// Pressed on the panel: it has the mouse until the release.
    static PRESSED: Cell<bool> = const { Cell::new(false) };
}

fn class_name(class: &str) -> Vec<u16> {
    wide(&format!("{class}Panel"))
}

/// Once, in [`run`]: the panel's window class (with the drop shadow, as Windows' flyouts).
pub(super) unsafe fn register(class: &str, hinst: isize, cursor: isize) {
    let name = class_name(class);
    let mut wc: WNDCLASSW = std::mem::zeroed();
    wc.lpfnWndProc = Some(panel_proc);
    wc.hInstance = hinst;
    wc.hCursor = cursor;
    wc.style = CS_DROPSHADOW | CS_DBLCLKS;
    wc.lpszClassName = name.as_ptr();
    RegisterClassW(&wc);
}

/// Where the main panel goes and its size (x, y, w, h), at what the Content asks for now: centred
/// above the strip; without one (`strip` 0: opened from the tray icon) above the icon, never over
/// the tray overflow panel ([`core::panel_at_tray`]), or above the pointer when Windows doesn't
/// say where the icon is; kept on its monitor. None: the Content has no panel. `say`: log it.
fn placement(strip: HWND, kind: PanelKind, say: bool) -> Option<(i32, i32, i32, i32)> {
    let (look, main) = st(|s| (s.look.clone(), s.main))?;
    unsafe {
        let pt = dock::cursor();
        // from the tray: where its icon is (on the taskbar, or in the overflow panel)
        let tray_at = if strip == 0 { tray::place(main) } else { None };
        let mon = match tray_at {
            _ if strip != 0 => MonitorFromWindow(strip, MONITOR_DEFAULTTONEAREST),
            Some((i, _)) => MonitorFromPoint(
                POINT {
                    x: (i.0 + i.2) / 2,
                    y: (i.1 + i.3) / 2,
                },
                MONITOR_DEFAULTTONEAREST,
            ),
            None => MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST),
        };
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let scale = dx.max(96) as f32 / 96.0;
        let (w, mut h) = st(|s| s.content.panel_size(kind, scale))??;
        if w <= 0 || h <= 0 {
            return None;
        }
        let work = (
            mi.rcWork.left,
            mi.rcWork.top,
            mi.rcWork.right,
            mi.rcWork.bottom,
        );
        h = h.min((work.3 - work.1 - 2 * (look.panel_margin * scale).round() as i32).max(1));
        let (x, y) = if strip != 0 {
            crate::panel::place(strip, w, h, &look, scale)
        } else {
            let wk = mi.rcWork;
            let work = (wk.left, wk.top, wk.right, wk.bottom);
            let margin = (look.panel_margin * scale).round() as i32;
            let at = match tray_at {
                Some((icon, overflow)) => core::panel_at_tray(icon, overflow, w, h, work, margin),
                None => core::panel_at_pointer((pt.x, pt.y), w, h, work, margin),
            };
            if say {
                log(&format!(
                    "panel from the tray: icon {:?}, overflow {:?} -> {at:?}",
                    tray_at.map(|t| t.0),
                    tray_at.and_then(|t| t.1)
                ));
            }
            at
        };
        Some((x, y, w, h))
    }
}

/// Creates the main panel where [`placement`] says (layered, not shown yet: the caller shows and
/// fades it). 0: the Content has no panel, or creating failed.
pub(super) fn open(strip: HWND, kind: PanelKind) -> HWND {
    let Some((class, title, look)) = st(|s| (s.spec.class, s.spec.title, s.look.clone())) else {
        return 0;
    };
    let Some((x, y, w, h)) = placement(strip, kind, true) else {
        return 0;
    };
    unsafe {
        let (cls, title) = (class_name(class), wide(title));
        let p = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
            cls.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            x,
            y,
            w,
            h,
            0,
            0,
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );
        if p == 0 {
            return 0;
        }
        let round = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            p,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &round as *const _ as *const _,
            4,
        );
        let dark: BOOL = (!look.light).into();
        DwmSetWindowAttribute(
            p,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark as *const _ as *const _,
            4,
        );
        TRACKING.set(false);
        PRESSED.set(false);
        p
    }
}

/// The open panel `p` takes the size the Content asks for now, placed by the same rule as when it
/// opened (above the strip, or the tray icon), no fade; repainted either way ([`Ctx::resize_panel`]).
pub(super) fn resize(p: HWND, strip: HWND) {
    unsafe {
        let kind = st(|s| s.panel_kind).unwrap_or(PanelKind::Main);
        if let Some((x, y, w, h)) = placement(strip, kind, false) {
            let r = dock::rect(p);
            if (r.left, r.top, r.right - r.left, r.bottom - r.top) != (x, y, w, h) {
                SetWindowPos(p, 0, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        InvalidateRect(p, std::ptr::null(), 0);
    }
}

pub(super) fn destroy(p: HWND) {
    unsafe { DestroyWindow(p) };
}

/// Paints the panel: theme background, the Content's drawing, one BitBlt (no flicker).
unsafe fn paint(h: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(h, &mut ps);
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(h, &mut rc);
    let (w, hh) = (rc.right, rc.bottom);
    if let Some(mut cv) = Canvas::new(w, hh) {
        let scale = GetDpiForWindow(h).max(96) as f32 / 96.0;
        let drawn = st(|s| {
            let f = PanelFrame {
                look: &s.look,
                scale,
                w,
                h: hh,
            };
            let brush = CreateSolidBrush(f.theme().color.bg.colorref());
            FillRect(cv.dc, &rc, brush);
            DeleteObject(brush);
            s.content.panel_draw(s.panel_kind, &mut cv, &f);
        });
        if drawn.is_some() {
            BitBlt(hdc, 0, 0, w, hh, cv.dc, 0, 0, SRCCOPY);
        }
        EndPaint(h, &ps);
        if drawn.is_none() {
            // Block is busy further up the stack: paint again once it's done
            InvalidateRect(h, std::ptr::null(), 0);
        }
        return;
    }
    EndPaint(h, &ps);
}

fn mouse(ev: PanelMouse) {
    let Some(kind) = st(|s| {
        if s.panel != 0 {
            Some(s.panel_kind)
        } else {
            None
        }
    })
    .flatten() else {
        return;
    };
    call(|c, ctx| c.panel_mouse(kind, ev, ctx));
}

/// (x, y) of a mouse message, client px.
fn xy(lp: LPARAM) -> (i32, i32) {
    (
        (lp & 0xFFFF) as i16 as i32,
        ((lp >> 16) & 0xFFFF) as i16 as i32,
    )
}

unsafe extern "system" fn panel_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // only while it's still Block's panel: closing it also deactivates it
    let ours = || st(|s| s.panel == h) == Some(true);
    match msg {
        WM_PAINT => {
            paint(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_ACTIVATE => {
            if (wp & 0xFFFF) as u32 == WA_INACTIVE && ours() {
                panel_closed(true);
            }
            0
        }
        WM_KEYDOWN if ours() => {
            super::dispatch_panel_vk(
                wp as u32,
                GetKeyState(VK_CONTROL as i32) < 0,
                GetKeyState(VK_MENU as i32) < 0,
            );
            0
        }
        WM_SYSKEYDOWN if ours() => {
            let key = super::panel_key_from_vk(
                wp as u32,
                GetKeyState(VK_CONTROL as i32) < 0,
                GetKeyState(VK_MENU as i32) < 0,
            );
            if super::dispatch_panel_key_with_fallback(key) {
                0
            } else {
                DefWindowProcW(h, msg, wp, lp)
            }
        }
        WM_CHAR if ours() => {
            super::dispatch_panel_utf16(wp as u16);
            0
        }
        WM_MOUSEWHEEL if ours() => {
            super::dispatch_panel_wheel(((wp >> 16) as i16) as i32);
            0
        }
        WM_RBUTTONUP => {
            if ours() {
                right_click(true);
            }
            0
        }
        WM_MOUSEMOVE => {
            if !TRACKING.replace(true) {
                let mut t = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: h,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut t);
            }
            let (x, y) = xy(lp);
            mouse(PanelMouse::Move(x, y));
            0
        }
        // with CS_DBLCLKS the second press of a double click comes as WM_LBUTTONDBLCLK: it's still
        // a press (pin, capture, Down), then also a DoubleClick for the Content
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            // a click in a hover-opened panel pins it (it stays until a click elsewhere), and
            // still does what it's on
            if let Some(a) = st(|s| {
                if s.core.panel.by_hover() {
                    s.core.panel.click(Instant::now())
                } else {
                    Action::None
                }
            }) {
                panel_do(a);
            }
            let (x, y) = xy(lp);
            PRESSED.set(true);
            SetCapture(h); // a slider can be dragged off the panel
            mouse(PanelMouse::Down(x, y));
            if msg == WM_LBUTTONDBLCLK {
                mouse(PanelMouse::DoubleClick(x, y));
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = xy(lp);
            let pressed = PRESSED.replace(false);
            if pressed {
                ReleaseCapture(); // its WM_CAPTURECHANGED then finds nothing to cancel
            }
            mouse(PanelMouse::Up(x, y));
            0
        }
        WM_CAPTURECHANGED => {
            if PRESSED.replace(false) {
                mouse(PanelMouse::Cancel);
            }
            0
        }
        WM_MOUSELEAVE => {
            TRACKING.set(false);
            mouse(PanelMouse::Leave);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
