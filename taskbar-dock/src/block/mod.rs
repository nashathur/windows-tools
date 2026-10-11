//! Block: the strip window and all its behaviour, owned by the library. A widget supplies a
//! [`Spec`] (adjustable parameters) and a [`Content`] (what to draw, what clicks do) and calls
//! [`run`]; everything else is here, the same for every widget:
//!
//! - the window, its procedure and timers; one instance per widget; panics logged
//! - press / drag / zoom / glide / compact resize ([`crate::strip::Motion`])
//! - dock slot, order, drop, fit next to the taskbar's buttons ([`crate::dock`]), WinEvent hooks
//! - left or right of the taskbar's buttons ([`Content::side`])
//! - Explorer restarts, and the floating fallback when the taskbar can't be joined
//! - hover highlight, button highlight boxes (neutral, or the Content's brand colour:
//!   [`Content::tint`]), the cover's hover / press feel
//! - when the panel opens and closes ([`crate::panel::PanelBehaviour`]), one panel across widgets
//! - the main (click / hover) panel's window (`shell.rs`): placed above the strip or the
//!   pointer, faded in, resized while open ([`Ctx::resize_panel`]), closed on Esc or lost
//!   focus; the Content only sizes and draws it
//!   ([`Content::panel_size`], [`Content::panel_draw`]) and gets its mouse ([`Content::panel_mouse`])
//! - the right-click panel with the standard rows (Show on taskbar, Compact mode, Start with
//!   Windows, Edit settings…, Quit)
//! - the tray icon, shown whenever the strip isn't (switched off, or nothing to show), so the
//!   panels stay reachable ([`Content::tray_icon`])
//! - the widget's own dialog (bus's stop picker) gets Tab / Enter / Esc ([`Spec::dialog`])
//! - following Windows' settings and the settings file live
//!
//! Ownership ("requests, applied afterwards"): Block keeps its state, the Content included, in
//! one thread-local `RefCell`. Content callbacks never touch windows; they record what they want
//! on a [`Ctx`] (`redraw`, `every`, `open_panel`, `quit`, `after`), and Block carries that out
//! once its borrow has ended. Code outside the callbacks (the widget's own windows, posts from
//! worker threads) reaches the Content through [`with`], which works the same way.

mod core;
mod frames;
mod place;
mod tray;
mod win;

use crate::menu_panel::{self, Item, Menu, Row};
use crate::style::Look;
use crate::theme::Theme;

pub use win::{run, strip_window, with};

/// Content timer ids must be below this ([`Ctx::every`]); Block's own are above.
pub const CONTENT_TIMERS: usize = 1000;

/// A box in strip px: (left, top, right, bottom), right/bottom exclusive (as in [`crate::draw`]).
pub type Box4 = (i32, i32, i32, i32);

/// Which end of the taskbar the strip sits at.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Side {
    /// In the dock stack against the tray (the usual place).
    #[default]
    Right,
    /// From the taskbar's left edge, after Windows' Widgets button, around other apps' docked
    /// windows (weather's `label_position: "left"`); not a dock member.
    Left,
}

/// Where a floating strip goes.
#[derive(Clone, Copy, Debug)]
pub enum FloatAt {
    /// Left of the tray and of the widgets docked there; bottom right of the screen without a
    /// taskbar. Not draggable while floating (no dock order to drag in). Media.
    BesideDock,
    /// At the x saved in this file (screen px); dragged freely, the new x saved on drop. Bus.
    Saved(fn() -> std::path::PathBuf),
}

/// What happens when the taskbar can't be joined.
#[derive(Clone, Copy, Debug)]
pub enum Float {
    /// Keep trying to attach (every watch tick); nothing shows meanwhile.
    Off,
    /// Attaching failed, or no taskbar for 10 s: a topmost window instead; re-attaches when it
    /// can; hides while a full-screen app is in front (the taskbar does that when attached).
    Fallback(FloatAt),
}

/// What a left click on the strip (outside its buttons and cover) does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Click {
    /// Opens / pins / closes the panel ([`crate::panel::PanelBehaviour::click`]).
    TogglePanel,
    /// [`Content::click`].
    Content,
}

/// The adjustable parameters. Defaults (from [`Spec::new`]) are what the weather widget does.
#[derive(Clone, Debug)]
pub struct Spec {
    /// Window class of the strip; also its name in the dock order (order.txt).
    pub class: &'static str,
    /// Settings section: its `compact` key, and the name on the startup list.
    pub section: &'static str,
    /// Window title.
    pub title: &'static str,
    /// Appends a line to the widget's log.
    pub log: fn(&str),
    /// Named mutex for one instance only (default `Local\<class>.SingleInstance`).
    pub mutex: String,
    pub float: Float,
    /// Float from the start (e.g. a `--float` argument).
    pub force_float: bool,
    pub draggable: bool,
    pub click: Click,
    pub hover_panel: bool,
    /// Crowded: compact before hiding (false: straight from full to hidden).
    pub compact_when_crowded: bool,
    /// Crowded: may take any width between compact and full ([`crate::dock::Size::stretch`]).
    pub stretch: bool,
    /// The widget's log file, for dockctl's `log` (None: dockctl says there is none).
    pub log_file: Option<std::path::PathBuf>,
    /// The widget's own dialog-like window open right now (0: none), e.g. bus's stop picker:
    /// Block's message loop gives it Tab / Enter / Esc (`IsDialogMessageW`). The widget opens it
    /// (`ctx.after`) and closes it itself; Quit ends it with the program.
    pub dialog: Option<fn() -> isize>,
}

impl Spec {
    pub fn new(class: &'static str, section: &'static str, title: &'static str, log: fn(&str)) -> Spec {
        Spec {
            class,
            section,
            title,
            log,
            mutex: format!("Local\\{class}.SingleInstance"),
            float: Float::Fallback(FloatAt::BesideDock),
            force_float: false,
            draggable: true,
            click: Click::TogglePanel,
            hover_panel: true,
            compact_when_crowded: true,
            stretch: false,
            log_file: None,
            dialog: None,
        }
    }
}

/// Which of the widget's panels ([`Content::panel_size`]); the right-click panel is Block's own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelKind {
    /// The click / hover panel.
    Main,
}

/// Everything [`Content::panel_draw`] needs.
#[derive(Clone, Copy, Debug)]
pub struct PanelFrame<'a> {
    pub look: &'a Look,
    /// Display scale of the panel's monitor (1.0 = 100%).
    pub scale: f32,
    /// The panel's size, px (what [`Content::panel_size`] gave).
    pub w: i32,
    pub h: i32,
}

impl PanelFrame<'_> {
    /// `v` px at 100%, at this panel's scale, whole px.
    pub fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    pub fn theme(&self) -> Theme {
        Theme::new(self.look)
    }
}

/// The mouse on a panel ([`Content::panel_mouse`]), panel px.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelMouse {
    Move(i32, i32),
    Down(i32, i32),
    Up(i32, i32),
    /// It left the panel.
    Leave,
}

/// What is under the mouse on the strip ([`Content::hit`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Hit {
    /// The strip itself: drag, click panel, whole-strip highlight.
    #[default]
    Strip,
    /// A button: [`Content::button`] on release over it; highlighted in its box (get the box
    /// from [`crate::draw::button_box`]: the hitbox is the highlight box).
    Button(usize, Box4),
    /// The cover or logo: [`Content::cover_click`] on release over it; grows on hover, dips on
    /// press ([`crate::anim::Feel`]); `Box4` is its resting box with the ring.
    Cover(Box4),
}

/// A brand colour for the strip's highlight boxes ([`Content::tint`]), instead of Block's
/// neutral one. Drawn under the Content's pixels with the shared highlight shape.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Tint {
    /// 0xRRGGBB.
    pub rgb: u32,
    /// Opacity (0..1) at rest (0: only on hover) and while hovered.
    pub rest: f32,
    pub hover: f32,
}

/// Everything [`Content::measure`] and [`Content::draw`] need.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    pub look: &'a Look,
    /// The strip's height (the taskbar's), px.
    pub height: i32,
    /// Display scale (1.0 = 100%).
    pub dpi: f32,
    /// Drag zoom (1.0 at rest).
    pub zoom: f32,
    /// `dpi × zoom`: multiply sizes at 100% by this (the content grows while dragged).
    pub scale: f32,
    /// Draw the compact mode.
    pub compact: bool,
    /// The mouse is on the strip.
    pub hover: bool,
    /// What it's over ([`Content::hit`]); meaningful while `hover`.
    pub hot: Hit,
    /// The button is down on the strip.
    pub pressed: bool,
    /// Floating rather than in the taskbar.
    pub floating: bool,
    /// The width the strip is drawn at, px at this frame's zoom: crowded, for a stretchable strip
    /// ([`Spec::stretch`]), or while a compact switch animates the width (the full content, at
    /// every width between compact and full). Lay out to fit it if you can (narrow a text column,
    /// keep what sits at the right end); Block cuts off anything wider, fading the end. None: the
    /// natural width.
    pub fit_width: Option<i32>,
}

impl Frame<'_> {
    /// `v` px at 100%, at this frame's scale, whole px.
    pub fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    pub fn theme(&self) -> Theme {
        Theme::new(self.look)
    }
}

/// A 32-bit top-down DIB section with a memory DC, for [`Content::draw`]. Pixels are
/// premultiplied BGRA (`0xAARRGGBB` as u32), all 0 (transparent) at the start. Draw GDI text
/// into `dc` (GDI leaves alpha 0: turn coverage into colour yourself, as the widgets do), or
/// write `pixels()` directly. `dc` is a windows-sys HDC; widgets on the `windows` crate wrap it
/// (`HDC(cv.dc as _)`).
pub struct Canvas {
    pub dc: isize,
    pub w: i32,
    pub h: i32,
    bits: *mut u32,
    bmp: isize,
    old: isize,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Option<Canvas> {
        use windows_sys::Win32::Graphics::Gdi::*;
        if w <= 0 || h <= 0 {
            return None;
        }
        unsafe {
            let dc = CreateCompatibleDC(0);
            if dc == 0 {
                return None;
            }
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h; // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, 0, 0);
            if bmp == 0 || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bmp);
            SetBkMode(dc, TRANSPARENT as i32);
            Some(Canvas { dc, w, h, bits: bits as *mut u32, bmp, old })
        }
    }

    /// The pixels, row by row (GDI drawing so far is flushed first).
    pub fn pixels(&mut self) -> &mut [u32] {
        unsafe {
            windows_sys::Win32::Graphics::Gdi::GdiFlush();
            std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize)
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        use windows_sys::Win32::Graphics::Gdi::*;
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

/// What a Content callback wants done; Block does it after the callback returns.
pub struct Ctx {
    pub(crate) reqs: Vec<Request>,
    main: isize,
}

pub(crate) enum Request {
    Redraw,
    RedrawPanel,
    ResizePanel,
    RedrawTray,
    Every(usize, u32),
    Stop(usize),
    OpenPanel,
    TogglePanel,
    ClosePanel,
    Menu,
    Quit,
    After(Box<dyn FnOnce()>),
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Request::Redraw => write!(f, "Redraw"),
            Request::RedrawPanel => write!(f, "RedrawPanel"),
            Request::ResizePanel => write!(f, "ResizePanel"),
            Request::RedrawTray => write!(f, "RedrawTray"),
            Request::Every(id, ms) => write!(f, "Every({id}, {ms})"),
            Request::Stop(id) => write!(f, "Stop({id})"),
            Request::OpenPanel => write!(f, "OpenPanel"),
            Request::TogglePanel => write!(f, "TogglePanel"),
            Request::ClosePanel => write!(f, "ClosePanel"),
            Request::Menu => write!(f, "Menu"),
            Request::Quit => write!(f, "Quit"),
            Request::After(_) => write!(f, "After(..)"),
        }
    }
}

impl Ctx {
    pub(crate) fn new(main: isize) -> Ctx {
        Ctx { reqs: Vec::new(), main }
    }

    /// Measure and draw the strip again (new data, a state change; also shows or hides it when
    /// [`Content::visible`] changed).
    pub fn redraw(&mut self) {
        self.reqs.push(Request::Redraw);
    }

    /// Draw the open panel again ([`Content::panel_draw`]; nothing if none is open). Separate
    /// from [`Ctx::redraw`], so a strip animation doesn't repaint the panel every frame. A look
    /// change repaints it anyway.
    pub fn redraw_panel(&mut self) {
        self.reqs.push(Request::RedrawPanel);
    }

    /// The open panel's size changed (discord: people joined or left): Block asks
    /// [`Content::panel_size`] again, moves it by the same rule as when it opened (above the
    /// strip, or the tray icon), no fade, and draws it again. Nothing if none is open.
    pub fn resize_panel(&mut self) {
        self.reqs.push(Request::ResizePanel);
    }

    /// The tray logo changed (bus: another line, or its badge colour): Block asks
    /// [`Content::tray_icon`] again. A look change redraws it anyway.
    pub fn redraw_tray(&mut self) {
        self.reqs.push(Request::RedrawTray);
    }

    /// Call [`Content::timer`] with `id` every `ms` (replaces a running one with the same id).
    /// `id` < [`CONTENT_TIMERS`]. Animation frames (a pulse, a marquee) and data refreshes alike.
    pub fn every(&mut self, id: usize, ms: u32) {
        debug_assert!(id < CONTENT_TIMERS);
        self.reqs.push(Request::Every(id, ms));
    }

    pub fn stop(&mut self, id: usize) {
        self.reqs.push(Request::Stop(id));
    }

    /// Open the panel as a click would (pinned, with focus), if it isn't open.
    pub fn open_panel(&mut self) {
        self.reqs.push(Request::OpenPanel);
    }

    /// Open, pin or close the panel as a click on the strip does (e.g. a tray icon's left click).
    /// Works without a strip: the panel then opens above the pointer.
    pub fn toggle_panel(&mut self) {
        self.reqs.push(Request::TogglePanel);
    }

    pub fn close_panel(&mut self) {
        self.reqs.push(Request::ClosePanel);
    }

    /// Open the right-click panel as a right click on the strip does: above the strip, or above
    /// the pointer while there's none (e.g. a tray icon's right click).
    pub fn open_menu(&mut self) {
        self.reqs.push(Request::Menu);
    }

    /// End the program (as the Quit row does).
    pub fn quit(&mut self) {
        self.reqs.push(Request::Quit);
    }

    /// Run `f` after the callback, outside Block's borrow: for Win32 calls that may send
    /// messages back (`window::raise`, ShellExecute, creating or destroying windows, ...).
    pub fn after(&mut self, f: impl FnOnce() + 'static) {
        self.reqs.push(Request::After(Box::new(f)));
    }

    /// The widget's hidden main window: worker threads post their results to it (handled in
    /// [`Content::on_message`]).
    pub fn main_window(&self) -> isize {
        self.main
    }
}

/// Lets [`with`] find the widget's own type behind `dyn Content`.
pub trait AsAny {
    fn as_any(&mut self) -> &mut dyn std::any::Any;
}

impl<T: std::any::Any> AsAny for T {
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// What the widget supplies: what the strip shows and what its clicks do. Only `measure` and
/// `draw` are required. Callbacks with a [`Ctx`] record requests; the others must not change
/// anything outside `self`.
pub trait Content: AsAny + 'static {
    /// The strip's natural width for `f` (full or `compact`), px. Called unzoomed for the dock's
    /// fit and at the drag zoom for drawing.
    fn measure(&mut self, f: &Frame, compact: bool) -> i32;

    /// Draws the strip's foreground (text, icons, cover, buttons) into `cv`, `cv.w` wide (the
    /// width `measure` gave for `f`). Block then grows the cover ([`Content::cover`]), cuts the
    /// strip off during a compact resize, and puts the background and hover highlight under it.
    fn draw(&mut self, cv: &mut Canvas, f: &Frame);

    /// What is at strip px (x, y), as last drawn.
    fn hit(&self, _x: i32, _y: i32) -> Hit {
        Hit::Strip
    }

    /// The cover's resting box (with its ring) as last drawn, for the hover / press feel.
    fn cover(&self) -> Option<Box4> {
        None
    }

    /// The highlight's colour over `area`: [`Hit::Strip`] for the whole strip, a [`Hit::Button`]
    /// for that button's box. None for the strip: Block's neutral highlight (the default). With a
    /// strip tint, Block draws the hovered button's box (its tint, else the strip's) at `hover`,
    /// then the whole strip at `rest` or `hover` (discord: always in a blurple box).
    fn tint(&self, _area: Hit, _look: &Look) -> Option<Tint> {
        None
    }

    /// Anything to show (state, e.g. discord in a call)? False: the strip is destroyed (its panels
    /// closed) until this is true again. The user's Show on taskbar switch is Block's: the strip
    /// shows when both are on. Checked on every redraw and every second.
    fn visible(&self) -> bool {
        true
    }

    fn side(&self) -> Side {
        Side::Right
    }

    /// Once, before the strip is created: start data timers, the first fetch, a tray icon.
    fn start(&mut self, _ctx: &mut Ctx) {}

    /// A content timer ([`Ctx::every`]).
    fn timer(&mut self, _id: usize, _ctx: &mut Ctx) {}

    /// Left click on the strip with [`Click::Content`].
    fn click(&mut self, _ctx: &mut Ctx) {}

    /// Released over the button `id` it was pressed on.
    fn button(&mut self, _id: usize, _ctx: &mut Ctx) {}

    /// Released over the cover it was pressed on: bring the app to the front
    /// (`ctx.after(|| window::raise(..))`).
    fn cover_click(&mut self, _ctx: &mut Ctx) {}

    /// The panel's size at display scale `scale`, px (None: the widget has no such panel; a
    /// click then opens nothing). Asked each time it opens.
    fn panel_size(&mut self, _kind: PanelKind, _scale: f32) -> Option<(i32, i32)> {
        None
    }

    /// Draws the open panel into `cv` (`f.w` × `f.h`), already filled with the theme's
    /// background. Opaque GDI (ClearType text is fine: the panel fades as a whole). Called on
    /// every paint: after [`Ctx::redraw_panel`], a look change, or Windows uncovering it.
    fn panel_draw(&mut self, _kind: PanelKind, _cv: &mut Canvas, _f: &PanelFrame) {}

    /// The mouse on the open panel (Esc, lost focus and right clicks are Block's).
    fn panel_mouse(&mut self, _kind: PanelKind, _ev: PanelMouse, _ctx: &mut Ctx) {}

    /// The right-click panel's own rows; Block adds the standard ones ([`MenuBuilder`]).
    fn menu(&mut self, _m: &mut MenuBuilder) {}

    /// A switch row of the widget's flipped: do it, return its new state (the panel stays open).
    fn menu_switch(&mut self, _id: usize, _ctx: &mut Ctx) -> bool {
        false
    }

    /// An action row of the widget's chosen (the panel has closed).
    fn menu_action(&mut self, _id: usize, _ctx: &mut Ctx) {}

    /// settings.json changed on disk (Edit settings…, another widget): reload what's yours.
    fn settings_changed(&mut self, _ctx: &mut Ctx) {}

    /// Windows' theme, scaling, text size or another setting in the look changed (the strip is
    /// redrawn anyway, an open panel too).
    fn look_changed(&mut self, _ctx: &mut Ctx) {}

    /// Explorer restarted (the strip is re-created by Block): e.g. add a tray icon again.
    fn taskbar_restarted(&mut self, _ctx: &mut Ctx) {}

    /// Any message to the main window Block doesn't handle (worker posts at `WM_APP + n`, a
    /// tray icon's callback). Some(result): handled.
    fn on_message(&mut self, _msg: u32, _wp: usize, _lp: isize, _ctx: &mut Ctx) -> Option<isize> {
        None
    }

    /// The program is ending: clean up (worker threads).
    fn closing(&mut self) {}

    /// Draws the tray icon (a static logo, not data) into `cv` (the small-icon size, square),
    /// colours from `look`/theme. False: the widget has no tray icon; then it has no Show on
    /// taskbar switch either (nothing could switch the strip back on).
    fn tray_icon(&mut self, _cv: &mut Canvas, _look: &Look) -> bool {
        false
    }

    /// The tray icon's tooltip (read again on every redraw; ~127 characters at most).
    fn tray_tip(&self) -> String {
        String::new()
    }
}

/// Ids of the standard rows (the widget's own ids must stay below these).
pub mod std_ids {
    pub const COMPACT: usize = 0xF000;
    pub const STARTUP: usize = 0xF001;
    pub const SETTINGS: usize = 0xF002;
    pub const QUIT: usize = 0xF003;
    pub const SHOW: usize = 0xF004;
}

/// The right-click panel as the widget wants it: a header, its own rows, and where the standard
/// switches go. [`MenuBuilder::show`] / [`MenuBuilder::compact`] / [`MenuBuilder::startup`] put
/// them at that point; switches not placed go after the widget's rows, in that order (with its
/// switches when it ended on one, else after a separator). Edit settings… and Quit always end it.
#[derive(Default, Debug)]
pub struct MenuBuilder {
    menu: Menu,
    /// The Show on taskbar switch (None: the widget has no tray icon, so no switch).
    show_on: Option<bool>,
    compact_on: bool,
    startup_on: bool,
    placed_show: bool,
    placed_compact: bool,
    placed_startup: bool,
}

impl MenuBuilder {
    pub(crate) fn new(show_on: Option<bool>, compact_on: bool, startup_on: bool) -> MenuBuilder {
        MenuBuilder { show_on, compact_on, startup_on, ..Default::default() }
    }

    pub fn title(&mut self, t: impl Into<String>) -> &mut Self {
        self.menu.title = t.into();
        self
    }

    pub fn subtitle(&mut self, t: impl Into<String>) -> &mut Self {
        self.menu.subtitle = t.into();
        self
    }

    /// One of the widget's rows (`Row::action`, `Row::switch`, `Item::Pair`).
    pub fn item(&mut self, item: Item) -> &mut Self {
        self.menu.items.push(item);
        self
    }

    /// A separator (never two in a row, never first).
    pub fn sep(&mut self) -> &mut Self {
        if !matches!(self.menu.items.last(), None | Some(Item::Sep)) {
            self.menu.items.push(Item::Sep);
        }
        self
    }

    /// The Show on taskbar switch here (nothing for a widget without a tray icon).
    pub fn show(&mut self) -> &mut Self {
        self.placed_show = true;
        if let Some(on) = self.show_on {
            self.menu.items.push(Row::switch(std_ids::SHOW, "Show on taskbar", on));
        }
        self
    }

    /// The Compact mode switch here.
    pub fn compact(&mut self) -> &mut Self {
        self.placed_compact = true;
        self.menu.items.push(Row::switch(std_ids::COMPACT, "Compact mode", self.compact_on));
        self
    }

    /// The Start with Windows switch here.
    pub fn startup(&mut self) -> &mut Self {
        self.placed_startup = true;
        self.menu.items.push(Row::switch(std_ids::STARTUP, "Start with Windows", self.startup_on));
        self
    }

    /// The finished panel: standard switches not placed yet, then Edit settings… / Quit.
    pub(crate) fn finish(mut self) -> Menu {
        let show = self.show_on.is_some() && !self.placed_show;
        if show || !self.placed_compact || !self.placed_startup {
            // after the widget's rows: with its switches, or in a group of their own
            if !matches!(self.menu.items.last(), Some(Item::Row(r)) if r.switch.is_some()) {
                self.sep();
            }
            if show {
                self.show();
            }
            if !self.placed_compact {
                self.compact();
            }
            if !self.placed_startup {
                self.startup();
            }
        }
        self.sep();
        self.menu.items.push(menu_panel::settings_and_quit(std_ids::SETTINGS, std_ids::QUIT));
        self.menu
    }
}
