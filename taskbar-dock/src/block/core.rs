//! Block's decisions, without windows: what a press, release, hover or menu row means, which
//! mode and width to draw, and the last steps of drawing (cut-off, highlight). `win.rs` feeds it
//! the events and does the window work; the tests below drive it with a fake Content.

use super::*;
use crate::anim::Feel;
use crate::dock::{self, Fit, FitState};
use crate::panel::{Action, PanelBehaviour};
use crate::strip::{Motion, Release};
use std::time::{Duration, Instant};

#[derive(Default, Debug)]
pub(crate) struct Core {
    pub motion: Motion,
    pub panel: PanelBehaviour,
    pub feel: Feel,
    pub fit: FitState,
    /// The mouse is on the strip (between the first move and WM_MOUSELEAVE).
    pub hover: bool,
    /// What it's over.
    pub hot: Hit,
    /// What the button went down on (until released).
    pub pressed_on: Option<Hit>,
    /// The user's Compact mode setting.
    pub compact: bool,
    /// The user's Show on taskbar setting.
    pub show: bool,
    /// The mode as last drawn: a switch animates the width.
    pub drawn_compact: Option<bool>,
    /// The width the strip was last headed for (unzoomed): with [`Spec::glide_width`] a change
    /// animates the width like a compact switch.
    pub drawn_target: Option<i32>,
    /// The content's width, held steady ([`Spec::glide_width`]).
    pub width: WidthHold,
}

/// Px at 100% a measured width may differ from the held one without counting as a change.
pub(crate) const WIDTH_SLACK: f32 = 4.0;

/// A strip's content width, held steady ([`Spec::glide_width`], plan step 9): a different width
/// is taken only once it has been measured for the grow (wider) or shrink (narrower) time of
/// [`Spec::width_hold`]; a width narrower by no more than the slack never counts (wider always does). The first width, and any width in a new
/// `key` (mode, height, scale), is taken at once.
#[derive(Default, Debug)]
pub(crate) struct WidthHold {
    held: Option<(WidthKey, i32)>,
    /// A different width measured since then: the latest, and since when.
    pending: Option<(i32, Instant)>,
}

/// What a held width belongs to: compact or not, the strip's height, the scale (in 1/1000).
pub(crate) type WidthKey = (bool, i32, i32);

impl WidthHold {
    /// `w` measured now; `(grow, shrink)`: how long a wider / narrower width must hold
    /// ([`Spec::width_hold`]). The width to use, and how long until a pending change is due (look
    /// again then).
    pub fn update(&mut self, key: WidthKey, w: i32, slack: i32, (grow, shrink): (Duration, Duration), now: Instant) -> (i32, Option<Duration>) {
        let Some((_, held)) = self.held.filter(|(k, _)| *k == key) else {
            self.held = Some((key, w));
            self.pending = None;
            return (w, None);
        };
        // a little narrower doesn't count (some room to spare); any wider does (the content
        // would be cut off: a title ending in "…" that fits)
        if w == held || (w < held && held - w <= slack) {
            self.pending = None;
            return (held, None);
        }
        // the same change as before (within the slack) keeps its start
        let since = match self.pending {
            Some((p, t)) if (p - w).abs() <= slack => t,
            _ => now,
        };
        let wait = if w > held { grow } else { shrink };
        let gone = now.saturating_duration_since(since);
        if gone >= wait {
            self.held = Some((key, w));
            self.pending = None;
            return (w, None);
        }
        self.pending = Some((w, since));
        (held, Some(wait - gone))
    }
}

/// What a release means for the window side.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Up {
    Nothing,
    /// The end of a drag: reorder, glide into the slot.
    Drop,
    /// Do this with the panel.
    Panel(Action),
}

impl Core {
    pub fn down(&mut self, at: Hit, cursor_x: i32, strip_x: i32) {
        self.motion.press(cursor_x, strip_x);
        self.pressed_on = Some(at);
    }

    /// The button came up (`click`) or the capture was lost (Esc, a window took the mouse): a
    /// drag ends like a drop either way; only a real release over what was pressed acts.
    pub fn up(&mut self, click: bool, at: Hit, mode: Click, content: &mut dyn Content, ctx: &mut Ctx, now: Instant) -> Up {
        let release = self.motion.release(now);
        let on = self.pressed_on.take();
        match release {
            Release::Drop => Up::Drop,
            Release::Click if click => match (on, at) {
                (Some(Hit::Button(a, _)), Hit::Button(b, _)) if a == b => {
                    self.panel.button_click(now);
                    content.button(a, ctx);
                    Up::Nothing
                }
                (Some(Hit::Cover(_)), Hit::Cover(_)) => {
                    content.cover_click(ctx);
                    Up::Nothing
                }
                // pressed on a button or the cover, released elsewhere: nothing
                (Some(Hit::Button(..) | Hit::Cover(_)), _) => Up::Nothing,
                _ => match mode {
                    Click::TogglePanel => Up::Panel(self.panel.click(now)),
                    Click::Content => {
                        content.click(ctx);
                        Up::Nothing
                    }
                },
            },
            _ => Up::Nothing,
        }
    }

    /// The cover's hover / press feel after any mouse change: it reacts only while the strip is
    /// at rest. True while it animates (run the feel timer).
    pub fn update_feel(&mut self, animations: bool) -> bool {
        let on = self.hover && matches!(self.hot, Hit::Cover(_)) && self.motion.calm();
        let pressed = on && matches!(self.pressed_on, Some(Hit::Cover(_)));
        self.feel.set(on, pressed);
        if !animations {
            self.feel.snap();
        }
        self.feel.animating()
    }

    /// The mode to draw and the width to cut the strip at, for the fit: compact when chosen; when
    /// crowded, a fixed-width strip goes compact, a stretchable one is cut at its fitted width
    /// (compact once that is its compact width `min`).
    pub fn mode(&self, stretch: bool, min: i32) -> (bool, Option<i32>) {
        match self.fit.get() {
            Fit::Shrunk(w) if stretch => (self.compact || w <= min, Some(w)),
            Fit::Shrunk(_) => (true, None),
            _ => (self.compact, None),
        }
    }
}

/// [`Frame::fit_width`] for drawing: a running width animation's width (`resize`, drawn px), else
/// the width the strip is headed for (`target`, unzoomed px, so times the drag `zoom`: crowded, or
/// a content width held wider or narrower) when that isn't the content's own (`natural`).
pub(crate) fn fit_width(resize: Option<i32>, target: i32, natural: i32, zoom: f32) -> Option<i32> {
    resize.or_else(|| (target != natural).then(|| (target as f32 * zoom).round() as i32))
}

/// Whether a new `target` width should animate: a compact switch (`switched`) always; any other
/// change of the target with [`Spec::glide_width`], or while an animation runs (it heads for the
/// new target from where it is). `prev`: the target last drawn (None: a new strip).
pub(crate) fn animate_width(switched: bool, prev: Option<i32>, target: i32, glide_width: bool, running: bool) -> bool {
    switched || (prev.is_some_and(|p| p != target) && (glide_width || running))
}

/// The widths the dock fits by: in the chosen mode, and compact (`full` when it must never go
/// compact for room).
pub(crate) fn size(chosen: i32, compact: i32, spec: &Spec) -> dock::Size {
    let min = if spec.compact_when_crowded { compact.min(chosen) } else { chosen };
    dock::Size { full: chosen, min, stretch: spec.stretch }
}

/// Where a right-click row goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Route {
    Show,
    Compact,
    Startup,
    Settings,
    Quit,
    Content(usize),
}

pub(crate) fn route(id: usize) -> Route {
    match id {
        std_ids::SHOW => Route::Show,
        std_ids::COMPACT => Route::Compact,
        std_ids::STARTUP => Route::Startup,
        std_ids::SETTINGS => Route::Settings,
        std_ids::QUIT => Route::Quit,
        id => Route::Content(id),
    }
}

/// The right-click panel for `content` (`show_on` None: no tray icon, no Show on taskbar row).
pub(crate) fn menu(content: &mut dyn Content, show_on: Option<bool>, compact_on: bool, startup_on: bool) -> Menu {
    let mut m = MenuBuilder::new(show_on, compact_on, startup_on);
    content.menu(&mut m);
    m.finish()
}

/// The strip shows: switched on and something to show. A widget without a tray icon ignores the
/// switch (it has no row for it, and nothing else would bring the strip back).
pub(crate) fn shown(switch: bool, visible: bool, has_tray: bool) -> bool {
    (switch || !has_tray) && visible
}

/// The tray icon shows whenever the strip doesn't (switched off, or nothing to show right now),
/// so the panels and the switch are always reachable.
pub(crate) fn tray_shown(switch: bool, visible: bool, has_tray: bool) -> bool {
    has_tray && !shown(switch, visible, has_tray)
}

/// The highlight box under the strip: a hovered button's box, else the whole strip (inset), or
/// none when the mouse isn't on it or it's being dragged.
pub(crate) fn highlight(hover: bool, dragging: bool, hot: Hit, width: i32, height: i32, inset: i32) -> Option<Box4> {
    match (hover && !dragging, hot) {
        (false, _) => None,
        (true, Hit::Button(_, b)) => Some(b),
        (true, _) => Some((0, inset, width, height - inset)),
    }
}

/// A [`Tint`] resolved for drawing: its colour and opacities at rest and hovered.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Shade {
    pub rgb: u32,
    pub rest: f32,
    pub hover: f32,
}

impl Shade {
    pub(crate) fn of(t: Tint, levels: &crate::theme::TintLevels) -> Shade {
        let (rest, hover) = t.strengths(levels);
        Shade { rgb: t.rgb, rest, hover }
    }
}

/// The tinted boxes under a strip with a brand tint ([`Content::tint`]), in drawing order (each
/// goes under what is there, so the first one shows on top): the hovered button's box (its own
/// tint, else the strip's) at hover strength, then the whole strip (inset) at rest or hover
/// strength. As [`highlight`], no hover while dragged. (box, 0xRRGGBB, opacity); boxes with
/// opacity 0 are left out.
pub(crate) fn tints(strip: Shade, button: Option<Shade>, hover: bool, dragging: bool, hot: Hit, width: i32, height: i32, inset: i32) -> Vec<(Box4, u32, f32)> {
    let hover = hover && !dragging;
    let mut out = Vec::new();
    if let (true, Hit::Button(_, b)) = (hover, hot) {
        let t = button.unwrap_or(strip);
        out.push((b, t.rgb, t.hover));
    }
    out.push(((0, inset, width, height - inset), strip.rgb, if hover { strip.hover } else { strip.rest }));
    out.retain(|t| t.2 > 0.0);
    out
}

/// Where the right-click panel goes: above the strip, or above the pointer `pt` without one.
pub(crate) fn menu_anchor(strip: Option<Box4>, pt: (i32, i32)) -> Box4 {
    strip.unwrap_or((pt.0, pt.1, pt.0 + 1, pt.1 + 1))
}

/// Where a `w`×`h` panel opened without a strip (from the tray icon) goes: centred on the
/// pointer's x, at the bottom of the work area `work`, kept `margin` inside it.
pub(crate) fn panel_at_pointer(pt: (i32, i32), w: i32, h: i32, work: Box4, margin: i32) -> (i32, i32) {
    use crate::panel::{position, RECT};
    let (left, top, right, bottom) = work;
    let at = RECT { left: pt.0, top: bottom, right: pt.0, bottom: bottom + 1 };
    position(at, w, h, RECT { left, top, right, bottom }, margin)
}

/// Where a `w`×`h` panel opened from the tray icon goes ([`tray::place`]). On the taskbar:
/// above it, centred on the icon. In the tray overflow panel `o`: never over it, as Windows'
/// own flyouts: above it, else beside it (left, then right) with the bottoms lined up; it
/// covers the overflow only if nothing else fits. All kept `margin` inside the work area.
pub(crate) fn panel_at_tray(icon: Box4, overflow: Option<Box4>, w: i32, h: i32, work: Box4, margin: i32) -> (i32, i32) {
    use crate::panel::{position, RECT};
    let r = |b: Box4| RECT { left: b.0, top: b.1, right: b.2, bottom: b.3 };
    let Some(o) = overflow else {
        return position(r(icon), w, h, r(work), margin);
    };
    let (wl, wt, wr, wb) = work;
    let fit = |pos: i32, size: i32, lo: i32, hi: i32| pos.min(hi - margin - size).max(lo + margin);
    if o.1 - margin - h >= wt + margin {
        return (fit((o.0 + o.2) / 2 - w / 2, w, wl, wr), o.1 - margin - h);
    }
    let y = fit(o.3 - h, h, wt, wb);
    if o.0 - margin - w >= wl + margin {
        return (o.0 - margin - w, y);
    }
    if o.2 + margin + w <= wr - margin {
        return (o.2 + margin, y);
    }
    position(r(o), w, h, r(work), margin)
}

/// The left `width` px of a `w`×`h` pixel buffer (cut off during a compact resize, or narrowed
/// for room), with the last `fade` px fading out.
pub(crate) fn cut(px: &[u32], w: i32, h: i32, width: i32, fade: i32) -> Vec<u32> {
    let width = width.clamp(1, w);
    let mut out = Vec::with_capacity((width * h) as usize);
    for y in 0..h {
        out.extend_from_slice(&px[(y * w) as usize..(y * w + width) as usize]);
    }
    if width < w {
        // premultiplied: all four channels go down together (0 at the very edge)
        for y in 0..h {
            for i in 0..fade.min(width) {
                let keep = (i as f32 + 0.5) / fade as f32;
                let p = &mut out[(y * width + width - 1 - i) as usize];
                let ch = |s: u32| ((((*p >> s) & 0xff) as f32 * keep).round() as u32) << s;
                *p = ch(24) | ch(16) | ch(8) | ch(0);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;
    use crate::system;

    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        switch_on: bool,
    }

    impl Content for Fake {
        fn measure(&mut self, f: &Frame, compact: bool) -> i32 {
            f.px(if compact { 40.0 } else { 100.0 })
        }
        fn draw(&mut self, _cv: &mut Canvas, _f: &Frame) {}
        fn click(&mut self, ctx: &mut Ctx) {
            self.calls.push("click".into());
            ctx.redraw();
        }
        fn button(&mut self, id: usize, ctx: &mut Ctx) {
            self.calls.push(format!("button {id}"));
            ctx.every(7, 40);
        }
        fn cover_click(&mut self, ctx: &mut Ctx) {
            self.calls.push("cover".into());
            ctx.after(|| {});
        }
        fn menu(&mut self, m: &mut MenuBuilder) {
            m.title("Fake").item(Row::action(1, menu_panel::glyph::REFRESH, "Refresh now")).sep().item(Row::switch(2, "Mine", self.switch_on));
        }
    }

    fn look(animations: bool) -> Look {
        let mut sys = system::read();
        sys.animations = animations;
        sys.hover_ms = 400;
        let mut lk = Style::default().resolve(&sys);
        lk.animations = animations;
        lk
    }

    const BTN: Hit = Hit::Button(3, (50, 4, 74, 28));
    const COVER: Hit = Hit::Cover((2, 2, 30, 30));

    fn release(core: &mut Core, f: &mut Fake, click: bool, at: Hit, mode: Click) -> (Up, Vec<String>) {
        let mut ctx = Ctx::new(0);
        let up = core.up(click, at, mode, f, &mut ctx, Instant::now());
        (up, ctx.reqs.iter().map(|r| format!("{r:?}")).collect())
    }

    #[test]
    fn a_click_on_the_strip_toggles_the_panel() {
        let (mut c, mut f) = (Core::default(), Fake::default());
        c.down(Hit::Strip, 100, 500);
        assert_eq!(release(&mut c, &mut f, true, Hit::Strip, Click::TogglePanel).0, Up::Panel(Action::OpenByClick));
        assert!(f.calls.is_empty());
        c.down(Hit::Strip, 100, 500);
        assert_eq!(release(&mut c, &mut f, true, Hit::Strip, Click::TogglePanel).0, Up::Panel(Action::Close));
    }

    #[test]
    fn click_mode_content_goes_to_the_content_with_its_requests() {
        let (mut c, mut f) = (Core::default(), Fake::default());
        c.down(Hit::Strip, 100, 500);
        let (up, reqs) = release(&mut c, &mut f, true, Hit::Strip, Click::Content);
        assert_eq!((up, f.calls.clone(), reqs), (Up::Nothing, vec!["click".to_string()], vec!["Redraw".to_string()]));
    }

    #[test]
    fn a_button_acts_only_when_released_over_it_and_never_toggles_the_panel() {
        let (mut c, mut f) = (Core::default(), Fake::default());
        c.down(BTN, 100, 500);
        let (up, reqs) = release(&mut c, &mut f, true, BTN, Click::TogglePanel);
        assert_eq!((up, reqs), (Up::Nothing, vec!["Every(7, 40)".to_string()]));
        assert_eq!(f.calls, ["button 3"]);
        assert!(!c.panel.is_open());
        c.down(BTN, 100, 500);
        assert_eq!(release(&mut c, &mut f, true, Hit::Strip, Click::TogglePanel).0, Up::Nothing);
        assert_eq!(f.calls.len(), 1, "released off the button: nothing");
        c.down(Hit::Strip, 100, 500);
        release(&mut c, &mut f, true, BTN, Click::TogglePanel);
        assert_eq!(f.calls.len(), 1, "pressed elsewhere, released on it: not a button press");
    }

    #[test]
    fn the_cover_opens_the_app_on_release_over_it() {
        let (mut c, mut f) = (Core::default(), Fake::default());
        c.down(COVER, 10, 500);
        let (_, reqs) = release(&mut c, &mut f, true, COVER, Click::TogglePanel);
        assert_eq!((f.calls.clone(), reqs), (vec!["cover".to_string()], vec!["After(..)".to_string()]));
        assert!(!c.panel.is_open());
    }

    #[test]
    fn a_drag_is_a_drop_never_a_click_and_lost_capture_only_drops() {
        let (lk, t0) = (look(true), Instant::now());
        let (mut c, mut f) = (Core::default(), Fake::default());
        c.down(BTN, 100, 500);
        c.motion.mouse_move(102, 80, crate::strip::Limits::NONE, &lk, t0);
        assert!(!c.motion.dragging(), "2 px is still a click");
        c.motion.mouse_move(110, 80, crate::strip::Limits::NONE, &lk, t0);
        assert!(c.motion.dragging());
        assert_eq!(release(&mut c, &mut f, true, BTN, Click::TogglePanel).0, Up::Drop);
        assert!(f.calls.is_empty());
        // capture lost without a drag: nothing at all
        c.down(Hit::Strip, 100, 500);
        assert_eq!(release(&mut c, &mut f, false, Hit::Strip, Click::TogglePanel).0, Up::Nothing);
        assert!(!c.panel.is_open() && !c.motion.pressed());
    }

    #[test]
    fn the_cover_feels_the_mouse_only_at_rest() {
        let lk = look(true);
        let mut c = Core { hover: true, hot: COVER, ..Default::default() };
        assert!(c.update_feel(true), "hovered: grows");
        c.feel.snap();
        assert!((c.feel.value() - crate::anim::HOVER_GROW).abs() < 1e-6);
        c.down(COVER, 10, 500);
        assert!(c.update_feel(true), "pressed: dips");
        c.feel.snap();
        assert!((c.feel.value() - crate::anim::PRESS_SHRINK).abs() < 1e-6);
        // dragged from the cover: back to normal, the drag zoom has it
        c.motion.mouse_move(40, 80, crate::strip::Limits::NONE, &lk, Instant::now());
        c.update_feel(false);
        assert_eq!(c.feel.value(), 1.0);
        // on the strip, not the cover: normal
        let mut c = Core { hover: true, hot: Hit::Strip, ..Default::default() };
        assert!(!c.update_feel(true));
    }

    #[test]
    fn mode_and_cut_for_the_fit() {
        let mut c = Core::default();
        assert_eq!(c.mode(false, 40), (false, None));
        c.fit.update(Fit::Shrunk(40));
        assert_eq!(c.mode(false, 40), (true, None), "fixed width: compact for room");
        c.fit.update(Fit::Shrunk(70));
        assert_eq!(c.mode(true, 40), (false, Some(70)), "stretchable: cut, still full");
        c.fit.update(Fit::Shrunk(40));
        assert_eq!(c.mode(true, 40), (true, Some(40)));
        c.compact = true;
        c.fit.update(Fit::Full);
        assert_eq!(c.mode(true, 40), (true, None));
    }

    #[test]
    fn size_for_the_dock() {
        let mut spec = Spec::new("C", "s", "t", |_| {});
        assert_eq!(size(100, 40, &spec), dock::Size { full: 100, min: 40, stretch: false });
        spec.compact_when_crowded = false;
        spec.stretch = true;
        assert_eq!(size(100, 40, &spec), dock::Size { full: 100, min: 100, stretch: true });
        spec.compact_when_crowded = true;
        assert_eq!(size(40, 40, &spec).min, 40, "already compact");
    }

    #[test]
    fn menu_gets_the_standard_rows() {
        let mut f = Fake { switch_on: true, ..Default::default() };
        let m = menu(&mut f, None, true, false);
        assert_eq!(m.title, "Fake");
        let ids: Vec<String> = m.items.iter().map(|i| match i {
            Item::Row(r) => format!("{}{}", r.id, r.switch.map_or("", |on| if on { "+" } else { "-" })),
            Item::Pair(a, b) => format!("{}|{}", a.id, b.id),
            Item::Sep => "sep".into(),
        }).collect();
        let (c, s, e, q) = (std_ids::COMPACT, std_ids::STARTUP, std_ids::SETTINGS, std_ids::QUIT);
        // ended on its own switch: the standard switches join that group
        assert_eq!(ids, ["1", "sep", "2+", &format!("{c}+"), &format!("{s}-"), "sep", &format!("{e}|{q}")]);
        assert_eq!(route(c), Route::Compact);
        assert_eq!(route(q), Route::Quit);
        assert_eq!(route(2), Route::Content(2));
    }

    fn ids(m: &Menu) -> Vec<String> {
        m.items.iter().map(|i| match i {
            Item::Row(r) => format!("{}{}", r.id, r.switch.map_or("", |on| if on { "+" } else { "-" })),
            Item::Pair(a, b) => format!("{}|{}", a.id, b.id),
            Item::Sep => "sep".into(),
        }).collect()
    }

    #[test]
    fn with_a_tray_icon_show_on_taskbar_leads_the_standard_switches() {
        let (sh, c, s, e, q) = (std_ids::SHOW, std_ids::COMPACT, std_ids::STARTUP, std_ids::SETTINGS, std_ids::QUIT);
        let mut f = Fake { switch_on: true, ..Default::default() };
        let m = menu(&mut f, Some(false), true, false);
        assert_eq!(ids(&m), ["1", "sep", "2+", &format!("{sh}-"), &format!("{c}+"), &format!("{s}-"), "sep", &format!("{e}|{q}")]);
        assert_eq!(route(sh), Route::Show);
        // placed by the widget: stays there, not added twice
        struct Placed;
        impl Content for Placed {
            fn measure(&mut self, _: &Frame, _: bool) -> i32 { 1 }
            fn draw(&mut self, _: &mut Canvas, _: &Frame) {}
            fn menu(&mut self, m: &mut MenuBuilder) {
                m.item(Row::action(1, menu_panel::glyph::REFRESH, "Refresh")).sep().show().item(Row::switch(2, "Mine", false));
            }
        }
        let m = menu(&mut Placed, Some(true), false, false);
        assert_eq!(ids(&m), ["1", "sep", &format!("{sh}+"), "2-", &format!("{c}-"), &format!("{s}-"), "sep", &format!("{e}|{q}")]);
        // no tray icon: show() places nothing
        assert!(!ids(&menu(&mut Placed, None, false, false)).iter().any(|i| i.starts_with(&sh.to_string())));
    }

    #[test]
    fn the_strip_shows_when_switched_on_and_there_is_something_to_show() {
        // (switch, visible, has_tray) -> (strip, tray icon)
        let cases = [
            ((true, true, true), (true, false)),
            ((false, true, true), (false, true)),  // switched off: tray icon
            ((true, false, true), (false, true)),  // idle (no call): tray icon stays
            ((false, false, true), (false, true)),
            ((false, true, false), (true, false)), // no tray icon: the switch can't hide it
            ((true, false, false), (false, false)),
        ];
        for ((sw, vis, tray), want) in cases {
            assert_eq!((shown(sw, vis, tray), tray_shown(sw, vis, tray)), want, "switch {sw}, visible {vis}, tray {tray}");
        }
    }

    #[test]
    fn menu_rows_placed_by_the_widget_stay_put() {
        struct Placed;
        impl Content for Placed {
            fn measure(&mut self, _: &Frame, _: bool) -> i32 { 1 }
            fn draw(&mut self, _: &mut Canvas, _: &Frame) {}
            fn menu(&mut self, m: &mut MenuBuilder) {
                m.compact().item(Row::switch(5, "Show when idle", false)).startup().sep().item(Row::action(6, menu_panel::glyph::OPEN, "Sign in"));
            }
        }
        let m = menu(&mut Placed, None, false, true);
        assert!(matches!(&m.items[0], Item::Row(r) if r.id == std_ids::COMPACT));
        assert!(matches!(&m.items[2], Item::Row(r) if r.id == std_ids::STARTUP && r.switch == Some(true)));
        assert_eq!(m.items.iter().filter(|i| matches!(i, Item::Row(r) if r.id == std_ids::COMPACT)).count(), 1);
        assert!(matches!(m.items.last(), Some(Item::Pair(..))));
        // only actions: the switches get their own group
        struct Actions;
        impl Content for Actions {
            fn measure(&mut self, _: &Frame, _: bool) -> i32 { 1 }
            fn draw(&mut self, _: &mut Canvas, _: &Frame) {}
            fn menu(&mut self, m: &mut MenuBuilder) {
                m.item(Row::action(1, menu_panel::glyph::REFRESH, "Refresh"));
            }
        }
        let m = menu(&mut Actions, None, false, false);
        assert_eq!(m.items[1], Item::Sep);
        // nothing of its own: no leading separator
        struct Empty;
        impl Content for Empty {
            fn measure(&mut self, _: &Frame, _: bool) -> i32 { 1 }
            fn draw(&mut self, _: &mut Canvas, _: &Frame) {}
        }
        assert!(matches!(&menu(&mut Empty, None, false, false).items[0], Item::Row(r) if r.id == std_ids::COMPACT));
    }

    #[test]
    fn highlight_box() {
        assert_eq!(highlight(false, false, Hit::Strip, 100, 48, 4), None);
        assert_eq!(highlight(true, false, Hit::Strip, 100, 48, 4), Some((0, 4, 100, 44)));
        assert_eq!(highlight(true, false, COVER, 100, 48, 4), Some((0, 4, 100, 44)));
        assert_eq!(highlight(true, false, BTN, 100, 48, 4), Some((50, 4, 74, 28)), "a button: its own box");
        assert_eq!(highlight(true, true, Hit::Strip, 100, 48, 4), None, "not while dragged");
    }

    #[test]
    fn tinted_boxes() {
        let blurple = Shade { rgb: 0x5865F2, rest: 0.3, hover: 0.5 };
        let red = Shade { rgb: 0xED4245, rest: 0.0, hover: 0.6 };
        let whole = (0, 4, 100, 44);
        // at rest: the whole strip, rest strength
        assert_eq!(tints(blurple, None, false, false, Hit::Strip, 100, 48, 4), vec![(whole, 0x5865F2, 0.3)]);
        // hovered: stronger
        assert_eq!(tints(blurple, None, true, false, COVER, 100, 48, 4), vec![(whole, 0x5865F2, 0.5)]);
        // a hovered button: its box first (on top), in its own tint
        assert_eq!(tints(blurple, Some(red), true, false, BTN, 100, 48, 4), vec![((50, 4, 74, 28), 0xED4245, 0.6), (whole, 0x5865F2, 0.5)]);
        // ... or the strip's when it has none
        assert_eq!(tints(blurple, None, true, false, BTN, 100, 48, 4)[0], ((50, 4, 74, 28), 0x5865F2, 0.5));
        // dragged: rest strength, no button box
        assert_eq!(tints(blurple, Some(red), true, true, BTN, 100, 48, 4), vec![(whole, 0x5865F2, 0.3)]);
        // a tint only on hover leaves nothing at rest
        assert!(tints(red, None, false, false, Hit::Strip, 100, 48, 4).is_empty());
    }

    #[test]
    fn tint_strength_comes_from_the_levels_by_state() {
        use crate::theme::TintLevels;
        let l = TintLevels { quiet: 0.2, active: 0.4, hover: 0.6 };
        let at = |t: Tint| Shade::of(t, &l);
        assert_eq!(at(Tint::new(0x123456, false)), Shade { rgb: 0x123456, rest: 0.2, hover: 0.6 }, "paused / muted");
        assert_eq!(at(Tint::new(0x123456, true)), Shade { rgb: 0x123456, rest: 0.4, hover: 0.6 }, "playing / unmuted");
        // fading between them, and the widget's own multiplier (media's tint_strength)
        let half = at(Tint { rgb: 1, active: 0.5, strength: 2.0 });
        assert!((half.rest - 0.6).abs() < 1e-6 && (half.hover - 1.0).abs() < 1e-6, "{half:?}");
        assert_eq!(at(Tint { rgb: 1, active: 1.0, strength: 0.0 }), Shade { rgb: 1, rest: 0.0, hover: 0.0 });
        // tint_fixed: one strength for every state, 0 = none, 1 = solid
        for v in [0.0, 1.0] {
            let f = TintLevels { quiet: v, active: v, hover: v };
            assert_eq!(Shade::of(Tint::new(1, false), &f), Shade { rgb: 1, rest: v, hover: v });
            assert_eq!(Shade::of(Tint::new(1, true), &f), Shade { rgb: 1, rest: v, hover: v });
        }
        // no tint at all leaves no boxes (Block's base highlight only)
        assert!(tints(Shade::of(Tint::new(1, true), &TintLevels { quiet: 0.0, active: 0.0, hover: 0.0 }), None, true, false, Hit::Strip, 100, 48, 4).is_empty());
    }

    #[test]
    fn cut_keeps_the_left_part_and_fades_the_end() {
        let (w, h) = (10, 2);
        let px = vec![0xFF_FF_FF_FFu32; (w * h) as usize];
        let out = cut(&px, w, h, 6, 3);
        assert_eq!(out.len(), 12);
        assert_eq!(out[0], 0xFFFF_FFFF);
        assert!(out[5] >> 24 < 0xFF, "the last column fades");
        assert_eq!(cut(&px, w, h, 10, 3), px, "not cut: unchanged");
        assert_eq!(cut(&px, w, h, 99, 3).len(), 20);
    }

    #[test]
    fn the_widgets_own_type_is_found_behind_dyn_content() {
        // what `block::with` does
        let mut boxed: Box<dyn Content> = Box::new(Fake { switch_on: true, ..Default::default() });
        let any = <dyn Content as AsAny>::as_any(boxed.as_mut());
        assert!(any.downcast_mut::<Fake>().is_some_and(|f| f.switch_on));
        struct Other;
        assert!(<dyn Content as AsAny>::as_any(boxed.as_mut()).downcast_mut::<Other>().is_none());
    }

    #[test]
    fn a_compact_switch_lays_out_to_its_width() {
        assert_eq!(fit_width(None, 258, 258, 1.0), None, "at rest, room: the natural width");
        assert_eq!(fit_width(Some(150), 120, 258, 1.0), Some(150), "a switch running: its width");
        assert_eq!(fit_width(None, 200, 258, 1.1), Some(220), "crowded, while dragged: zoomed");
        assert_eq!(fit_width(None, 230, 180, 1.0), Some(230), "held wider than the content");
    }

    #[test]
    fn which_width_changes_animate() {
        assert!(animate_width(true, Some(258), 120, false, false), "a compact switch, any widget");
        assert!(!animate_width(false, None, 258, true, false), "a new strip: straight in");
        assert!(!animate_width(false, Some(258), 258, true, false), "no change");
        assert!(animate_width(false, Some(258), 200, true, false), "content width, gliding widget");
        assert!(!animate_width(false, Some(80), 90, false, false), "bus's countdown grows: jumps");
        assert!(animate_width(false, Some(258), 200, false, true), "mid-switch: heads for the new one");
    }

    #[test]
    fn a_content_width_is_held_steady() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let k = (false, 48, 1000);
        const GROW_AFTER: Duration = Duration::from_millis(400);
        let hold = (GROW_AFTER, Duration::from_millis(2000));
        let mut h = WidthHold::default();
        assert_eq!(h.update(k, 200, 4, hold, at(0)), (200, None), "the first width: at once");
        assert_eq!(h.update(k, 197, 4, hold, at(10)), (200, None), "a little narrower: no change");
        // wider: after GROW_AFTER
        assert_eq!(h.update(k, 240, 4, hold, at(100)), (200, Some(GROW_AFTER)));
        assert_eq!(h.update(k, 241, 4, hold, at(300)), (200, Some(GROW_AFTER - Duration::from_millis(200))), "the same change: its start kept");
        assert_eq!(h.update(k, 241, 4, hold, at(500)), (241, None));
        // narrower: only after 2 s, and a flicker back starts it again
        assert_eq!(h.update(k, 150, 4, hold, at(1000)).0, 241);
        assert_eq!(h.update(k, 241, 4, hold, at(1500)), (241, None), "back: nothing pending");
        assert_eq!(h.update(k, 150, 4, hold, at(1600)).0, 241);
        assert_eq!(h.update(k, 150, 4, hold, at(3500)).0, 241, "1.9 s: still held");
        assert_eq!(h.update(k, 150, 4, hold, at(3600)), (150, None));
        // a different measurement while pending starts the wait again
        assert_eq!(h.update(k, 220, 4, hold, at(4000)).0, 150);
        assert_eq!(h.update(k, 260, 4, hold, at(4300)), (150, Some(GROW_AFTER)));
        // a new mode, height or scale: at once
        assert_eq!(h.update((true, 48, 1000), 90, 4, hold, at(4400)), (90, None));
        assert_eq!(h.update((false, 48, 1250), 300, 4, hold, at(4500)), (300, None));
        // no hold (the default): at once both ways
        let none = (Duration::ZERO, Duration::ZERO);
        assert_eq!(h.update((false, 48, 1250), 120, 4, none, at(4600)), (120, None));
        assert_eq!(h.update((false, 48, 1250), 123, 4, none, at(4600)), (123, None), "a little wider: taken, else the text is cut off");
        assert_eq!(h.update((false, 48, 1250), 120, 4, none, at(4600)), (123, None), "a little narrower: room to spare");
        assert_eq!(h.update((false, 48, 1250), 250, 4, none, at(4601)), (250, None));
    }

    #[test]
    fn requests_are_recorded_not_done() {
        let mut ctx = Ctx::new(42);
        ctx.redraw();
        ctx.redraw_panel();
        ctx.redraw_tray();
        ctx.every(1, 1000);
        ctx.stop(1);
        ctx.open_panel();
        ctx.toggle_panel();
        ctx.close_panel();
        ctx.open_menu();
        ctx.quit();
        assert_eq!(ctx.main_window(), 42);
        let s: Vec<String> = ctx.reqs.iter().map(|r| format!("{r:?}")).collect();
        assert_eq!(s, ["Redraw", "RedrawPanel", "RedrawTray", "Every(1, 1000)", "Stop(1)", "OpenPanel", "TogglePanel", "ClosePanel", "Menu", "Quit"]);
    }

    #[test]
    fn the_menu_goes_above_the_strip_or_else_the_pointer() {
        assert_eq!(menu_anchor(Some((10, 20, 70, 68)), (500, 900)), (10, 20, 70, 68));
        assert_eq!(menu_anchor(None, (500, 900)), (500, 900, 501, 901), "no strip (tray icon): the pointer");
    }

    #[test]
    fn a_tray_opened_panel_goes_above_the_pointer_at_the_work_areas_bottom() {
        let work = (0, 0, 1920, 1032);
        // measured with the old flyout.rs: pointer x 864 → (684, 526) for 360×494, margin 12
        assert_eq!(panel_at_pointer((864, 761), 360, 494, work, 12), (684, 526));
        // the pointer's y doesn't matter (the tray is on the taskbar, below the work area)
        assert_eq!(panel_at_pointer((864, 1060), 360, 494, work, 12), (684, 526));
        // kept inside the work area
        assert_eq!(panel_at_pointer((1910, 1060), 360, 494, work, 12), (1920 - 12 - 360, 526));
        assert_eq!(panel_at_pointer((5, 1060), 360, 494, work, 12), (12, 526));
    }

    #[test]
    fn a_panel_from_the_tray_never_covers_the_overflow_panel() {
        let work = (0, 0, 1920, 1032);
        let overlaps = |(x, y): (i32, i32), o: Box4| x < o.2 && x + 360 > o.0 && y < o.3 && y + 494 > o.1;
        // icon on the taskbar: above it, centred on the icon
        assert_eq!(panel_at_tray((1600, 1040, 1624, 1072), None, 360, 494, work, 12), (1432, 526));
        // in a small overflow panel above the chevron: above that
        let o = (1500, 860, 1700, 1020);
        let at = panel_at_tray((1520, 880, 1544, 904), Some(o), 360, 494, work, 12);
        assert_eq!(at, (1420, 860 - 12 - 494));
        assert!(!overlaps(at, o));
        // a tall overflow (no room above): left of it, bottoms lined up
        let o = (1500, 400, 1700, 1020);
        let at = panel_at_tray((1520, 420, 1544, 444), Some(o), 360, 494, work, 12);
        assert_eq!(at, (1500 - 12 - 360, 1020 - 494));
        assert!(!overlaps(at, o));
        // ... and right of it when it's at the screen's left edge
        let o = (10, 400, 210, 1020);
        let at = panel_at_tray((20, 420, 44, 444), Some(o), 360, 494, work, 12);
        assert_eq!(at, (222, 526));
        assert!(!overlaps(at, o));
    }

    #[test]
    fn a_tray_click_toggles_the_panel_like_a_strip_click() {
        // what Request::TogglePanel does: the strip-click decision, with or without a strip
        let mut core = Core::default();
        let now = Instant::now();
        assert_eq!(core.panel.click(now), Action::OpenByClick);
        assert_eq!(core.panel.click(now), Action::Close);
    }
}
