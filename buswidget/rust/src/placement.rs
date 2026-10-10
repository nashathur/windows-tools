//! Where the strip sits: dock position, gliding, dragging, zoom (rules in taskbar_dock::strip),
//! WinEvent hooks, creating the strip.

use super::*;
use std::time::Instant;
use taskbar_dock::strip::{self, DragMove, Glide, Release};

pub(crate) const TIMER_TICK: usize = 1;
pub(crate) const TIMER_GLIDE: usize = 2;
pub(crate) const TIMER_ZOOM: usize = 3;
pub(crate) const TIMER_HOVER: usize = 4;
pub(crate) const TIMER_FADE: usize = 5;
pub(crate) const TIMER_RESIZE: usize = 6;

fn motion<R>(f: impl FnOnce(&mut strip::Motion) -> R) -> R {
    ui(|u| f(&mut u.motion.borrow_mut()))
}

unsafe fn timer(id: usize, on: bool) {
    let popup = ui(|u| u.popup.get());
    if on {
        SetTimer(popup, id, 10, None);
    } else {
        KillTimer(popup, id);
    }
}

/// One frame of the zoom.
pub(crate) unsafe fn zoom_step() {
    let Some(step) = motion(|m| m.zoom_step(&look(), Instant::now())) else {
        return timer(TIMER_ZOOM, false);
    };
    render_bar(&bar_text());
    let (rc, cx) = (bar_rect(), dock::cursor().x);
    let lim = limits(rc.right - rc.left);
    match motion(|m| m.drag_x(cx, rc.right - rc.left, lim)) {
        Some(x) => move_bar(x, false), // keep it centred under the mouse while it grows
        None => snap_now(true),        // shrinking after a drop: keep heading for the (changing) slot
    }
    if step.done {
        timer(TIMER_ZOOM, false);
    }
}

/// Compact mode switched since the last drawing (the panel's switch or the settings file):
/// animate the width from the current one to the new mode's (a shown, docked strip only).
/// Returns the width to draw at while that runs.
pub(crate) unsafe fn start_resize(compact: bool) -> Option<i32> {
    let (h, embedded) = ui(|u| (u.bar.get(), u.embedded.get()));
    let switched = ui(|u| u.drawn_compact.replace(Some(compact))).is_some_and(|c| c != compact);
    if switched && embedded && IsWindowVisible(h) != 0 {
        let rc = bar_rect();
        let from = motion(|m| m.resize_width()).unwrap_or(rc.right - rc.left);
        if let Some(to) = bar_width(compact) {
            let lk = look();
            if motion(|m| m.resize(from, to, &lk, Instant::now())) {
                timer(TIMER_RESIZE, true);
            }
        }
    }
    motion(|m| m.resize_width())
}

/// One frame of the collapse/expand animation (the last one draws the new mode).
pub(crate) unsafe fn resize_step() {
    let lk = look();
    let step = motion(|m| m.resize_step(&lk, Instant::now()));
    if step.map_or(true, |s| s.done) {
        timer(TIMER_RESIZE, false);
    }
    render_bar(&bar_text());
}

/// Move to a new spot (see taskbar_dock::strip for when it slides).
pub(crate) unsafe fn glide_to(x: i32, always_slide: bool) {
    match motion(|m| m.glide_to(bar_rect().left, x, always_slide, &look(), Instant::now())) {
        Glide::Stay => {}
        Glide::Jump(x) => move_bar(x, false),
        Glide::Slide => timer(TIMER_GLIDE, true),
    }
}

/// One frame of a slide.
pub(crate) unsafe fn glide_step() {
    let Some(step) = motion(|m| m.glide_step(&look(), Instant::now())) else {
        return timer(TIMER_GLIDE, false);
    };
    move_bar(step.value, false);
    if step.done {
        timer(TIMER_GLIDE, false);
    }
}

/// Go to our place in the stack now (not while pressed or dragged). While another widget is
/// being dragged every move is a slide, so the live preview flows.
pub(crate) unsafe fn snap_now(always_slide: bool) {
    if IsWindow(ui(|u| u.bar.get())) == 0 || motion(|m| m.pressed()) {
        return;
    }
    if let Some(s) = dock_slot() {
        if !apply_fit(s.fit) && s.fit != dock::Fit::Hidden {
            let rc = bar_rect();
            glide_to(s.x_for(rc.right - rc.left), always_slide || s.slide);
        }
    }
}

/// The room next to the taskbar's buttons changed (dock::fit): redraw compact or full (it
/// animates and keeps its right edge), or hide / show the strip. True if anything changed:
/// then the redraw or the next tick takes care of the position.
pub(crate) unsafe fn apply_fit(fit: dock::Fit) -> bool {
    let change = ui(|u| u.fit.borrow_mut().update(fit));
    let bar = ui(|u| u.bar.get());
    match change {
        dock::FitChange::None => return false,
        dock::FitChange::Redraw => {
            render_bar(&bar_text());
        }
        dock::FitChange::Hide => {
            if IsWindowVisible(ui(|u| u.popup.get())) != 0 {
                popup_closed(false);
            }
            menu_panel::close();
            dock::set_squeezed(bar, true); // stays in the stack's order while hidden
            ShowWindow(bar, SW_HIDE);
            dock::notify(bar);
            data::log(data_dir(), "no room next to the taskbar's buttons: strip hidden");
        }
        dock::FitChange::Show => {
            dock::set_squeezed(bar, false);
            render_bar(&bar_text());
            if let Some(s) = dock_slot() {
                let rc = bar_rect();
                move_bar(s.x_for(rc.right - rc.left), true);
            }
            ShowWindow(bar, SW_SHOWNOACTIVATE);
            dock::notify(bar);
            data::log(data_dir(), "room again next to the taskbar's buttons: strip shown");
        }
    }
    true
}

// ---- mouse on the strip ------------------------------------------------------

/// How far a strip `width` wide may go while dragged: against the tray, and not over the
/// taskbar's own buttons (none when floating).
unsafe fn limits(width: i32) -> strip::Limits {
    let Some(b) = boundary() else { return strip::Limits::NONE };
    let tray = ui(|u| u.tray.get());
    strip::Limits { min: dock::icons_right(tray).map(|r| r + gap()), max: Some(b - width - gap()) }
}

pub(crate) unsafe fn mouse_down() {
    let x = bar_rect().left;
    motion(|m| m.press(dock::cursor().x, x));
}

pub(crate) unsafe fn mouse_drag() {
    let (rc, cx) = (bar_rect(), dock::cursor().x);
    let lim = limits(rc.right - rc.left);
    let mv = motion(|m| m.mouse_move(cx, rc.right - rc.left, lim, &look(), Instant::now()));
    match mv {
        DragMove::None => {}
        DragMove::Started(x) => {
            let bar = ui(|u| u.bar.get());
            timer(TIMER_GLIDE, false);
            dock::set_dragging(bar, true);
            panel_do(ui(|u| u.panel.borrow_mut().drag_started())); // a hover-opened popup closes
            timer(TIMER_ZOOM, true); // picked up: content grows a little
            dock::notify(bar); // the other widgets close up
            move_bar(x, false);
        }
        DragMove::Moved(x) => move_bar(x, false),
    }
}

/// Button released, or the capture lost (Esc, a window took the mouse; then `click` is false:
/// an interrupted drag ends like a drop, an interrupted click does nothing; `true` also releases
/// the capture).
pub(crate) unsafe fn mouse_up(click: bool) {
    let release = motion(|m| m.release(Instant::now()));
    if click {
        ReleaseCapture(); // its MSG_CAPTURECHANGED then finds nothing left to end
    }
    match release {
        Release::None => {}
        Release::Click => {
            if click {
                toggle_popup();
            }
        }
        Release::Drop => drag_ended(),
    }
}

unsafe fn drag_ended() {
    let (bar, embedded, tray) = ui(|u| (u.bar.get(), u.embedded.get(), u.tray.get()));
    ui(|u| u.panel.borrow_mut().drag_ended());
    timer(TIMER_ZOOM, true); // put down: back to normal size while it glides into place
    if embedded {
        // save the new order first, then end the drag: the others keep the preview till then
        if let Some(b) = boundary() {
            dock::drop_reorder(tray, bar, MY_CLASS, b, gap());
        }
        dock::set_dragging(bar, false);
        snap_now(true);
        dock::notify(bar);
    } else {
        dock::set_dragging(bar, false);
        // floating (not attached to the taskbar): stay where dropped
        let _ = fs::write(data_dir().join("pos.txt"), bar_rect().left.to_string());
    }
}

/// A neighbour (tray, another widget) changed: take our place right away.
pub(crate) unsafe extern "system" fn on_win_event(_hook: HWINEVENTHOOK, _event: u32, hwnd: HWND, id_object: i32, _child: i32, _thread: u32, _time: u32) {
    let (tray, bar) = ui(|u| (u.tray.get(), u.bar.get()));
    if dock::is_neighbour_event(hwnd, id_object, tray, bar) {
        snap_now(false);
    }
}

pub(crate) unsafe fn update_hooks() {
    let tray = ui(|u| u.tray.get());
    ui(|u| u.hooks.borrow_mut().update(tray, Some(on_win_event)));
}

/// Put the strip at screen x (never past the tray boundary, also when dragged), vertically
/// centred on the taskbar. `raise` re-asserts its z-order.
pub(crate) unsafe fn move_bar(screen_x: i32, raise: bool) {
    let (h, tray) = ui(|u| (u.bar.get(), u.tray.get()));
    let rc = bar_rect();
    let screen_x = match boundary() {
        Some(b) => screen_x.min(b - (rc.right - rc.left) - gap()),
        None => screen_x,
    };
    strip::move_strip(h, if tray != 0 { tray } else { dock::taskbar() }, screen_x, raise);
    if let (true, Some(b)) = (tray != 0, boundary()) {
        dock::track_drag(tray, h, b, gap()); // while dragged: the slot it would drop into
    }
}

/// Create the strip: attached to the taskbar if possible, else floating above it.
pub(crate) unsafe fn create_bar() {
    let hinst = ui(|u| u.hinst.get());
    let (cls, title) = (wide("BusWidgetBar"), wide("Bus widget"));
    let tray = dock::taskbar();
    let mut bar = 0;
    if tray != 0 {
        bar = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW,
            cls.as_ptr(),
            title.as_ptr(),
            WS_CHILD | WS_CLIPSIBLINGS,
            0, 0, 10, 10,
            tray, 0, hinst, null(),
        );
    }
    let mut embedded = bar != 0;
    ui(|u| {
        u.bar.set(bar);
        u.embedded.set(embedded);
        u.tray.set(tray);
        u.shown.borrow_mut().clear();
        *u.fit.borrow_mut() = dock::FitState::default(); // a new strip starts full and shown
        u.drawn_compact.set(None);
    });
    // a transparent child window needs Windows 8+ support declared in the exe manifest;
    // if drawing it fails anyway, fall back to the floating window
    if !embedded || !render_bar(&bar_text()) {
        if bar != 0 {
            DestroyWindow(bar);
        }
        bar = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            cls.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0, 0, 10, 10,
            0, 0, hinst, null(),
        );
        embedded = false;
        ui(|u| {
            u.bar.set(bar);
            u.embedded.set(false);
        });
        render_bar(&bar_text());
    }
    if embedded {
        // join the dock stack shared with the other widgets (see dock_slot)
        dock::mark_member(bar);
    }
    let x = dock_slot()
        .map(|s| { let rc = bar_rect(); s.x_for(rc.right - rc.left) })
        .or_else(saved_x)
        .unwrap_or((GetSystemMetrics(SM_CXSCREEN) as f32 * 0.68) as i32);
    move_bar(x, true);
    ShowWindow(bar, SW_SHOWNOACTIVATE);
    dock::notify(bar);
    data::log(data_dir(), if embedded { "strip attached to the taskbar" } else { "strip floating above the taskbar" });
}
