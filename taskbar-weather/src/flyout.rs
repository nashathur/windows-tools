//! The flyout panel: open, pin, hover-open, fade and close.

use super::*;

/// Left click on the label or tray icon: open, pin or close the flyout.
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
            // a click elsewhere now closes it
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

/// Destroys the flyout. Callers tell the panel behaviour why (or it decided it).
pub(crate) fn close_popup() {
    if let Some(Some(p)) = with(|a| {
        a.fade.stop();
        a.popup.take()
    }) {
        unsafe { let _ = DestroyWindow(p); }
    }
}

/// The flyout closed for a reason of its own (lost focus, Esc, another widget's panel).
pub(crate) fn popup_closed(lost_focus: bool) {
    with(|a| a.panel.closed(Instant::now(), lost_focus));
    close_popup();
    panel_do(Action::None);
}

/// The mouse came onto the label: the flyout opens if it stays (see hover_step).
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

/// Every 50 ms while hovering or while the flyout is hover-opened.
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

/// One frame of the flyout's fade-in.
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
/// Opens the flyout, fading in. `activate`: opened by a click (takes focus; a click elsewhere
/// closes it). Opened by hovering it doesn't take focus, so typing elsewhere isn't interrupted.
pub(crate) fn open_popup(activate: bool) {
    let light = with(|a| a.look.light).unwrap_or(false);
    // only one panel at a time: clicking a widget doesn't activate anything, so the other
    // widgets' panels wouldn't notice the click by themselves
    if let (Some(tb), Some(Some(label))) = (taskbar(), with(|a| a.label)) {
        unsafe { dock::close_other_panels(raw(tb), raw(label)); }
    }
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
        let margin = (look().panel_margin * scale).round() as i32;
        let work = mi.rcWork;
        // centred on the label, kept on its monitor (shared rule); without a label (opened
        // from the tray icon) centred on the mouse
        let (x, y) = match with(|a| a.label).flatten() {
            Some(label) => taskbar_dock::panel::place(raw(label), w, h, &look(), scale),
            None => {
                use taskbar_dock::panel::{position, RECT as R};
                let at = R { left: pt.x, top: work.bottom, right: pt.x, bottom: work.bottom + 1 };
                let wk = R { left: work.left, top: work.top, right: work.right, bottom: work.bottom };
                position(at, w, h, wk, margin)
            }
        };
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED, POPUP_CLASS, w!("Weather"),
            WS_POPUP, x, y, w, h, None, None, hinstance(), None) else { return };
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const c_void, 4);
        let dark: BOOL = (!light).into();
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const c_void, 4);
        // start transparent; fade_step brings it in
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
