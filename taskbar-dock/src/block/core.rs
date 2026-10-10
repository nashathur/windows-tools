//! Block's decisions, without windows: what a press, release, hover or menu row means, which
//! mode and width to draw, and the last steps of drawing (cut-off, highlight). `win.rs` feeds it
//! the events and does the window work; the tests below drive it with a fake Content.

use super::*;
use crate::anim::Feel;
use crate::dock::{self, Fit, FitState};
use crate::panel::{Action, PanelBehaviour};
use crate::strip::{Motion, Release};
use std::time::Instant;

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
    /// The mode as last drawn: a switch animates the width.
    pub drawn_compact: Option<bool>,
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

/// The widths the dock fits by: in the chosen mode, and compact (`full` when it must never go
/// compact for room).
pub(crate) fn size(chosen: i32, compact: i32, spec: &Spec) -> dock::Size {
    let min = if spec.compact_when_crowded { compact.min(chosen) } else { chosen };
    dock::Size { full: chosen, min, stretch: spec.stretch }
}

/// Where a right-click row goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Route {
    Compact,
    Startup,
    Settings,
    Quit,
    Content(usize),
}

pub(crate) fn route(id: usize) -> Route {
    match id {
        std_ids::COMPACT => Route::Compact,
        std_ids::STARTUP => Route::Startup,
        std_ids::SETTINGS => Route::Settings,
        std_ids::QUIT => Route::Quit,
        id => Route::Content(id),
    }
}

/// The right-click panel for `content`.
pub(crate) fn menu(content: &mut dyn Content, compact_on: bool, startup_on: bool) -> Menu {
    let mut m = MenuBuilder::new(compact_on, startup_on);
    content.menu(&mut m);
    m.finish()
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

/// Where the right-click panel goes: above the strip, or above the pointer `pt` without one.
pub(crate) fn menu_anchor(strip: Option<Box4>, pt: (i32, i32)) -> Box4 {
    strip.unwrap_or((pt.0, pt.1, pt.0 + 1, pt.1 + 1))
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
        let m = menu(&mut f, true, false);
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
        let m = menu(&mut Placed, false, true);
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
        let m = menu(&mut Actions, false, false);
        assert_eq!(m.items[1], Item::Sep);
        // nothing of its own: no leading separator
        struct Empty;
        impl Content for Empty {
            fn measure(&mut self, _: &Frame, _: bool) -> i32 { 1 }
            fn draw(&mut self, _: &mut Canvas, _: &Frame) {}
        }
        assert!(matches!(&menu(&mut Empty, false, false).items[0], Item::Row(r) if r.id == std_ids::COMPACT));
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
    fn requests_are_recorded_not_done() {
        let mut ctx = Ctx::new(42);
        ctx.redraw();
        ctx.every(1, 1000);
        ctx.stop(1);
        ctx.open_panel();
        ctx.toggle_panel();
        ctx.close_panel();
        ctx.open_menu();
        ctx.quit();
        assert_eq!(ctx.main_window(), 42);
        let s: Vec<String> = ctx.reqs.iter().map(|r| format!("{r:?}")).collect();
        assert_eq!(s, ["Redraw", "Every(1, 1000)", "Stop(1)", "OpenPanel", "TogglePanel", "ClosePanel", "Menu", "Quit"]);
    }

    #[test]
    fn the_menu_goes_above_the_strip_or_else_the_pointer() {
        assert_eq!(menu_anchor(Some((10, 20, 70, 68)), (500, 900)), (10, 20, 70, 68));
        assert_eq!(menu_anchor(None, (500, 900)), (500, 900, 501, 901), "no strip (tray icon): the pointer");
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
