//! How a widget's panel (popup / flyout) opens and closes: one behaviour for every widget.
//!
//! The widget reports what happens to its strip and panel; [`PanelBehaviour`] answers with an
//! [`Action`]. It does no window work itself, so every rule is tested below.
//!
//! The rules:
//! - Rest the mouse on the strip for `hover_open_ms` → the panel opens without taking focus
//!   (typing elsewhere isn't interrupted).
//! - A hover-opened panel closes `hover_close_ms` after the mouse has left both the strip and
//!   the panel (so you can move from one to the other).
//! - Click the strip → the panel opens with focus (a click elsewhere closes it); click again →
//!   it closes. Clicking while it's hover-opened keeps it open and gives it focus instead.
//! - A click that closed the panel by taking focus away doesn't reopen it straight after.
//! - Right-click isn't a hover: it cancels a pending hover-open, closes a hover-opened panel
//!   (unless the right-click was on the panel), no hover-opening while the menu is up, and none
//!   after it either until the mouse has really left the strip (the menu takes the mouse,
//!   which also reports a "leave").
//! - Starting a drag closes a hover-opened panel and stops hover-opening.
//! - Only one panel at a time across widgets (see dock::close_other_panels).
//! - The panel is centred on the widget's centre, `panel_margin` from the taskbar, and kept
//!   inside the monitor's work area (`panel_margin` from its edges): see [`place`].
//!
//! Wiring, in a widget:
//! - strip: first WM_MOUSEMOVE after a leave → `mouse_entered`, then start a 50 ms timer;
//!   WM_MOUSELEAVE → `mouse_left(pointer outside the strip)`;
//!   left click (not a drag) → `click`; before the context menu → `right_click`, after it →
//!   `menu_closed`; drag start → `drag_started`.
//! - 50 ms timer → `tick(now, pointer on strip, pointer on panel)`; stop the timer when
//!   `wants_ticks()` is false.
//! - panel closed for any other reason (lost focus, another widget opened its panel, clicked)
//!   → `closed(lost_focus)`.
//! - Do what each returned [`Action`] says. When opening, fade in with [`Fade`].

use crate::anim;
use windows_sys::Win32::Foundation::HWND;
/// Screen rectangle used by [`position`] (re-exported so widgets on the `windows` crate can use it).
pub use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
use crate::style::Look;
use std::time::{Duration, Instant};

/// What the widget should do with its panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    None,
    /// Show without taking focus (opened by hovering).
    OpenByHover,
    /// Show and take focus (opened by a click), so a click elsewhere closes it.
    OpenByClick,
    /// It's open by hover: keep it open and give it focus (a click on the strip).
    Pin,
    Close,
}

/// A click that closed the panel by taking focus away shouldn't reopen it within this time.
const REOPEN_GUARD: Duration = Duration::from_millis(400);

#[derive(Default, Debug)]
pub struct PanelBehaviour {
    open: bool,
    by_hover: bool,
    hover_since: Option<Instant>,
    outside_since: Option<Instant>,
    menu_open: bool,
    blocked: bool,
    dragging: bool,
    lost_focus_at: Option<Instant>,
}

impl PanelBehaviour {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open because of hovering (rather than a click)?
    pub fn by_hover(&self) -> bool {
        self.open && self.by_hover
    }

    /// Keep calling `tick` (every ~50 ms) while this is true.
    pub fn wants_ticks(&self) -> bool {
        self.hover_since.is_some() || self.by_hover()
    }

    /// The mouse came onto the strip.
    pub fn mouse_entered(&mut self, now: Instant) {
        if !self.blocked && !self.menu_open && !self.dragging && !self.open {
            self.hover_since = Some(now);
        }
    }

    /// The mouse left the strip (WM_MOUSELEAVE). `pointer_outside`: the pointer really is
    /// outside the strip (not just taken by a menu or a mouse capture).
    pub fn mouse_left(&mut self, pointer_outside: bool) {
        if pointer_outside {
            self.blocked = false;
            self.hover_since = None;
        }
    }

    /// Every ~50 ms while `wants_ticks`.
    pub fn tick(&mut self, now: Instant, on_strip: bool, on_panel: bool, look: &Look) -> Action {
        if self.menu_open || self.dragging {
            self.hover_since = None;
            return Action::None;
        }
        if !self.open {
            match self.hover_since {
                Some(t) if on_strip => {
                    if now.duration_since(t) >= Duration::from_millis(look.hover_open_ms as u64) {
                        self.hover_since = None;
                        return self.opened(true);
                    }
                }
                _ => self.hover_since = None,
            }
            return Action::None;
        }
        if self.by_hover {
            if on_strip || on_panel {
                self.outside_since = None;
            } else {
                let t = *self.outside_since.get_or_insert(now);
                if now.duration_since(t) >= Duration::from_millis(look.hover_close_ms as u64) {
                    self.reset_closed();
                    return Action::Close;
                }
            }
        }
        Action::None
    }

    /// Left click on the strip (not the end of a drag).
    pub fn click(&mut self, now: Instant) -> Action {
        if self.open {
            if self.by_hover {
                self.by_hover = false;
                self.outside_since = None;
                return Action::Pin;
            }
            self.reset_closed();
            return Action::Close;
        }
        if self.lost_focus_at.is_some_and(|t| now.duration_since(t) < REOPEN_GUARD) {
            return Action::None; // this click took focus from the panel and so closed it
        }
        self.hover_since = None;
        self.opened(false)
    }

    /// Left click on a button on the strip (e.g. play/pause): it acts on its own and doesn't
    /// open, pin or close the panel. A pending hover-open waits its full delay again, so the
    /// panel doesn't pop up while the buttons are being used.
    pub fn button_click(&mut self, now: Instant) {
        if !self.open && self.hover_since.is_some() {
            self.hover_since = Some(now);
        }
    }

    /// Before showing the context menu. `on_panel`: the right-click was on the panel itself.
    pub fn right_click(&mut self, on_panel: bool) -> Action {
        self.menu_open = true;
        self.blocked = true;
        self.hover_since = None;
        if self.by_hover() && !on_panel {
            self.reset_closed();
            return Action::Close;
        }
        Action::None
    }

    /// The context menu closed. Hover-opening stays blocked until the mouse really leaves.
    pub fn menu_closed(&mut self) {
        self.menu_open = false;
    }

    /// A drag of the strip started.
    pub fn drag_started(&mut self) -> Action {
        self.dragging = true;
        self.hover_since = None;
        if self.by_hover() {
            self.reset_closed();
            return Action::Close;
        }
        Action::None
    }

    pub fn drag_ended(&mut self) {
        self.dragging = false;
    }

    /// The panel was closed by something else: it lost focus (`lost_focus`), another widget
    /// opened its panel, or it was clicked.
    pub fn closed(&mut self, now: Instant, lost_focus: bool) {
        if lost_focus {
            self.lost_focus_at = Some(now);
        }
        self.reset_closed();
    }

    fn opened(&mut self, by_hover: bool) -> Action {
        self.open = true;
        self.by_hover = by_hover;
        self.outside_since = None;
        if by_hover { Action::OpenByHover } else { Action::OpenByClick }
    }

    fn reset_closed(&mut self) {
        self.open = false;
        self.by_hover = false;
        self.outside_since = None;
    }
}

/// Where a `w`×`h` panel goes (screen px) for the widget `anchor`: horizontally centred on the
/// widget's centre; vertically `margin` beyond the taskbar (the work area's edge), or next to
/// the widget when that isn't on a screen edge; always inside `work` by `margin`.
pub fn position(anchor: RECT, w: i32, h: i32, work: RECT, margin: i32) -> (i32, i32) {
    let fit = |pos: i32, size: i32, lo: i32, hi: i32| {
        // inside [lo + margin, hi - margin]; too big → start at the near edge
        pos.min(hi - margin - size).max(lo + margin)
    };
    let cx = (anchor.left + anchor.right) / 2;
    let x = fit(cx - w / 2, w, work.left, work.right);
    let y = if anchor.bottom > work.bottom {
        work.bottom - margin - h // taskbar at the bottom: above it
    } else if anchor.top < work.top {
        work.top + margin // taskbar at the top: below it
    } else if anchor.top - margin - h >= work.top + margin {
        anchor.top - margin - h // floating widget: above it if there's room
    } else {
        anchor.bottom + margin
    };
    (x, fit(y, h, work.top, work.bottom))
}

/// [`position`] for the widget window `anchor`, on the monitor it's on, with the shared
/// `panel_margin` at display scale `scale`.
pub fn place(anchor: HWND, w: i32, h: i32, look: &Look, scale: f32) -> (i32, i32) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(anchor, &mut rc);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(anchor, MONITOR_DEFAULTTONEAREST), &mut mi);
        position(rc, w, h, mi.rcWork, (look.panel_margin * scale).round() as i32)
    }
}

/// The panel's fade-in: call `start` when showing it (opacity 0), then `alpha` every ~10 ms
/// until it returns None.
#[derive(Default, Debug)]
pub struct Fade(Option<Instant>);

impl Fade {
    pub fn start(&mut self, now: Instant) {
        self.0 = Some(now);
    }

    pub fn stop(&mut self) {
        self.0 = None;
    }

    pub fn running(&self) -> bool {
        self.0.is_some()
    }

    /// The opacity (0..255) for now; None once finished (then it's fully opaque).
    pub fn alpha(&mut self, now: Instant, look: &Look) -> Option<u8> {
        let t0 = self.0?;
        let t = anim::progress(now.duration_since(t0).as_secs_f32() * 1000.0, look.fade_ms);
        if t >= 1.0 {
            self.0 = None;
            return Some(255);
        }
        Some((255.0 * anim::ease_out_quad(t)).round() as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;
    use crate::system;

    fn look() -> Look {
        let mut sys = system::read();
        sys.hover_ms = 400;
        sys.animations = true;
        Style::default().resolve(&sys)
    }

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn hover_opens_after_delay_without_focus_and_closes_after_leaving() {
        let (lk, t0) = (look(), Instant::now());
        let mut p = PanelBehaviour::default();
        p.mouse_entered(t0);
        assert!(p.wants_ticks());
        assert_eq!(p.tick(ms(t0, 200), true, false, &lk), Action::None);
        assert_eq!(p.tick(ms(t0, 400), true, false, &lk), Action::OpenByHover);
        // moving from the strip onto the panel keeps it open
        assert_eq!(p.tick(ms(t0, 600), false, true, &lk), Action::None);
        // off both: closes after hover_close_ms (300)
        assert_eq!(p.tick(ms(t0, 700), false, false, &lk), Action::None);
        assert_eq!(p.tick(ms(t0, 900), false, false, &lk), Action::None);
        assert_eq!(p.tick(ms(t0, 1000), false, false, &lk), Action::Close);
        assert!(!p.is_open() && !p.wants_ticks());
    }

    #[test]
    fn leaving_before_the_delay_opens_nothing() {
        let (lk, t0) = (look(), Instant::now());
        let mut p = PanelBehaviour::default();
        p.mouse_entered(t0);
        p.mouse_left(true);
        assert_eq!(p.tick(ms(t0, 500), false, false, &lk), Action::None);
        assert!(!p.wants_ticks());
    }

    #[test]
    fn click_opens_with_focus_and_click_again_closes() {
        let t0 = Instant::now();
        let mut p = PanelBehaviour::default();
        assert_eq!(p.click(t0), Action::OpenByClick);
        assert!(!p.wants_ticks()); // a clicked-open panel isn't closed by mouse movement
        assert_eq!(p.click(ms(t0, 500)), Action::Close);
    }

    #[test]
    fn click_on_a_hover_opened_panel_pins_it() {
        let (lk, t0) = (look(), Instant::now());
        let mut p = PanelBehaviour::default();
        p.mouse_entered(t0);
        assert_eq!(p.tick(ms(t0, 450), true, false, &lk), Action::OpenByHover);
        assert_eq!(p.click(ms(t0, 500)), Action::Pin);
        // pinned: leaving no longer closes it
        assert_eq!(p.tick(ms(t0, 2000), false, false, &lk), Action::None);
        assert!(p.is_open());
    }

    #[test]
    fn a_button_click_neither_opens_nor_closes_and_restarts_the_hover_delay() {
        let (lk, t0) = (look(), Instant::now());
        let mut p = PanelBehaviour::default();
        p.mouse_entered(t0);
        p.button_click(ms(t0, 300));
        assert_eq!(p.tick(ms(t0, 500), true, false, &lk), Action::None); // 200 ms since the click
        assert_eq!(p.tick(ms(t0, 700), true, false, &lk), Action::OpenByHover); // 400 ms since it
        // open (by hover or click): a button click leaves it as it is
        p.button_click(ms(t0, 800));
        assert!(p.is_open() && p.by_hover());
        let mut q = PanelBehaviour::default();
        assert_eq!(q.click(t0), Action::OpenByClick);
        q.button_click(ms(t0, 100));
        assert!(q.is_open() && !q.by_hover());
    }

    #[test]
    fn the_click_that_took_focus_away_does_not_reopen() {
        let t0 = Instant::now();
        let mut p = PanelBehaviour::default();
        assert_eq!(p.click(t0), Action::OpenByClick);
        p.closed(ms(t0, 1000), true); // clicking the strip deactivated the panel first
        assert_eq!(p.click(ms(t0, 1050)), Action::None);
        assert_eq!(p.click(ms(t0, 2000)), Action::OpenByClick);
    }

    #[test]
    fn right_click_is_not_a_hover() {
        let (lk, t0) = (look(), Instant::now());
        let mut p = PanelBehaviour::default();
        p.mouse_entered(t0);
        assert_eq!(p.right_click(false), Action::None);
        // menu open, mouse resting on the strip: nothing opens
        assert_eq!(p.tick(ms(t0, 1000), true, false, &lk), Action::None);
        p.menu_closed();
        // the menu took the mouse: a "leave" with the pointer still on the strip, then it
        // comes "back": still blocked
        p.mouse_left(false);
        p.mouse_entered(ms(t0, 1100));
        assert_eq!(p.tick(ms(t0, 2000), true, false, &lk), Action::None);
        // after a real leave, hover works again
        p.mouse_left(true);
        p.mouse_entered(ms(t0, 3000));
        assert_eq!(p.tick(ms(t0, 3400), true, false, &lk), Action::OpenByHover);
    }

    #[test]
    fn right_click_closes_a_hover_opened_panel_unless_on_the_panel() {
        let (lk, t0) = (look(), Instant::now());
        let mut p = PanelBehaviour::default();
        p.mouse_entered(t0);
        assert_eq!(p.tick(ms(t0, 400), true, false, &lk), Action::OpenByHover);
        assert_eq!(p.right_click(true), Action::None); // right-click on the panel: stays
        p.menu_closed();
        assert_eq!(p.right_click(false), Action::Close); // on the strip: closes
    }

    #[test]
    fn drag_closes_a_hover_opened_panel_and_stops_hovering() {
        let (lk, t0) = (look(), Instant::now());
        let mut p = PanelBehaviour::default();
        p.mouse_entered(t0);
        assert_eq!(p.tick(ms(t0, 400), true, false, &lk), Action::OpenByHover);
        assert_eq!(p.drag_started(), Action::Close);
        p.mouse_entered(ms(t0, 500));
        assert_eq!(p.tick(ms(t0, 1000), true, false, &lk), Action::None);
        p.drag_ended();
    }

    fn r(l: i32, t: i32, rr: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: rr, bottom: b }
    }

    #[test]
    fn panel_is_centred_on_the_widget_and_stays_on_screen() {
        let work = r(0, 0, 1920, 1032); // taskbar 1032..1080
        // in the middle: centred, 12 px above the taskbar
        assert_eq!(position(r(900, 1032, 1000, 1080), 300, 200, work, 12), (800, 820));
        // near the right edge: pushed left to 12 px from it
        assert_eq!(position(r(1850, 1032, 1910, 1080), 300, 200, work, 12), (1608, 820));
        // near the left edge
        assert_eq!(position(r(0, 1032, 40, 1080), 300, 200, work, 12), (12, 820));
        // second monitor to the left (negative coordinates)
        let w2 = r(-1920, 0, 0, 1032);
        assert_eq!(position(r(-60, 1032, -10, 1080), 300, 200, w2, 12), (-312, 820));
        // taskbar at the top
        assert_eq!(position(r(900, 0, 1000, 48), 300, 200, r(0, 48, 1920, 1080), 12), (800, 60));
        // floating widget near the top: below it
        assert_eq!(position(r(900, 100, 1000, 140), 300, 200, work, 12), (800, 152));
    }

    #[test]
    fn fade_runs_to_opaque() {
        let (lk, t0) = (look(), Instant::now());
        let mut f = Fade::default();
        f.start(t0);
        let a = f.alpha(ms(t0, 50), &lk).unwrap();
        assert!(a > 0 && a < 255);
        assert_eq!(f.alpha(ms(t0, 200), &lk), Some(255));
        assert_eq!(f.alpha(ms(t0, 210), &lk), None);
    }
}
