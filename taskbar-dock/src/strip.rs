//! How a widget's strip moves in the dock: gliding to its slot, being dragged to a new place,
//! and the zoom that grows its content while it's picked up. One behaviour for every widget.
//!
//! [`Motion`] holds the state and answers with positions; it does no window work itself (like
//! [`crate::panel::PanelBehaviour`]), so the widget moves its window outside any borrow of its
//! own state, and every rule is tested below. [`move_strip`] does the move itself.
//!
//! The rules:
//! - Moving to a new slot slides when moving right (a gap opened up); moving left it jumps
//!   (making room: sliding would briefly overlap the neighbour), unless `always_slide` (after a
//!   drop, or while another widget is dragged, so the live preview flows). While a neighbour
//!   changes width (its compact switch), it follows that neighbour frame by frame instead
//!   ([`Motion::follow`]: jumps either way).
//! - Slides and the compact switch take `glide_ms` and ease like the taskbar's own buttons
//!   ([`anim::ease_dock`]).
//! - A press becomes a drag once the mouse has moved more than [`DRAG_THRESHOLD`] px; a release
//!   without that is a click.
//! - Picked up, the content grows to `drag_zoom`; while it grows the strip stays centred where
//!   the unzoomed one would be under the mouse. Dropped, it shrinks back while gliding into place.
//! - A slide never fights the mouse: dragging cancels it.
//! - The stack hangs from the tray, so a strip that changes width (compact mode, longer text)
//!   keeps its right edge and grows or shrinks to the left ([`keep_right_edge`]).
//! - Switching between full and compact mode animates the width ([`Motion::resize`]): the
//!   widget draws its full content at [`Motion::resize_width`] (cut off at the right) until
//!   the animation ends, then the new mode.
//! - While dragged the strip stays within its [`Limits`]: against the tray on the right, the
//!   taskbar's own buttons on the left.
//!
//! All x values are screen coordinates of the strip's left edge.
//!
//! Wiring, in a widget (two timers on any of its windows, ~10 ms):
//! - WM_LBUTTONDOWN → `press(cursor x, strip x)` and SetCapture.
//! - WM_MOUSEMOVE → `mouse_move(cursor x, strip width, max x)` (max x: against the tray): on
//!   [`DragMove::Started`] mark the drag for the dock (`dock::set_dragging`, `notify`), close a
//!   hover-opened panel, start the zoom timer; on `Started`/`Moved` move the strip to the x it
//!   carries, then `dock::track_drag` (the sticky drop slot, for the others' preview).
//! - WM_LBUTTONUP / capture lost → `release()`: [`Release::Click`] → the click action;
//!   [`Release::Drop`] → `drop_reorder`, then `dock::set_dragging(false)`, start the zoom timer,
//!   `glide_to` the new slot, `notify`.
//! - A new slot (neighbour moved, regular tick) → `glide_to`, then do the [`Glide`].
//! - Glide timer → `glide_step`; zoom timer → `zoom_step`, redraw at `zoom()`, then
//!   `drag_x` (follow the mouse) or `glide_to` the slot (shrinking after a drop).
//! - Compact mode switched → `resize(old width, new width)`; resize timer → `resize_step`,
//!   redraw at `resize_width()`. After every redraw that changed the width, `keep_right_edge`
//!   (when `settled()`).

use crate::anim;
use crate::style::Look;
use std::time::Instant;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetParent, GetSystemMetrics, GetWindowLongW, GetWindowRect, SetWindowPos, GWL_STYLE, HWND_TOP, HWND_TOPMOST,
    SM_CYSCREEN, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, WS_CHILD,
};

/// Px the mouse must move with the button down before a press counts as a drag.
pub const DRAG_THRESHOLD: i32 = 3;

/// The client (x, y) in a mouse message's `lparam` (signed: a captured mouse can be left of or
/// above the window).
pub fn client_pos(lparam: isize) -> (i32, i32) {
    ((lparam & 0xFFFF) as i16 as i32, ((lparam >> 16) & 0xFFFF) as i16 as i32)
}

#[cfg(test)]
mod pos_tests {
    #[test]
    fn client_pos_is_signed() {
        assert_eq!(super::client_pos(0x0018_0064), (100, 24));
        assert_eq!(super::client_pos(0xFFF6_FFECu32 as i32 as isize), (-20, -10));
    }
}

#[derive(Clone, Copy, Debug)]
struct Press {
    cursor_x: i32,
    strip_x: i32,
    moved: bool,
    /// Strip width when the drag started (before zooming).
    base_w: i32,
}

#[derive(Clone, Copy, Debug)]
struct Tween<T> {
    from: T,
    to: T,
    t0: Instant,
}

#[derive(Debug)]
pub struct Motion {
    glide: Option<Tween<i32>>,
    zoom: f32,
    zoom_anim: Option<Tween<f32>>,
    press: Option<Press>,
    /// Collapse/expand: the width animation, and the width to draw at right now.
    resize: Option<Tween<i32>>,
    resize_w: Option<i32>,
}

impl Default for Motion {
    fn default() -> Self {
        Motion { glide: None, zoom: 1.0, zoom_anim: None, press: None, resize: None, resize_w: None }
    }
}

/// How far a dragged strip may go (screen x of its left edge): `max` against the tray, `min`
/// against the taskbar's own buttons. With no room between them, the tray side wins.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Limits {
    pub min: Option<i32>,
    pub max: Option<i32>,
}

impl Limits {
    pub const NONE: Limits = Limits { min: None, max: None };

    pub fn up_to(max: i32) -> Limits {
        Limits { min: None, max: Some(max) }
    }
}

/// What to do after [`Motion::glide_to`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Glide {
    /// Already there, already heading there, or being dragged.
    Stay,
    /// Move there right away (no glide timer needed).
    Jump(i32),
    /// Start the glide timer; [`Motion::glide_step`] gives the positions.
    Slide,
}

/// What a mouse move means while the button is down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DragMove {
    /// Not pressed, or not moved far enough yet.
    None,
    /// Just became a drag: move the strip here, start the zoom timer, tell the dock.
    Started(i32),
    /// Still dragging: move the strip here.
    Moved(i32),
}

/// What a release means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Release {
    /// Wasn't pressed.
    None,
    /// Pressed and released without dragging.
    Click,
    /// The end of a drag: start the zoom timer (shrinking back) and glide into the new slot.
    Drop,
}

/// One frame of a running animation.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Step<T> {
    pub value: T,
    /// The last frame: stop the timer.
    pub done: bool,
}

impl Motion {
    /// Content size: 1.0, bigger while dragged.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// The mouse button is down on the strip (dragging or not yet).
    pub fn pressed(&self) -> bool {
        self.press.is_some()
    }

    /// Being dragged (the mouse moved past the threshold).
    pub fn dragging(&self) -> bool {
        self.press.is_some_and(|p| p.moved)
    }

    pub fn gliding(&self) -> bool {
        self.glide.is_some()
    }

    /// Pressed, dragged or gliding: leave the position alone (e.g. in a periodic check).
    pub fn busy(&self) -> bool {
        self.press.is_some() || self.glide.is_some()
    }

    /// Not dragged, gliding or zoomed: the strip is at rest, so its cover may react to the
    /// mouse (`anim::Feel`). A press that hasn't moved yet still counts as at rest.
    pub fn calm(&self) -> bool {
        !self.dragging() && self.glide.is_none() && self.zoom_anim.is_none() && self.zoom == 1.0
    }

    /// Not pressed, gliding or zooming: a width change should keep the right edge
    /// ([`keep_right_edge`]). (While dragged the mouse has it; a glide or the zoom after a drop
    /// is already heading for the slot.)
    pub fn settled(&self) -> bool {
        !self.busy() && self.zoom_anim.is_none()
    }

    pub fn press(&mut self, cursor_x: i32, strip_x: i32) {
        self.press = Some(Press { cursor_x, strip_x, moved: false, base_w: 0 });
    }

    /// `limits`: how far the strip may go (see [`Limits`]).
    pub fn mouse_move(&mut self, cursor_x: i32, width: i32, limits: Limits, look: &Look, now: Instant) -> DragMove {
        let Some(mut p) = self.press else { return DragMove::None };
        if p.moved {
            return DragMove::Moved(self.drag_x(cursor_x, width, limits).unwrap_or(p.strip_x));
        }
        if (cursor_x - p.cursor_x).abs() <= DRAG_THRESHOLD {
            return DragMove::None;
        }
        p.moved = true;
        p.base_w = width;
        self.press = Some(p);
        self.glide = None; // a slide never fights the mouse
        self.zoom_to(look.drag_zoom, now); // picked up: content grows a little
        DragMove::Started(self.drag_x(cursor_x, width, limits).unwrap_or(p.strip_x))
    }

    pub fn release(&mut self, now: Instant) -> Release {
        match self.press.take() {
            None => Release::None,
            Some(p) if !p.moved => Release::Click,
            Some(_) => {
                self.zoom_to(1.0, now); // put down: back to normal size while it glides into place
                Release::Drop
            }
        }
    }

    /// Where the strip goes while dragged: following the mouse, its centre staying where the
    /// unzoomed strip's centre would be, within `limits`. None when not dragging.
    ///
    /// Held against a limit while the mouse goes further, the grab point moves along with the
    /// mouse, so the strip follows straight away when the mouse turns back (no "debt" to work
    /// off first).
    pub fn drag_x(&mut self, cursor_x: i32, width: i32, limits: Limits) -> Option<i32> {
        let mut p = self.press.filter(|p| p.moved)?;
        let grown = if p.base_w > 0 { width - p.base_w } else { 0 };
        let x = p.strip_x + cursor_x - p.cursor_x - grown / 2;
        let max = limits.max.unwrap_or(i32::MAX);
        let min = limits.min.unwrap_or(i32::MIN).min(max);
        let held = x.clamp(min, max);
        if held != x {
            p.strip_x += held - x;
            self.press = Some(p);
        }
        Some(held)
    }

    /// Start animating the strip's width from `from` to `to` (compact mode switched). True:
    /// start the resize timer. False: no animation (same width, or animations off): just redraw.
    /// Switched again halfway, pass the current [`resize_width`](Self::resize_width) as `from`.
    pub fn resize(&mut self, from: i32, to: i32, look: &Look, now: Instant) -> bool {
        if from == to || from <= 0 || to <= 0 || !look.animations {
            self.resize = None;
            self.resize_w = None;
            return false;
        }
        self.resize = Some(Tween { from, to, t0: now });
        self.resize_w = Some(from);
        true
    }

    /// While a resize runs: the width to draw the strip at (the full content, cut off there).
    pub fn resize_width(&self) -> Option<i32> {
        self.resize_w
    }

    /// One frame of the resize: redraw at [`resize_width`](Self::resize_width) (None again on
    /// the last frame: then draw the new mode). None: nothing running, stop the timer.
    pub fn resize_step(&mut self, look: &Look, now: Instant) -> Option<Step<i32>> {
        let r = self.resize?;
        let t = anim::progress(now.duration_since(r.t0).as_secs_f32() * 1000.0, look.glide_ms);
        let value = r.from + ((r.to - r.from) as f32 * anim::ease_dock(t)).round() as i32;
        let done = t >= 1.0;
        self.resize_w = if done { None } else { Some(value) };
        if done {
            self.resize = None;
        }
        Some(Step { value, done })
    }

    /// Head for `x` from `current`. See the rules at the top.
    pub fn glide_to(&mut self, current: i32, x: i32, always_slide: bool, look: &Look, now: Instant) -> Glide {
        if self.dragging() {
            return Glide::Stay; // the mouse has it
        }
        if self.glide.map(|g| g.to) == Some(x) {
            return Glide::Stay; // already heading there
        }
        if x == current || (x < current && !always_slide) || !look.animations {
            self.glide = None;
            return if x == current { Glide::Stay } else { Glide::Jump(x) };
        }
        self.glide = Some(Tween { from: current, to: x, t0: now });
        Glide::Slide
    }

    /// Follow a neighbour that's changing width, frame by frame ([`crate::dock::Slot::follow`]):
    /// straight to `x` either way (a slide would trail behind it: a gap or an overlap), ending any
    /// slide. Stays put while dragged.
    pub fn follow(&mut self, current: i32, x: i32) -> Glide {
        if self.dragging() {
            return Glide::Stay;
        }
        self.glide = None;
        if x == current { Glide::Stay } else { Glide::Jump(x) }
    }

    /// Stop a slide where it is (e.g. the window went away).
    pub fn stop_glide(&mut self) {
        self.glide = None;
    }

    /// One frame of a slide: where the strip is now. None: nothing to do, stop the timer.
    pub fn glide_step(&mut self, look: &Look, now: Instant) -> Option<Step<i32>> {
        if self.dragging() {
            self.glide = None;
        }
        let g = self.glide?;
        let t = anim::progress(now.duration_since(g.t0).as_secs_f32() * 1000.0, look.glide_ms);
        let value = g.from + ((g.to - g.from) as f32 * anim::ease_dock(t)).round() as i32;
        let done = t >= 1.0;
        if done {
            self.glide = None;
        }
        Some(Step { value, done })
    }

    /// Grow or shrink the content smoothly to `target`. True: start the zoom timer.
    pub fn zoom_to(&mut self, target: f32, now: Instant) -> bool {
        if self.zoom_anim.map(|z| z.to) == Some(target) || (self.zoom_anim.is_none() && self.zoom == target) {
            return false;
        }
        self.zoom_anim = Some(Tween { from: self.zoom, to: target, t0: now });
        true
    }

    /// One frame of the zoom: the new [`zoom`](Self::zoom), redraw at it. None: stop the timer.
    pub fn zoom_step(&mut self, look: &Look, now: Instant) -> Option<Step<f32>> {
        let z = self.zoom_anim?;
        let t = anim::progress(now.duration_since(z.t0).as_secs_f32() * 1000.0, look.zoom_ms);
        self.zoom = z.from + (z.to - z.from) * anim::ease_out_cubic(t);
        let done = t >= 1.0;
        if done {
            self.zoom = z.to;
            self.zoom_anim = None;
        }
        Some(Step { value: self.zoom, done })
    }
}

/// After a redraw changed the strip's width: moves it so its right edge stays where it was in
/// `before` (its window rectangle before the redraw). `UpdateLayeredWindow` without a position
/// keeps the left edge, which made a strip switched to compact mode shrink away from the tray
/// and then slide back. Call only while [`Motion::settled`], and only for a docked strip.
///
/// # Safety
/// Win32 calls on `h` and `tb`.
pub unsafe fn keep_right_edge(h: HWND, tb: HWND, before: RECT) {
    let mut now: RECT = std::mem::zeroed();
    GetWindowRect(h, &mut now);
    let (w0, w1) = (before.right - before.left, now.right - now.left);
    if w0 > 0 && w1 > 0 && w0 != w1 && now.left == before.left {
        move_strip(h, tb, before.right - w1, false);
    }
}

/// For a docked strip about to be drawn `width` px wide: the position that keeps its right edge
/// where it is now (`before`, its window rectangle), for `UpdateLayeredWindow`'s `pptDst`, so the
/// size and the position change in one step. (Without it the strip first grows to the right, over
/// its neighbour, until [`keep_right_edge`] moves it back ~2 ms later; seen every frame of a
/// compact switch, 2026-10-11.) None: the width doesn't change, or it isn't a taskbar child.
/// Call only while [`Motion::settled`].
///
/// # Safety
/// Win32 calls on `h`.
pub unsafe fn right_anchored(h: HWND, before: RECT, width: i32) -> Option<POINT> {
    if before.right - before.left == width || width <= 0 || GetWindowLongW(h, GWL_STYLE) as u32 & WS_CHILD == 0 {
        return None;
    }
    // child window: coordinates relative to the parent (the taskbar)
    let mut pt = POINT { x: before.right - width, y: before.top };
    ScreenToClient(GetParent(h), &mut pt);
    Some(pt)
}

/// Puts the strip `h` at screen x, vertically centred on the taskbar (a taskbar child) or on the
/// taskbar `tb` (a floating window; above the screen bottom when there's no taskbar).
/// `raise` re-asserts its z-order: on top of the taskbar's own content when attached, above
/// other windows when floating. Never call it while holding a borrow of the widget's state:
/// moving a window sends it messages.
///
/// # Safety
/// Win32 calls on `h` and `tb`.
pub unsafe fn move_strip(h: HWND, tb: HWND, screen_x: i32, raise: bool) {
    let mut rc: RECT = std::mem::zeroed();
    GetWindowRect(h, &mut rc);
    let height = rc.bottom - rc.top;
    let z = if raise { 0 } else { SWP_NOZORDER };
    if GetWindowLongW(h, GWL_STYLE) as u32 & WS_CHILD != 0 {
        // child window: coordinates are relative to the parent (the taskbar)
        let parent = GetParent(h);
        let mut pr: RECT = std::mem::zeroed();
        GetWindowRect(parent, &mut pr);
        let mut pt = POINT { x: screen_x, y: pr.top + (pr.bottom - pr.top - height) / 2 };
        ScreenToClient(parent, &mut pt);
        // raised only when the taskbar's content is above it (see dock::raise_flag)
        let z = if raise { crate::dock::raise_flag(h) } else { z };
        SetWindowPos(h, HWND_TOP, pt.x, pt.y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | z);
    } else {
        let y = if tb != 0 {
            let mut t: RECT = std::mem::zeroed();
            GetWindowRect(tb, &mut t);
            t.top + (t.bottom - t.top - height) / 2
        } else {
            GetSystemMetrics(SM_CYSCREEN) - height - 4
        };
        SetWindowPos(h, HWND_TOPMOST, screen_x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | z);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;
    use crate::system;
    use std::time::Duration;

    fn look(animations: bool) -> Look {
        let mut sys = system::read();
        sys.animations = animations;
        let mut lk = Style::default().resolve(&sys);
        lk.animations = animations;
        lk
    }

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn slides_right_jumps_left_unless_always_slide() {
        let (lk, t0) = (look(true), Instant::now());
        let mut m = Motion::default();
        assert_eq!(m.glide_to(100, 100, false, &lk, t0), Glide::Stay);
        assert_eq!(m.glide_to(100, 50, false, &lk, t0), Glide::Jump(50));
        assert_eq!(m.glide_to(100, 50, true, &lk, t0), Glide::Slide);
        let mut m = Motion::default();
        assert_eq!(m.glide_to(100, 160, false, &lk, t0), Glide::Slide);
        assert_eq!(m.glide_to(100, 160, false, &lk, t0), Glide::Stay, "already heading there");
        let mid = m.glide_step(&lk, ms(t0, 100)).unwrap();
        assert!(mid.value > 100 && mid.value < 160 && !mid.done);
        let end = m.glide_step(&lk, ms(t0, 1000)).unwrap();
        assert_eq!(end, Step { value: 160, done: true });
        assert!(m.glide_step(&lk, ms(t0, 1010)).is_none() && !m.gliding());
    }

    #[test]
    fn no_slides_without_animations() {
        let (lk, t0) = (look(false), Instant::now());
        let mut m = Motion::default();
        assert_eq!(m.glide_to(100, 160, true, &lk, t0), Glide::Jump(160));
    }

    #[test]
    fn small_moves_are_a_click_larger_ones_a_drag() {
        let (lk, t0) = (look(true), Instant::now());
        let mut m = Motion::default();
        m.press(500, 400);
        assert_eq!(m.mouse_move(502, 80, Limits::NONE, &lk, t0), DragMove::None);
        assert_eq!(m.release(t0), Release::Click);
        assert_eq!(m.release(t0), Release::None);

        m.press(500, 400);
        assert_eq!(m.mouse_move(510, 80, Limits::NONE, &lk, t0), DragMove::Started(410));
        assert!(m.dragging());
        assert_eq!(m.mouse_move(520, 80, Limits::NONE, &lk, t0), DragMove::Moved(420));
        assert_eq!(m.release(t0), Release::Drop);
        assert!(!m.pressed());
    }

    #[test]
    fn zoom_grows_while_dragged_and_stays_centred() {
        let (lk, t0) = (look(true), Instant::now());
        let mut m = Motion::default();
        m.press(500, 400);
        m.mouse_move(510, 80, Limits::NONE, &lk, t0);
        let s = m.zoom_step(&lk, ms(t0, 1000)).unwrap();
        assert!(s.done && (m.zoom() - lk.drag_zoom).abs() < 1e-6);
        // the strip grew 12 px: it moves 6 px left so its centre stays put
        assert_eq!(m.drag_x(510, 92, Limits::NONE), Some(404));
        m.release(ms(t0, 1000));
        let s = m.zoom_step(&lk, ms(t0, 3000)).unwrap();
        assert!(s.done && m.zoom() == 1.0);
        assert!(!m.zoom_to(1.0, t0), "already there");
    }

    #[test]
    fn held_against_the_tray_it_follows_back_at_once() {
        let (lk, t0) = (look(true), Instant::now());
        let mut m = Motion::default();
        m.press(500, 400);
        // limit 450: the strip stops there while the mouse goes on
        assert_eq!(m.mouse_move(510, 80, Limits::up_to(450), &lk, t0), DragMove::Started(410));
        assert_eq!(m.mouse_move(600, 80, Limits::up_to(450), &lk, t0), DragMove::Moved(450));
        assert_eq!(m.mouse_move(700, 80, Limits::up_to(450), &lk, t0), DragMove::Moved(450));
        // turning back 10 px moves it 10 px, not only once the mouse is back where it stopped
        assert_eq!(m.mouse_move(690, 80, Limits::up_to(450), &lk, t0), DragMove::Moved(440));
    }

    #[test]
    fn held_against_the_taskbar_buttons_it_follows_back_at_once() {
        let (lk, t0) = (look(true), Instant::now());
        let mut m = Motion::default();
        m.press(500, 400);
        let lim = Limits { min: Some(380), max: Some(450) };
        assert_eq!(m.mouse_move(490, 80, lim, &lk, t0), DragMove::Started(390));
        assert_eq!(m.mouse_move(400, 80, lim, &lk, t0), DragMove::Moved(380));
        assert_eq!(m.mouse_move(410, 80, lim, &lk, t0), DragMove::Moved(390));
        // no room between the limits: the tray side wins
        let tight = Limits { min: Some(470), max: Some(450) };
        assert_eq!(m.drag_x(410, 80, tight), Some(450));
    }

    #[test]
    fn compact_switch_animates_the_width() {
        let (lk, t0) = (look(true), Instant::now());
        let mut m = Motion::default();
        assert!(m.resize(300, 120, &lk, t0));
        assert_eq!(m.resize_width(), Some(300), "draws at the old width until the first frame");
        let mid = m.resize_step(&lk, ms(t0, 100)).unwrap();
        assert!(mid.value < 300 && mid.value > 120 && !mid.done);
        assert_eq!(m.resize_width(), Some(mid.value));
        let end = m.resize_step(&lk, ms(t0, 1000)).unwrap();
        assert_eq!(end, Step { value: 120, done: true });
        assert_eq!(m.resize_width(), None, "the last frame draws the new mode");
        assert!(m.resize_step(&lk, ms(t0, 1010)).is_none());
        assert!(!m.resize(120, 120, &lk, t0));
        assert!(!Motion::default().resize(120, 300, &look(false), t0), "instant without animations");
    }

    #[test]
    fn glide_and_compact_switch_ease_like_the_taskbar() {
        let (lk, t0) = (look(true), Instant::now());
        // a 44 px glide and a 44 px switch move together, along the measured curve
        let mut g = Motion::default();
        assert_eq!(g.glide_to(100, 144, false, &lk, t0), Glide::Slide);
        let mut r = Motion::default();
        assert!(r.resize(144, 100, &lk, t0));
        for t in [20, 80, 150, 250] {
            let x = g.glide_step(&lk, ms(t0, t)).unwrap().value;
            let w = r.resize_step(&lk, ms(t0, t)).unwrap().value;
            let want = (44.0 * anim::ease_dock(t as f32 / lk.glide_ms)).round() as i32;
            assert_eq!((x - 100, 144 - w), (want, want), "{t} ms");
        }
    }

    #[test]
    fn following_a_resizing_neighbour_jumps_both_ways() {
        let (lk, t0) = (look(true), Instant::now());
        let mut m = Motion::default();
        assert_eq!(m.glide_to(100, 160, false, &lk, t0), Glide::Slide);
        assert_eq!(m.follow(100, 104), Glide::Jump(104), "right: no slide, and the slide ends");
        assert!(!m.gliding());
        assert_eq!(m.follow(104, 98), Glide::Jump(98));
        assert_eq!(m.follow(98, 98), Glide::Stay);
        m.press(500, 98);
        m.mouse_move(520, 80, Limits::NONE, &lk, t0);
        assert_eq!(m.follow(98, 120), Glide::Stay, "the mouse has it");
    }

    #[test]
    fn dragging_cancels_a_slide_and_blocks_new_ones() {
        let (lk, t0) = (look(true), Instant::now());
        let mut m = Motion::default();
        assert_eq!(m.glide_to(100, 160, false, &lk, t0), Glide::Slide);
        m.press(500, 120);
        m.mouse_move(520, 80, Limits::NONE, &lk, t0);
        assert!(!m.gliding());
        assert_eq!(m.glide_to(120, 300, true, &lk, t0), Glide::Stay);
        assert!(m.glide_step(&lk, t0).is_none());
    }
}
