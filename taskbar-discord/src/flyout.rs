//! The panel (hover and left click): open, pin, hover-open, fade, close, and its buttons.

use super::*;

/// Left click on the strip: open, pin or close the panel.
pub(crate) fn toggle_popup() {
    if let Some(a) = with(|a| a.panel.click(Instant::now())) {
        panel_do(a);
    }
}

/// Does what the shared panel behaviour (taskbar_dock::panel) decided.
pub(crate) fn panel_do(action: Action) {
    match action {
        Action::OpenByHover => open_popup(false),
        Action::OpenByClick => open_popup(true),
        Action::Pin => {
            if let Some(Some(p)) = with(|a| a.popup) {
                unsafe { let _ = SetForegroundWindow(p); }
            }
        }
        Action::Close => close_popup(),
        Action::None => {}
    }
    if let Some((main, false)) = with(|a| (a.main, a.panel.wants_ticks())) {
        unsafe { let _ = KillTimer(main, TIMER_HOVER); }
    }
}

pub(crate) fn close_popup() {
    if let Some(Some(p)) = with(|a| {
        a.fade.stop();
        a.panel_hover = None;
        a.popup.take()
    }) {
        unsafe { let _ = DestroyWindow(p); }
    }
}

/// The panel closed for a reason of its own (lost focus, Esc, another widget's panel).
pub(crate) fn popup_closed(lost_focus: bool) {
    with(|a| a.panel.closed(Instant::now(), lost_focus));
    close_popup();
    panel_do(Action::None);
}

pub(crate) fn hover_started() {
    let Some((main, true)) = with(|a| {
        a.panel.mouse_entered(Instant::now());
        (a.main, a.panel.wants_ticks())
    }) else { return };
    unsafe { SetTimer(main, TIMER_HOVER, 50, None); }
}

pub(crate) fn contains(rc: &RECT, pt: POINT) -> bool {
    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
}

/// Every 50 ms while hovering or while the panel is hover-opened.
pub(crate) fn hover_step() {
    let Some((label, popup)) = with(|a| (a.label, a.popup)) else { return };
    let rect = |h: Option<HWND>| {
        let mut rc = RECT::default();
        if let Some(h) = h { unsafe { let _ = GetWindowRect(h, &mut rc); } }
        rc
    };
    let mut pt = POINT::default();
    unsafe { let _ = GetCursorPos(&mut pt); }
    let (on_label, on_panel) = (contains(&rect(label), pt), popup.is_some() && contains(&rect(popup), pt));
    let lk = look();
    if let Some(a) = with(|a| a.panel.tick(Instant::now(), on_label, on_panel, &lk)) {
        panel_do(a);
    }
}

pub(crate) fn fade_step() {
    let lk = look();
    let Some((main, popup, alpha)) = with(|a| (a.main, a.popup, a.fade.alpha(Instant::now(), &lk))) else { return };
    unsafe {
        match (popup, alpha) {
            (Some(p), Some(alpha)) => { let _ = SetLayeredWindowAttributes(p, COLORREF(0), alpha, LWA_ALPHA); }
            _ => { let _ = KillTimer(main, TIMER_FADE); }
        }
    }
}

/// The panel's scale and its place above the strip (shared rule), sized for what it shows now.
fn placement(label: HWND, v: &Voice) -> (f32, render::PanelLayout, i32, i32) {
    let scale = unsafe { GetDpiForWindow(label).max(96) } as f32 / 96.0;
    let lay = render::panel_layout(v, scale);
    let (x, y) = taskbar_dock::panel::place(raw(label), lay.w, lay.h, &look(), scale);
    (scale, lay, x, y)
}

/// Opens the panel, fading in. `activate`: opened by a click (takes focus; a click elsewhere
/// closes it). Opened by hovering it doesn't take focus.
pub(crate) fn open_popup(activate: bool) {
    let Some((light, Some(label), voice)) = with(|a| (a.look.light, a.label, a.voice.clone())) else { return };
    if let Some(tb) = taskbar() {
        unsafe { dock::close_other_panels(raw(tb), raw(label)); }
    }
    let (_, lay, x, y) = placement(label, &voice);
    unsafe {
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED, POPUP_CLASS, w!("Discord voice"),
            WS_POPUP, x, y, lay.w, lay.h, None, None, hinstance(), None) else { return };
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const c_void, 4);
        let dark: BOOL = (!light).into();
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const c_void, 4);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
        let main = with(|a| {
            a.popup = Some(hwnd);
            a.fade.start(Instant::now());
            a.main
        });
        if activate {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        if let Some(main) = main {
            SetTimer(main, TIMER_FADE, 10, None);
        }
    }
}

/// The call changed: the open panel grows or shrinks (still above the strip) and repaints.
pub(crate) fn refresh_popup() {
    let Some((Some(p), Some(label), voice)) = with(|a| (a.popup, a.label, a.voice.clone())) else { return };
    let (_, lay, x, y) = placement(label, &voice);
    unsafe {
        let mut rc = RECT::default();
        let _ = GetWindowRect(p, &mut rc);
        if (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top) != (x, y, lay.w, lay.h) {
            let _ = SetWindowPos(p, None, x, y, lay.w, lay.h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let _ = InvalidateRect(p, None, false);
    }
}

fn point(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

fn hit(hwnd: HWND, lp: LPARAM) -> Option<Button> {
    let (x, y) = point(lp);
    let scale = unsafe { GetDpiForWindow(hwnd).max(96) } as f32 / 96.0;
    with(|a| render::panel_layout(&a.voice, scale).hit(x, y)).flatten()
}

/// Mouse over the panel: highlight the button under it.
pub(crate) fn panel_mouse(hwnd: HWND, lp: LPARAM) {
    let b = hit(hwnd, lp);
    if with(|a| std::mem::replace(&mut a.panel_hover, b) != b) == Some(true) {
        unsafe {
            let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            let _ = TrackMouseEvent(&mut tme);
            let _ = InvalidateRect(hwnd, None, false);
        }
    }
}

/// A click on one of the panel's buttons: Discord does it and reports back (the panel updates
/// when its event arrives).
pub(crate) fn panel_click(hwnd: HWND, lp: LPARAM) {
    if let Some(b) = hit(hwnd, lp) {
        press(b);
    }
}
