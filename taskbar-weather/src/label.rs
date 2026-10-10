//! The taskbar label: drawing, dock position, gliding, dragging and zoom (rules in
//! taskbar_dock::strip), WinEvent hooks.

use super::*;
use taskbar_dock::strip::{self, DragMove, Glide, Release};

pub(crate) fn taskbar() -> Option<HWND> {
    unsafe { FindWindowW(w!("Shell_TrayWnd"), None).ok().filter(|h| !h.is_invalid()) }
}

pub(crate) fn create_label() {
    let Some((enabled, existing)) = with(|a| (a.cfg.taskbar_label, a.label)) else { return };
    if !enabled || existing.is_some() {
        return;
    }
    let Some(tb) = taskbar() else { return };
    let hwnd = unsafe {
        CreateWindowExW(WS_EX_LAYERED | WS_EX_NOACTIVATE, LABEL_CLASS, w!("Weather"),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS, 0, 0, 1, 1, tb, None, hinstance(), None)
    };
    if let Ok(hwnd) = hwnd {
        with(|a| {
            a.label = Some(hwnd);
        });
        // join the dock stack shared with the bus widget (see dock_x)
        unsafe { dock::mark_member(raw(hwnd)); }
        redraw_label();
        unsafe { dock::notify(raw(hwnd)); }
    }
}

// ---------------------------------------------------------------- dock stack
//
// Docking, the shared look and the Windows settings come from the taskbar-dock library
// (C:\Users\thurs\tools\taskbar-dock), shared with the bus widget: with the label on the right
// the widgets form a stack against the tray; drag one to reorder. The library uses windows-sys
// handles (isize); `raw` converts.

pub(crate) const MY_CLASS: &str = "TaskbarWeatherLabel";

pub(crate) fn raw(h: HWND) -> isize {
    h.0 as isize
}

/// The shared look (style + Windows settings), as last refreshed.
pub(crate) fn look() -> Look {
    with(|a| a.look.clone()).unwrap_or_else(Look::current)
}

/// Re-reads the shared style and the Windows settings; true if anything changed.
pub(crate) fn refresh_look() -> bool {
    let new = Look::current();
    with(|a| {
        let changed = a.look != new;
        a.look = new;
        changed
    }) == Some(true)
}

/// The label sits on the right (stacked against the tray) rather than at the left edge.
pub(crate) fn on_right(cfg: &Config) -> bool {
    match cfg.label_position.as_str() {
        "left" => false,
        "right" => true,
        _ => unsafe { dock::left_aligned() },
    }
}

pub(crate) fn scale_of(tb: HWND) -> f32 {
    system::scale_for(raw(tb))
}

pub(crate) fn gap_px(scale: f32) -> i32 {
    (look().gap * scale).round() as i32
}

/// Screen x of the taskbar's left edge (the label is positioned in taskbar client coordinates).
pub(crate) fn tb_left(tb: HWND) -> i32 {
    unsafe { dock::rect(raw(tb)).left }
}

fn motion<R>(f: impl FnOnce(&mut strip::Motion) -> R) -> Option<R> {
    with(|a| f(&mut a.motion))
}

fn timer(id: usize, on: bool) {
    let Some(main) = with(|a| a.main) else { return };
    unsafe {
        if on {
            SetTimer(main, id, 10, None);
        } else {
            let _ = KillTimer(main, id);
        }
    }
}

// ---------------------------------------------------------------- mouse on the label
// (the rules for dragging, gliding and zooming are in taskbar_dock::strip)

/// How far (taskbar client x) a label `width` wide may go while dragged: against the tray,
/// and (docked on the right) not over the taskbar's own buttons.
fn limits(hwnd: HWND, width: i32) -> strip::Limits {
    let Ok(tb) = (unsafe { GetParent(hwnd) }) else { return strip::Limits::NONE };
    let (scale, tl) = (scale_of(tb), tb_left(tb));
    let right = with(|a| on_right(&a.cfg)) == Some(true);
    let min = dock::icons_right(raw(tb)).filter(|_| right).map(|r| r + gap_px(scale) - tl);
    let max = unsafe { dock::tray_boundary(raw(tb), scale) } - tl - width - gap_px(scale);
    strip::Limits { min, max: Some(max) }
}

pub(crate) fn mouse_down(hwnd: HWND) {
    let lx = unsafe { GetParent(hwnd) }.map(|tb| label_client_x(hwnd, tb)).unwrap_or(0);
    let cx = unsafe { dock::cursor() }.x;
    motion(|m| m.press(cx, lx));
}

pub(crate) fn mouse_drag(hwnd: HWND) {
    let mut rc = RECT::default();
    unsafe { let _ = GetWindowRect(hwnd, &mut rc); }
    let (cx, lk) = (unsafe { dock::cursor() }.x, look());
    let lim = limits(hwnd, rc.right - rc.left);
    match motion(|m| m.mouse_move(cx, rc.right - rc.left, lim, &lk, Instant::now())) {
        Some(DragMove::Started(x)) => {
            timer(TIMER_GLIDE, false);
            unsafe { dock::set_dragging(raw(hwnd), true); }
            if let Some(a) = with(|a| a.panel.drag_started()) {
                panel_do(a); // a hover-opened flyout closes
            }
            timer(TIMER_ZOOM, true); // picked up: content grows a little
            unsafe { dock::notify(raw(hwnd)); } // the other widgets close up
            drag_move(hwnd, x);
        }
        Some(DragMove::Moved(x)) => drag_move(hwnd, x),
        _ => {}
    }
}

/// Button released (`click`), or the capture lost (Esc, a window took the mouse): a drag ends
/// like a drop either way; `click` also releases the capture and toggles the flyout.
pub(crate) fn mouse_up(hwnd: HWND, click: bool) {
    let release = motion(|m| m.release(Instant::now())).unwrap_or(Release::None);
    if click {
        unsafe { let _ = ReleaseCapture(); } // its WM_CAPTURECHANGED then finds nothing left to end
    }
    match release {
        Release::Drop => drag_ended(hwnd), // snaps into the stack next to the others
        _ if click => toggle_popup(),
        _ => {}
    }
}

/// One frame of the zoom: redraw at the new size (which also heads for the slot after a drop);
/// while dragged keep it centred under the mouse.
pub(crate) fn zoom_step() {
    let lk = look();
    let Some(Some(step)) = motion(|m| m.zoom_step(&lk, Instant::now())) else {
        return timer(TIMER_ZOOM, false);
    };
    redraw_label();
    if let Some(Some(hwnd)) = with(|a| a.label) {
        let mut rc = RECT::default();
        unsafe { let _ = GetWindowRect(hwnd, &mut rc); }
        let cx = unsafe { dock::cursor() }.x;
        let lim = limits(hwnd, rc.right - rc.left);
        if let Some(Some(x)) = motion(|m| m.drag_x(cx, rc.right - rc.left, lim)) {
            drag_move(hwnd, x);
        }
    }
    if step.done {
        timer(TIMER_ZOOM, false);
    }
}

fn drag_ended(hwnd: HWND) {
    timer(TIMER_ZOOM, true); // put down: back to normal size while it glides into place
    unsafe {
        with(|a| a.panel.drag_ended());
        // save the new order first, then end the drag: the others keep the preview till then
        if let Ok(tb) = GetParent(hwnd) {
            if with(|a| on_right(&a.cfg)) == Some(true) {
                let scale = scale_of(tb);
                dock::drop_reorder(raw(tb), raw(hwnd), MY_CLASS, dock::tray_boundary(raw(tb), scale), gap_px(scale));
            }
        }
        dock::set_dragging(raw(hwnd), false);
    }
    reposition_label_with(true);
    unsafe { dock::notify(raw(hwnd)); }
}

pub(crate) fn destroy_label() {
    if let Some(Some(h)) = with(|a| a.label.take()) {
        unsafe { let _ = DestroyWindow(h); }
    }
}

/// Explorer restarted: keeps the label if `watch_label` already put it on the new taskbar,
/// otherwise replaces it. Forgetting a live label left an untracked duplicate on the taskbar
/// (frozen, ignoring clicks; seen 2026-10-10).
pub(crate) fn taskbar_restarted() {
    let Some(label) = with(|a| a.label) else { return };
    if let Some(h) = label {
        let current = unsafe { IsWindow(h).as_bool() && GetParent(h).ok() == taskbar() };
        if current {
            return;
        }
        destroy_label();
    }
    create_label();
}

/// The label's content in full or compact mode.
pub(crate) fn content_for(a: &App, compact: bool) -> LabelContent {
    let mut c = full_content(a);
    if compact {
        c.desc.clear(); // temperature over humidity only
    }
    c
}

pub(crate) fn full_content(a: &App) -> LabelContent {
    let text = |line1: &str, line2: String, desc: &str| LabelContent { glyph: "☁".into(), line1: line1.into(), line2, desc: desc.into() };
    match &a.weather {
        Some(wx) => {
            // temperature over humidity, the conditions beside them
            let (glyph, desc) = describe(wx.code, wx.is_day);
            let line2 = if wx.humidity.is_finite() { format!("{:.0}%", wx.humidity) } else { String::new() };
            LabelContent { glyph: glyph.into(), line1: format!("{}{}", deg(wx.temp), &wx.unit()[2..]), line2, desc: desc.into() }
        }
        None if a.error.is_some() => text("--", String::new(), "Offline"),
        None => text("…", String::new(), "Weather"),
    }
}

/// On the right: the label's slot in the dock stack and its fit next to the taskbar's buttons
/// (None: the taskbar's list can't be trusted right now, e.g. the action center is open).
fn right_slot(tb: HWND, me: HWND, size: dock::Size, scale: f32) -> Option<dock::Slot> {
    unsafe { dock::slot(raw(tb), raw(me), size, dock::tray_boundary(raw(tb), scale), gap_px(scale)) }
}

/// Picks the x (taskbar client coordinates) for a label drawn `width` wide. On the right: its
/// `slot` in the dock stack against the tray (None: stay where it is). On the left (centred
/// taskbar icons, or label_position "left"): from the left edge, after Windows' Widgets button,
/// around other apps' docked windows.
pub(crate) fn label_x(tb: HWND, me: HWND, client: &RECT, width: i32, scale: f32, cfg: &Config, slot: Option<dock::Slot>) -> i32 {
    let gap = gap_px(scale);
    let tl = tb_left(tb);
    unsafe {
        if on_right(cfg) {
            return slot.map_or(label_client_x(me, tb), |s| s.x_for(width) - tl);
        }
        let widgets_w = if !dock::left_aligned() && dock::widgets_button() { (170.0 * scale) as i32 } else { 0 };
        let mut x = gap + widgets_w;
        let mut occupied: Vec<(i32, i32)> = dock::children(raw(tb))
            .iter()
            .filter(|c| c.hwnd != raw(me) && !c.squeezed && !dock::is_taskbar_part(c, client.right))
            .map(|c| (c.rc.left - tl, c.rc.right - tl))
            .collect();
        occupied.sort();
        for (l, r) in occupied {
            if l < x + width && r > x {
                x = r + gap;
            }
        }
        x
    }
}

/// Compact mode switched since the last drawing (the panel's switch or the settings file):
/// animate the width from the current one to `to` (the new mode's), for a shown label. Returns
/// the width to draw at while that runs.
fn start_resize(hwnd: HWND, compact: bool, to: impl FnOnce() -> i32) -> Option<i32> {
    let switched = with(|a| a.drawn_compact.replace(compact)).flatten().is_some_and(|c| c != compact);
    if switched && unsafe { IsWindowVisible(hwnd) }.as_bool() {
        let r = unsafe { dock::rect(raw(hwnd)) };
        let from = motion(|m| m.resize_width()).flatten().unwrap_or(r.right - r.left);
        let (to, lk) = (to(), look());
        if motion(|m| m.resize(from, to, &lk, Instant::now())) == Some(true) {
            timer(TIMER_RESIZE, true);
        }
    }
    motion(|m| m.resize_width()).flatten()
}

/// One frame of the collapse/expand animation (the last one draws the new mode).
pub(crate) fn resize_step() {
    let lk = look();
    let step = motion(|m| m.resize_step(&lk, Instant::now())).flatten();
    if step.map_or(true, |s| s.done) {
        timer(TIMER_RESIZE, false);
    }
    redraw_label();
}

/// The room next to the taskbar's buttons changed (dock::fit): hide or show the label, and with
/// `redraw` also redraw it compact or full (redraw_label shows it and puts it in place).
fn apply_fit(hwnd: HWND, fit: dock::Fit, redraw: bool) -> dock::FitChange {
    let Some(change) = with(|a| a.fit.update(fit)) else { return dock::FitChange::None };
    unsafe {
        match change {
            dock::FitChange::Hide => {
                popup_closed(false);
                taskbar_dock::menu_panel::close();
                dock::set_squeezed(raw(hwnd), true); // stays in the stack's order while hidden
                let _ = ShowWindow(hwnd, SW_HIDE);
                dock::notify(raw(hwnd));
                log("no room next to the taskbar's buttons: label hidden");
            }
            dock::FitChange::Show => {
                dock::set_squeezed(raw(hwnd), false);
                log("room again next to the taskbar's buttons: label shown");
                if redraw {
                    redraw_label();
                }
            }
            dock::FitChange::Redraw if redraw => redraw_label(),
            _ => {}
        }
    }
    change
}

pub(crate) fn redraw_label() {
    let Some(Some((hwnd, full, compact_c, hover, cfg, zoom))) =
        with(|a| a.label.map(|h| (h, full_content(a), content_for(a, true), a.hover, a.cfg.clone(), a.motion.zoom())))
    else { return };
    let lk = look();
    unsafe {
        let Ok(tb) = GetParent(hwnd) else { return };
        let mut rc = RECT::default();
        let _ = GetClientRect(tb, &mut rc);
        let scale = scale_of(tb);
        let height = rc.bottom - rc.top;
        if height <= 0 {
            return;
        }
        // on the right it's part of the dock stack against the tray; on the left it isn't
        let right = on_right(&cfg);
        if right {
            dock::mark_member(raw(hwnd));
        } else {
            dock::leave(raw(hwnd));
        }
        // its widths for the fit: in the chosen mode, and compact
        let measure = |c: &LabelContent| render::update_label(hwnd, c, height, scale, &lk, hover, None, true).cx;
        let size = dock::Size { full: measure(if cfg.compact { &compact_c } else { &full }), min: measure(&compact_c), stretch: false };
        with(|a| a.size = Some(size));
        if right && zoom == 1.0 {
            dock::publish_widths(raw(hwnd), size);
        }
        // the room next to the taskbar's buttons: compact, or hidden while there's none
        let slot = if right { right_slot(tb, hwnd, size, scale) } else { None };
        let fit = if right { slot.map(|s| s.fit) } else { Some(dock::Fit::Full) };
        if let Some(f) = fit.filter(|_| !dragging()) {
            apply_fit(hwnd, f, false);
        }
        if with(|a| a.fit.hidden()) == Some(true) {
            return;
        }
        // compact mode switched (chosen, or for room): the full label narrows or widens
        let compact = cfg.compact || with(|a| a.fit.compact()) == Some(true);
        let content = if compact { &compact_c } else { &full };
        let resize_w = start_resize(hwnd, compact, || measure(content));
        let shown = if resize_w.is_some() { &full } else { content };
        let before = dock::rect(raw(hwnd));
        let drawn = render::update_label(hwnd, shown, height, scale * zoom, &lk, hover, resize_w, false);
        if !IsWindowVisible(hwnd).as_bool() {
            // appearing (room again): straight into place
            let x = label_x(tb, hwnd, &rc, drawn.cx, scale, &cfg, slot);
            motion(|m| m.stop_glide());
            let _ = SetWindowPos(hwnd, HWND_TOP, x, 0, drawn.cx, drawn.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW | SET_WINDOW_POS_FLAGS(dock::raise_flag(raw(hwnd))));
            dock::notify(raw(hwnd));
            return;
        }
        // size and z-order now; the position goes through move_label_to (slide or jump)
        let _ = SetWindowPos(hwnd, HWND_TOP, 0, 0, drawn.cx, drawn.cy, SWP_NOMOVE | SWP_NOACTIVATE | SET_WINDOW_POS_FLAGS(dock::raise_flag(raw(hwnd))));
        if right && motion(|m| m.settled()) == Some(true) {
            // docked against the tray: a new width keeps the right edge, not the left
            strip::keep_right_edge(raw(hwnd), raw(tb), before);
        }
        let x = label_x(tb, hwnd, &rc, drawn.cx, scale, &cfg, slot);
        if !dragging() {
            move_label_to(hwnd, tb, x, member_dragged(tb, hwnd));
        }
    }
}

pub(crate) fn dragging() -> bool {
    motion(|m| m.dragging()) == Some(true)
}

/// Is another dock member being dragged? Then every move is a slide, so the live preview flows.
pub(crate) fn member_dragged(tb: HWND, me: HWND) -> bool {
    unsafe { dock::children(raw(tb)).iter().any(|c| c.member && c.dragging && c.hwnd != raw(me)) }
}

/// The label's current x in taskbar client coordinates.
pub(crate) fn label_client_x(hwnd: HWND, tb: HWND) -> i32 {
    unsafe { dock::rect(raw(hwnd)).left - tb_left(tb) }
}

/// Moves the label to x (taskbar client coordinates); taskbar_dock::strip decides between a
/// slide and a jump.
pub(crate) fn move_label_to(hwnd: HWND, tb: HWND, x: i32, always_slide: bool) {
    let (cur, lk) = (label_client_x(hwnd, tb), look());
    match motion(|m| m.glide_to(cur, x, always_slide, &lk, Instant::now())) {
        Some(Glide::Jump(x)) => {
            timer(TIMER_GLIDE, false);
            set_x(hwnd, x);
        }
        Some(Glide::Slide) => timer(TIMER_GLIDE, true),
        _ => {}
    }
}

fn set_x(hwnd: HWND, x: i32) {
    unsafe { let _ = SetWindowPos(hwnd, None, x, 0, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE); }
}

/// One frame of a slide.
pub(crate) fn glide_step() {
    let lk = look();
    let step = motion(|m| m.glide_step(&lk, Instant::now())).flatten();
    let (Some(Some(hwnd)), Some(step)) = (with(|a| a.label), step) else {
        motion(|m| m.stop_glide());
        return timer(TIMER_GLIDE, false);
    };
    set_x(hwnd, step.value);
    if step.done {
        timer(TIMER_GLIDE, false);
    }
}

/// Recomputes only the position (no redraw): used when a neighbour moved.
pub(crate) fn reposition_label() {
    reposition_label_with(false);
}

pub(crate) fn reposition_label_with(always_slide: bool) {
    if dragging() {
        return; // the user has it
    }
    let Some(Some((hwnd, cfg, size))) = with(|a| a.label.map(|h| (h, a.cfg.clone(), a.size))) else { return };
    unsafe {
        let Ok(tb) = GetParent(hwnd) else { return };
        let mut client = RECT::default();
        let _ = GetClientRect(tb, &mut client);
        let mut rc = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rc);
        let (width, scale) = (rc.right - rc.left, scale_of(tb));
        let slot = if on_right(&cfg) { right_slot(tb, hwnd, size.unwrap_or(dock::Size::fixed(width)), scale) } else { None };
        // the room next to the taskbar's buttons changed: redrawn compact or full, or hidden
        if let Some(s) = slot {
            if apply_fit(hwnd, s.fit, true) != dock::FitChange::None {
                return;
            }
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return;
        }
        let x = label_x(tb, hwnd, &client, width, scale, &cfg, slot);
        move_label_to(hwnd, tb, x, always_slide || member_dragged(tb, hwnd));
    }
}

/// While dragging: put the label at x (taskbar client coordinates), never past the tray boundary.
pub(crate) fn drag_move(hwnd: HWND, x: i32) {
    unsafe {
        let Ok(tb) = GetParent(hwnd) else { return };
        let mut rc = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rc);
        let scale = scale_of(tb);
        let b = dock::tray_boundary(raw(tb), scale);
        let max_x = b - tb_left(tb) - (rc.right - rc.left) - gap_px(scale);
        let _ = SetWindowPos(hwnd, HWND_TOP, x.min(max_x), 0, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SET_WINDOW_POS_FLAGS(dock::raise_flag(raw(hwnd))));
        dock::track_drag(raw(tb), raw(hwnd), b, gap_px(scale)); // the slot it would drop into
    }
}

/// A neighbour (tray, another widget) changed: reposition right away instead of waiting for the
/// 2-second watchdog. (windows-sys signature: the library installs it.)
pub(crate) unsafe extern "system" fn on_win_event(_: isize, _event: u32, hwnd: isize, id_object: i32, _: i32, _: u32, _: u32) {
    let Some(tb) = taskbar() else { return };
    let Some(Some(label)) = with(|a| a.label) else { return };
    if dock::is_neighbour_event(hwnd, id_object, raw(tb), raw(label)) {
        reposition_label();
    }
}

/// Keeps one event hook per process that owns taskbar child windows; re-hooks when they restart.
pub(crate) fn update_hooks() {
    let Some(mut hooks) = with(|a| std::mem::take(&mut a.hooks)) else { return };
    unsafe { hooks.update(taskbar().map_or(0, raw), Some(on_win_event)); }
    with(|a| a.hooks = hooks);
}

/// Keeps the label alive, on top of the taskbar's own content and out of other widgets' way,
/// and follows the shared style and Windows settings live (redrawn below either way).
pub(crate) fn watch_label() {
    if refresh_look() {
        update_tray(false);
        if let Some(Some(p)) = with(|a| a.popup) {
            unsafe { let _ = InvalidateRect(p, None, false); }
        }
    }
    let Some((enabled, label)) = with(|a| (a.cfg.taskbar_label, a.label)) else { return };
    unsafe {
        match label {
            Some(h) if !IsWindow(h).as_bool() => {
                with(|a| a.label = None);
                if enabled { create_label(); }
            }
            Some(_) => redraw_label(),
            None if enabled => create_label(),
            None => {}
        }
    }
    update_hooks();
}
