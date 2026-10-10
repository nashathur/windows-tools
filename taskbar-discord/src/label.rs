//! The taskbar strip: when it shows, drawing, dock position, gliding, dragging and zoom (rules
//! in taskbar_dock::strip), WinEvent hooks. It always docks on the right, in the stack against
//! the tray shared with the other widgets.

use super::*;
use taskbar_dock::strip::{DragMove, Glide, Release};

pub(crate) const MY_CLASS: &str = "TaskbarDiscordStrip";

pub(crate) fn taskbar() -> Option<HWND> {
    unsafe { FindWindowW(w!("Shell_TrayWnd"), None).ok().filter(|h| !h.is_invalid()) }
}

pub(crate) fn raw(h: HWND) -> isize {
    h.0 as isize
}

/// The strip shows during a call, when something needs the user (setup, sign-in, an error),
/// and with show_when_idle while connected.
fn wanted(a: &App) -> bool {
    a.voice.in_call() || a.voice.status.needs_user() || (a.cfg.show_when_idle && a.voice.status == Status::Ready)
}

/// Creates or removes the strip to match `wanted`.
pub(crate) fn update_visibility() {
    let Some((want, existing)) = with(|a| (wanted(a), a.label)) else { return };
    match (want, existing) {
        (true, None) => create_label(),
        (false, Some(_)) => {
            popup_closed(false);
            taskbar_dock::menu_panel::close();
            destroy_label();
        }
        _ => {}
    }
}

fn create_label() {
    let Some(tb) = taskbar() else { return };
    let hwnd = unsafe {
        CreateWindowExW(WS_EX_LAYERED | WS_EX_NOACTIVATE, LABEL_CLASS, w!("Discord voice"),
            WS_CHILD | WS_CLIPSIBLINGS, 0, 0, 1, 1, tb, None, hinstance(), None)
    };
    if let Ok(hwnd) = hwnd {
        with(|a| {
            a.label = Some(hwnd);
            a.drawn_compact = None;
            a.fit = dock::FitState::default();
        });
        unsafe { dock::mark_member(raw(hwnd)); }
        redraw_label(); // shows it straight in its slot
    }
}

/// The media strip's text column, px at 100% (its `max_width`): our strip is as wide as its own.
pub(crate) fn media_text_width() -> f32 {
    taskbar_dock::settings::section("media")
        .and_then(|m| m.get("max_width").and_then(|v| v.as_f64()))
        .filter(|w| *w > 0.0)
        .map_or(136.0, |w| w as f32)
}

/// Runs the ring's timer exactly while it pulses.
fn set_beating(on: bool) {
    let Some((main, was)) = with(|a| (a.main, std::mem::replace(&mut a.beating, on))) else { return };
    if on != was {
        unsafe {
            if on {
                SetTimer(main, TIMER_BEAT, 40, None);
            } else {
                let _ = KillTimer(main, TIMER_BEAT);
            }
        }
    }
}

/// The mouse state changed: the logo grows while hovered, dips while pressed (anim::Feel), and
/// the frame timer runs while it moves. Only while the strip is at rest: dragged from the logo,
/// it eases back to normal and leaves the size to the drag zoom. Cheap when nothing changed.
pub(crate) fn feel_update() {
    let Some((main, changed, run, was)) = with(|a| {
        let on = a.on_logo && a.motion.calm();
        let changed = a.feel.set(on, on && a.pressed == Some(Press::Logo));
        if !a.look.animations {
            a.feel.snap();
        }
        (a.main, changed, a.feel.animating(), std::mem::replace(&mut a.feeling, a.feel.animating()))
    }) else { return };
    if run && !was {
        unsafe { SetTimer(main, TIMER_FEEL, taskbar_dock::anim::FEEL_FRAME_MS, None); }
    }
    if changed {
        redraw_label();
    }
}

pub(crate) fn feel_step() {
    let Some((main, more)) = with(|a| (a.main, a.feel.tick(Instant::now()))) else { return };
    if !more {
        with(|a| a.feeling = false);
        unsafe { let _ = KillTimer(main, TIMER_FEEL); }
    }
    redraw_label();
}

pub(crate) fn destroy_label() {
    set_beating(false);
    if let Some(Some(h)) = with(|a| a.label.take()) {
        unsafe {
            dock::leave(raw(h));
            let _ = ShowWindow(h, SW_HIDE);
            dock::notify(raw(h)); // the others close the gap
            let _ = DestroyWindow(h);
        }
    }
}

/// Explorer restarted: keeps the strip if it is already on the new taskbar, otherwise replaces
/// it. Forgetting a live strip left an untracked duplicate on the taskbar (seen in weather,
/// 2026-10-10).
pub(crate) fn taskbar_restarted() {
    if let Some(Some(h)) = with(|a| a.label) {
        if unsafe { IsWindow(h).as_bool() && GetParent(h).ok() == taskbar() } {
            return;
        }
        destroy_label();
    }
    update_visibility();
}

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

pub(crate) fn scale_of(tb: HWND) -> f32 {
    system::scale_for(raw(tb))
}

pub(crate) fn gap_px(scale: f32) -> i32 {
    (look().gap * scale).round() as i32
}

/// Screen x of the taskbar's left edge (the strip is positioned in taskbar client coordinates).
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

// ---------------------------------------------------------------- mouse on the strip

/// How far (taskbar client x) a strip `width` wide may go while dragged: against the tray, and
/// not over the taskbar's own buttons.
fn limits(hwnd: HWND, width: i32) -> strip::Limits {
    let Ok(tb) = (unsafe { GetParent(hwnd) }) else { return strip::Limits::NONE };
    let (scale, tl) = (scale_of(tb), tb_left(tb));
    let min = dock::icons_right(raw(tb)).map(|r| r + gap_px(scale) - tl);
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
                panel_do(a); // a hover-opened panel closes
            }
            timer(TIMER_ZOOM, true);
            unsafe { dock::notify(raw(hwnd)); }
            drag_move(hwnd, x);
        }
        Some(DragMove::Moved(x)) => drag_move(hwnd, x),
        _ => {}
    }
}

/// The strip's button under client point (x, y), inside its highlight box (None: the strip itself).
pub(crate) fn button_at(x: i32, y: i32) -> Option<Button> {
    with(|a| a.strip.and_then(|l| l.hit(x, y))).flatten()
}

/// What a mouse press on the strip landed on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Press {
    /// A strip button.
    Button(Button),
    /// The server logo with its ring: a click opens Discord's window.
    Logo,
    /// The rest: a click toggles the panel.
    Strip,
}

/// What is under client point (x, y).
pub(crate) fn press_at(x: i32, y: i32) -> Press {
    if let Some(b) = button_at(x, y) {
        return Press::Button(b);
    }
    if with(|a| a.strip.is_some_and(|l| l.on_logo(x, y))).unwrap_or(false) {
        return Press::Logo;
    }
    Press::Strip
}

/// Button released (`click`: at client x, y), or the capture lost: a drag ends like a drop either
/// way. A click on one of the strip's buttons acts when released on the same button and leaves
/// the panel be; elsewhere it toggles the panel.
pub(crate) fn mouse_up(hwnd: HWND, click: Option<(i32, i32)>) {
    let release = motion(|m| m.release(Instant::now())).unwrap_or(Release::None);
    let pressed = with(|a| a.pressed.take()).flatten();
    feel_update();
    if click.is_some() {
        unsafe { let _ = ReleaseCapture(); }
    }
    match (release, click, pressed) {
        (Release::Drop, _, _) => drag_ended(hwnd),
        (_, Some((x, y)), Some(Press::Button(b))) => {
            if button_at(x, y) == Some(b) {
                press(b);
            }
            with(|a| a.panel.button_click(Instant::now()));
        }
        // the logo opens Discord (when released on it) and leaves the panel be
        (_, Some((x, y)), Some(Press::Logo)) => {
            if press_at(x, y) == Press::Logo {
                crate::open::open_discord();
            }
            with(|a| a.panel.button_click(Instant::now()));
        }
        (_, Some(_), _) => toggle_popup(),
        _ => {}
    }
}

/// Mute, Deafen or Leave, from the strip or the panel.
pub(crate) fn press(b: Button) {
    let Some((mute, deaf)) = with(|a| (a.voice.self_mute, a.voice.self_deaf)) else { return };
    match b {
        Button::Mute => rpc(rpc::Cmd::Mute(!mute)),
        Button::Deafen => rpc(rpc::Cmd::Deaf(!deaf)),
        Button::Leave => rpc(rpc::Cmd::Leave),
    }
}

/// One frame of the zoom; while dragged keep the strip centred under the mouse.
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
    timer(TIMER_ZOOM, true);
    unsafe {
        with(|a| a.panel.drag_ended());
        // save the new order first, then end the drag: the others keep the preview till then
        if let Ok(tb) = GetParent(hwnd) {
            let scale = scale_of(tb);
            dock::drop_reorder(raw(tb), raw(hwnd), MY_CLASS, dock::tray_boundary(raw(tb), scale), gap_px(scale));
        }
        dock::set_dragging(raw(hwnd), false);
    }
    reposition_label_with(true);
    unsafe { dock::notify(raw(hwnd)); }
}

// ---------------------------------------------------------------- drawing and position

fn slot(tb: HWND, me: HWND, size: dock::Size, scale: f32) -> Option<dock::Slot> {
    unsafe { dock::slot(raw(tb), raw(me), size, dock::tray_boundary(raw(tb), scale), gap_px(scale)) }
}

/// Compact mode switched since the last drawing: animate the width. Returns the width to draw
/// at while that runs.
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

pub(crate) fn resize_step() {
    let lk = look();
    let step = motion(|m| m.resize_step(&lk, Instant::now())).flatten();
    if step.map_or(true, |s| s.done) {
        timer(TIMER_RESIZE, false);
    }
    redraw_label();
}

/// The room next to the taskbar's buttons changed (dock::fit): hide or show the strip.
fn apply_fit(hwnd: HWND, fit: dock::Fit, redraw: bool) -> dock::FitChange {
    let Some(change) = with(|a| a.fit.update(fit)) else { return dock::FitChange::None };
    unsafe {
        match change {
            dock::FitChange::Hide => {
                popup_closed(false);
                taskbar_dock::menu_panel::close();
                dock::set_squeezed(raw(hwnd), true);
                let _ = ShowWindow(hwnd, SW_HIDE);
                dock::notify(raw(hwnd));
                log("no room next to the taskbar's buttons: strip hidden");
            }
            dock::FitChange::Show => {
                dock::set_squeezed(raw(hwnd), false);
                log("room again next to the taskbar's buttons: strip shown");
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
    let Some(Some((hwnd, voice, hover, hot, cfg, zoom, main, grow))) =
        with(|a| a.label.map(|h| (h, a.voice.clone(), a.hover, a.hot, a.cfg.clone(), a.motion.zoom(), a.main.0 as usize, a.feel.value())))
    else { return };
    let lk = look();
    // the ring: pulses (from dim) while we're unmuted, steady while muted, red while deafened
    let pulsing = voice.in_call() && !voice.self_mute && !voice.self_deaf && lk.animations;
    let ring = if voice.self_deaf {
        render::Ring::Red
    } else if voice.self_mute {
        render::Ring::Dim
    } else if !pulsing {
        render::Ring::Pulse(0.6)
    } else {
        let t = with(|a| *a.ring_since.get_or_insert_with(Instant::now)).unwrap_or_else(Instant::now);
        render::Ring::Pulse(taskbar_dock::anim::pulse(t.elapsed().as_secs_f32()))
    };
    if !pulsing {
        with(|a| a.ring_since = None);
    }
    set_beating(pulsing);
    let logo = voice.channel.as_ref().and_then(|c| {
        let (id, url) = (c.guild_id.as_ref()?, c.guild_icon.clone()?);
        let stem: String = url.rsplit('/').next().unwrap_or("").chars().take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_').collect();
        avatars::get_url(format!("guild_{id}_{stem}"), url, move || notify_avatar(main))
    });
    let avatars: Vec<_> = voice
        .channel
        .iter()
        .flat_map(|c| c.members.iter().take(cfg.max_avatars.max(1)))
        .map(|m| avatars::get(m, move || notify_avatar(main)))
        .collect();
    let text_w = with(|a| a.media_w).unwrap_or(136.0);
    let content = render::StripContent { voice: &voice, avatars, max_avatars: cfg.max_avatars, logo, ring, text_w };
    unsafe {
        let Ok(tb) = GetParent(hwnd) else { return };
        let mut rc = RECT::default();
        let _ = GetClientRect(tb, &mut rc);
        let scale = scale_of(tb);
        let height = rc.bottom - rc.top;
        if height <= 0 {
            return;
        }
        dock::mark_member(raw(hwnd));
        let measure = |compact: bool| render::update_strip(hwnd, &content, compact, height, scale, &lk, hover, None, None, true, 1.0).size.cx;
        let size = dock::Size { full: measure(cfg.compact), min: measure(true), stretch: false };
        with(|a| a.size = Some(size));
        if zoom == 1.0 {
            dock::publish_widths(raw(hwnd), size);
        }
        let slot = slot(tb, hwnd, size, scale);
        if let Some(f) = slot.map(|s| s.fit).filter(|_| !dragging()) {
            apply_fit(hwnd, f, false);
        }
        if with(|a| a.fit.hidden()) == Some(true) {
            return;
        }
        let compact = cfg.compact || with(|a| a.fit.compact()) == Some(true);
        let resize_w = start_resize(hwnd, compact, || measure(compact));
        // while animating, the full content is drawn cut off
        let draw_compact = compact && resize_w.is_none();
        let before = dock::rect(raw(hwnd));
        let lay = render::update_strip(hwnd, &content, draw_compact, height, scale * zoom, &lk, hover, hot, resize_w, false, grow);
        with(|a| a.strip = Some(lay));
        let drawn = lay.size;
        let tl = tb_left(tb);
        if !IsWindowVisible(hwnd).as_bool() {
            // appearing: straight into place
            let x = slot.map_or(0, |s| s.x_for(drawn.cx) - tl);
            motion(|m| m.stop_glide());
            let _ = SetWindowPos(hwnd, HWND_TOP, x, 0, drawn.cx, drawn.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW | SET_WINDOW_POS_FLAGS(dock::raise_flag(raw(hwnd))));
            dock::notify(raw(hwnd));
            return;
        }
        let _ = SetWindowPos(hwnd, HWND_TOP, 0, 0, drawn.cx, drawn.cy, SWP_NOMOVE | SWP_NOACTIVATE | SET_WINDOW_POS_FLAGS(dock::raise_flag(raw(hwnd))));
        if motion(|m| m.settled()) == Some(true) {
            // docked against the tray: a new width keeps the right edge
            strip::keep_right_edge(raw(hwnd), raw(tb), before);
        }
        if !dragging() {
            let x = slot.map_or(label_client_x(hwnd, tb), |s| s.x_for(drawn.cx) - tl);
            move_label_to(hwnd, tb, x, member_dragged(tb, hwnd));
        }
    }
}

pub(crate) fn dragging() -> bool {
    motion(|m| m.dragging()) == Some(true)
}

/// Is another dock member being dragged? Then every move is a slide.
fn member_dragged(tb: HWND, me: HWND) -> bool {
    unsafe { dock::children(raw(tb)).iter().any(|c| c.member && c.dragging && c.hwnd != raw(me)) }
}

fn label_client_x(hwnd: HWND, tb: HWND) -> i32 {
    unsafe { dock::rect(raw(hwnd)).left - tb_left(tb) }
}

fn move_label_to(hwnd: HWND, tb: HWND, x: i32, always_slide: bool) {
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

pub(crate) fn reposition_label() {
    reposition_label_with(false);
}

fn reposition_label_with(always_slide: bool) {
    if dragging() {
        return;
    }
    let Some(Some((hwnd, size))) = with(|a| a.label.map(|h| (h, a.size))) else { return };
    unsafe {
        let Ok(tb) = GetParent(hwnd) else { return };
        let mut rc = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rc);
        let (width, scale) = (rc.right - rc.left, scale_of(tb));
        let slot = slot(tb, hwnd, size.unwrap_or(dock::Size::fixed(width)), scale);
        if let Some(s) = slot {
            if apply_fit(hwnd, s.fit, true) != dock::FitChange::None {
                return;
            }
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return;
        }
        if let Some(s) = slot {
            move_label_to(hwnd, tb, s.x_for(width) - tb_left(tb), always_slide || member_dragged(tb, hwnd));
        }
    }
}

/// While dragging: put the strip at x (taskbar client coordinates), never past the tray.
fn drag_move(hwnd: HWND, x: i32) {
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

/// A neighbour (tray, another widget) changed: reposition right away.
pub(crate) unsafe extern "system" fn on_win_event(_: isize, _event: u32, hwnd: isize, id_object: i32, _: i32, _: u32, _: u32) {
    let Some(tb) = taskbar() else { return };
    let Some(Some(label)) = with(|a| a.label) else { return };
    if dock::is_neighbour_event(hwnd, id_object, raw(tb), raw(label)) {
        reposition_label();
    }
}

pub(crate) fn update_hooks() {
    let Some(mut hooks) = with(|a| std::mem::take(&mut a.hooks)) else { return };
    unsafe { hooks.update(taskbar().map_or(0, raw), Some(on_win_event)); }
    with(|a| a.hooks = hooks);
}

/// Once a second: keeps the strip alive (Explorer restarts), follows the shared style live, and
/// re-checks the fit (a hidden strip gets no events).
pub(crate) fn watch_label() {
    let media_w = media_text_width();
    if with(|a| std::mem::replace(&mut a.media_w, media_w)) != Some(media_w) {
        redraw_label();
    }
    if refresh_look() {
        if let Some(Some(p)) = with(|a| a.popup) {
            unsafe { let _ = InvalidateRect(p, None, false); }
        }
    }
    if let Some(Some(h)) = with(|a| a.label) {
        let parent_ok = unsafe { IsWindow(h).as_bool() && GetParent(h).ok() == taskbar() };
        if !parent_ok {
            with(|a| a.label = None);
        }
    }
    update_visibility();
    redraw_label();
    update_hooks();
}
