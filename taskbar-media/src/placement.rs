//! Where the strip sits in the taskbar: dock position, gliding, dragging and zoom (the rules are
//! shared: `taskbar_dock::strip`), and the WinEvent hooks that notice other dock members moving.
//! All x values are screen coordinates of the strip's left edge.

use super::*;
use taskbar_dock::strip::{self, DragMove, Glide, Release};

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

/// The strip and its taskbar, while attached (a floating strip has no dock place to move in).
fn attached() -> Option<(HWND, HWND)> {
    let h = with(|a| a.strip.filter(|_| !a.floating)).flatten()?;
    let tb = unsafe { GetParent(h).ok()? };
    Some((h, tb))
}

fn width(h: HWND) -> i32 {
    let r = unsafe { dock::rect(raw(h)) };
    r.right - r.left
}

/// The strip's place in the dock stack against the tray, and whether another member is being
/// dragged (then every move is a slide, so the live preview flows). Not in the taskbar's list
/// right now (action center open): stay where it is.
/// `width`: as drawn right now (the slot keeps the right edge while it animates or is zoomed).
pub(crate) fn slot(tb: HWND, me: HWND, width: i32, scale: f32) -> (i32, bool) {
    full_slot(tb, me, width, scale).map_or((unsafe { dock::rect(raw(me)).left }, false), |s| (s.x_for(width), s.slide))
}

/// The strip's dock slot with its fit next to the taskbar's buttons (dock::slot), for the
/// widths it published. None: the taskbar's list can't be trusted right now.
fn full_slot(tb: HWND, me: HWND, width: i32, scale: f32) -> Option<dock::Slot> {
    let size = with(|a| a.size).flatten().unwrap_or(dock::Size::fixed(width));
    unsafe { dock::slot(raw(tb), raw(me), size, dock::tray_boundary(raw(tb), scale), gap_px(scale)) }
}

/// Every second and when a neighbour moved: the room next to the taskbar's buttons (dock::fit).
/// Narrower or wider: redraw (it animates); none at all: hide until there is again.
pub(crate) fn check_fit() {
    let Some(Some((h, away))) = with(|a| a.strip.filter(|_| !a.floating).map(|h| (h, a.shown.is_none()))) else { return };
    if motion(|m| m.pressed()) != Some(false) {
        return;
    }
    if away {
        // nothing to show: the strip is away (faded out), not squeezed; it fits afresh later
        if with(|a| a.fit.hidden()) == Some(true) {
            unsafe { dock::set_squeezed(raw(h), false); }
        }
        with(|a| a.fit = dock::FitState::default());
        return;
    }
    let Ok(tb) = (unsafe { GetParent(h) }) else { return };
    if let Some(s) = full_slot(tb, h, width(h), scale_of(tb)) {
        apply_fit(h, s.fit);
    }
}

fn apply_fit(h: HWND, fit: dock::Fit) {
    let Some(change) = with(|a| a.fit.update(fit)) else { return };
    unsafe {
        match change {
            dock::FitChange::None => {}
            dock::FitChange::Redraw => {
                with(|a| a.drawn = None);
                redraw_strip();
            }
            dock::FitChange::Hide => {
                popup::closed(false);
                dock::set_squeezed(raw(h), true); // stays in the stack's order while hidden
                let _ = ShowWindow(h, SW_HIDE);
                with(|a| a.drawn = None);
                dock::notify(raw(h));
                log("no room next to the taskbar's buttons: strip hidden");
            }
            dock::FitChange::Show => {
                dock::set_squeezed(raw(h), false);
                with(|a| a.drawn = None);
                log("room again next to the taskbar's buttons: strip shown");
                redraw_strip(); // appears: jumps into place and fades in
            }
        }
    }
}

/// The furthest right a strip `w` wide may go: against the tray.
fn max_x(tb: HWND, w: i32) -> i32 {
    let scale = scale_of(tb);
    let b = unsafe { dock::tray_boundary(raw(tb), scale) };
    b - w - gap_px(scale)
}

/// How far a strip `w` wide may go while dragged: against the tray, and not over the taskbar's
/// own buttons.
fn limits(tb: HWND, w: i32) -> strip::Limits {
    let min = dock::icons_right(raw(tb)).map(|r| r + gap_px(scale_of(tb)));
    strip::Limits { min, max: Some(max_x(tb, w)) }
}

/// Moves the strip to x, never past the tray boundary (also while dragged); `raise` puts it on
/// top of its neighbours (while dragged).
fn put(h: HWND, tb: HWND, x: i32, raise: bool) {
    unsafe {
        strip::move_strip(raw(h), raw(tb), x.min(max_x(tb, width(h))), raise);
        // while dragged: the slot it would drop into (does nothing otherwise)
        let scale = scale_of(tb);
        dock::track_drag(raw(tb), raw(h), dock::tray_boundary(raw(tb), scale), gap_px(scale));
    }
}

/// Head for x: a slide or a jump (rules in `taskbar_dock::strip`).
pub(crate) fn glide_to(h: HWND, tb: HWND, x: i32, always_slide: bool) {
    let cur = unsafe { dock::rect(raw(h)).left };
    match motion(|m| m.glide_to(cur, x, always_slide, &look(), Instant::now())) {
        Some(Glide::Jump(x)) => put(h, tb, x, false),
        Some(Glide::Slide) => timer(TIMER_GLIDE, true),
        _ => {}
    }
}

/// Appearing: straight to x, no slide.
pub(crate) fn jump_to(h: HWND, tb: HWND, x: i32) {
    motion(|m| m.stop_glide());
    put(h, tb, x, false);
}

pub(crate) fn glide_step() {
    let step = motion(|m| m.glide_step(&look(), Instant::now())).flatten();
    let (Some(step), Some((h, tb))) = (step, attached()) else {
        motion(|m| m.stop_glide());
        return timer(TIMER_GLIDE, false);
    };
    put(h, tb, step.value, false);
    if step.done {
        timer(TIMER_GLIDE, false);
    }
}

/// Recomputes only the position (no redraw): a neighbour moved. Not while pressed or dragged.
pub(crate) fn reposition_strip(always_slide: bool) {
    if motion(|m| m.pressed()) != Some(false) {
        return;
    }
    let Some((h, tb)) = attached() else { return };
    unsafe {
        if !IsWindowVisible(h).as_bool() {
            return;
        }
    }
    let (x, other_dragged) = slot(tb, h, width(h), scale_of(tb));
    glide_to(h, tb, x, always_slide || other_dragged);
}

// ---------------------------------------------------------------- the mouse on the strip

pub(crate) fn mouse_down(h: HWND) {
    let x = unsafe { dock::rect(raw(h)).left };
    motion(|m| m.press(unsafe { dock::cursor().x }, x));
}

/// The mouse moved with the button down: past the threshold it becomes a drag (attached only;
/// floating, there is no dock order to drag in).
pub(crate) fn mouse_drag() {
    let Some((h, tb)) = attached() else { return };
    let (cx, w) = (unsafe { dock::cursor().x }, width(h));
    match motion(|m| m.mouse_move(cx, w, limits(tb, w), &look(), Instant::now())) {
        Some(DragMove::Started(x)) => {
            timer(TIMER_GLIDE, false);
            unsafe { dock::set_dragging(raw(h), true); }
            if let Some(a) = with(|a| a.panel.drag_started()) {
                popup::act(a); // a hover-opened panel closes
            }
            timer(TIMER_ZOOM, true); // picked up: the content grows a little
            unsafe { dock::notify(raw(h)); } // the others close up
            put(h, tb, x, true);
        }
        Some(DragMove::Moved(x)) => put(h, tb, x, true),
        _ => {}
    }
}

/// The button was released, or the capture lost. A drop ends here; a click is the caller's.
pub(crate) fn mouse_up(h: HWND) -> Release {
    let r = motion(|m| m.release(Instant::now())).unwrap_or(Release::None);
    if r == Release::Drop {
        drag_ended(h);
    }
    r
}

fn drag_ended(h: HWND) {
    timer(TIMER_ZOOM, true); // put down: back to normal size while it glides into place
    unsafe {
        with(|a| a.panel.drag_ended());
        // save the new order first, then end the drag: the others keep the preview till then
        if let Some((h, tb)) = attached() {
            let scale = scale_of(tb);
            dock::drop_reorder(raw(tb), raw(h), MY_CLASS, dock::tray_boundary(raw(tb), scale), gap_px(scale));
        }
        dock::set_dragging(raw(h), false);
    }
    reposition_strip(true);
    unsafe { dock::notify(raw(h)); }
}

/// One frame of the zoom: redraw at the new size, centred under the mouse while dragged, else
/// heading for the (changing) slot.
pub(crate) fn zoom_step() {
    let Some(step) = motion(|m| m.zoom_step(&look(), Instant::now())).flatten() else {
        return timer(TIMER_ZOOM, false);
    };
    redraw_strip();
    if let Some((h, tb)) = attached() {
        let (cx, w) = (unsafe { dock::cursor().x }, width(h));
        match motion(|m| m.drag_x(cx, w, limits(tb, w))).flatten() {
            Some(x) => put(h, tb, x, true),
            None => reposition_strip(true),
        }
    }
    if step.done {
        timer(TIMER_ZOOM, false);
    }
}

// ---------------------------------------------------------------- neighbours

/// A neighbour (tray, another widget) changed: reposition right away.
pub(crate) unsafe extern "system" fn on_win_event(_: isize, _event: u32, hwnd: isize, id_object: i32, _: i32, _: u32, _: u32) {
    let Some(tb) = taskbar() else { return };
    let Some(Some(strip)) = with(|a| a.strip) else { return };
    if dock::is_neighbour_event(hwnd, id_object, raw(tb), raw(strip)) {
        check_fit();
        reposition_strip(false);
    }
}

pub(crate) fn update_hooks() {
    let Some(mut hooks) = with(|a| std::mem::take(&mut a.hooks)) else { return };
    unsafe { hooks.update(taskbar().map_or(0, raw), Some(on_win_event)); }
    with(|a| a.hooks = hooks);
}
