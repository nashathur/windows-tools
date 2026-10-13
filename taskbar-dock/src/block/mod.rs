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
//! - following Windows' settings and the settings file live
//!
//! Ownership ("requests, applied afterwards"): Block keeps its state, the Content included, in
//! one thread-local `RefCell`. Content callbacks never touch windows; they record what they want
//! on a [`Ctx`] (`redraw`, `every`, `close_panel`, `quit`, `after`), and Block carries that out
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

pub use win::run;

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

/// The adjustable parameters. Defaults (from [`Spec::new`]) are what the weather widget does.
#[derive(Clone, Debug)]
pub struct Spec {
    /// Window class of the strip; also its name in the dock order (order.txt).
    pub class: &'static str,
    /// Settings section: its `compact` key, and the name on the startup list.
    pub section: &'static str,
    /// Window title.
    pub title: &'static str,
    /// One line shown beside this widget in the settings app.
    pub description: &'static str,
    /// The widget's setting descriptions.
    pub settings: Option<fn() -> Vec<crate::describe::Setting>>,
    /// Appends a line to the widget's log.
    pub log: fn(&str),
    /// Named mutex for one instance only (default `Local\<class>.SingleInstance`).
    pub mutex: String,
    pub float: Float,
    /// Float from the start (e.g. a `--float` argument).
    pub force_float: bool,
    pub draggable: bool,
    pub hover_panel: bool,
    /// Crowded: compact before hiding (false: straight from full to hidden).
    pub compact_when_crowded: bool,
    /// Crowded: may take any width between compact and full ([`crate::dock::Size::stretch`]).
    pub stretch: bool,
    /// The width follows the content (media, discord): the strip glides to a new
    /// measured width like a compact switch, its neighbours following; crowded refits glide too.
    /// Meanwhile [`Frame::fit_width`] may be wider than the content: lay out to it. False: a new
    /// width jumps (weather, bus).
    pub glide_width: bool,
    /// The widget's log file, for dockctl's `log` (None: dockctl says there is none).
    pub log_file: Option<std::path::PathBuf>,
    /// The widget's stored configuration defaults, before Block adds its own settings.
    pub defaults: Option<fn() -> serde_json::Value>,
}

impl Spec {
    pub fn new(
        class: &'static str,
        section: &'static str,
        title: &'static str,
        log: fn(&str),
    ) -> Spec {
        Spec {
            class,
            section,
            title,
            description: "",
            settings: None,
            log,
            mutex: format!("Local\\{class}.SingleInstance"),
            float: Float::Fallback(FloatAt::BesideDock),
            force_float: false,
            draggable: true,
            hover_panel: true,
            compact_when_crowded: true,
            stretch: false,
            glide_width: false,
            log_file: None,
            defaults: None,
        }
    }
}

/// Metadata returned by a widget's side-effect-free `--describe` entry point.
#[derive(Clone, Debug, PartialEq)]
pub struct Describe {
    /// Settings section name used by this widget.
    pub section: String,
    /// Window class used for the widget's hidden Block window.
    pub class: String,
    /// Display name shown by the settings app.
    pub title: String,
    /// One-line description shown by the settings app.
    pub description: String,
    /// All settings exposed by this widget, including Block's settings.
    pub settings: Vec<crate::describe::Setting>,
    /// Stored defaults for every described setting.
    pub defaults: serde_json::Value,
    /// Optional path to the widget log file.
    pub log_file: Option<std::path::PathBuf>,
}

/// Builds a widget description and adds Block's shared settings and defaults.
pub fn describe(
    section: &str,
    class: &str,
    title: &str,
    description: &str,
    settings: Option<fn() -> Vec<crate::describe::Setting>>,
    defaults: Option<fn() -> serde_json::Value>,
    log_file: Option<std::path::PathBuf>,
) -> Describe {
    let mut defaults = defaults
        .map(|make| make())
        .unwrap_or_else(|| serde_json::json!({}));
    if !defaults.is_object() {
        defaults = serde_json::json!({});
    }
    defaults["show_on_taskbar"] = serde_json::json!(true);
    defaults["compact"] = serde_json::json!(false);
    defaults["highlight"] = serde_json::json!("auto");
    defaults["highlight_color"] = serde_json::json!("#0078D4");
    defaults["highlight_strength"] = serde_json::json!(1.0);

    let mut settings = settings.map(|make| make()).unwrap_or_default();
    settings.push(crate::describe::Setting::new(
        "show_on_taskbar",
        "Show on taskbar",
        crate::describe::Kind::Switch,
    ));
    settings.push(crate::describe::Setting::new(
        "compact",
        "Compact mode",
        crate::describe::Kind::Switch,
    ));
    settings.push(
        crate::describe::Setting::new(
            "highlight",
            "Mode",
            crate::describe::Kind::Choice {
                options: vec![
                    (serde_json::json!("auto"), "Automatic".into()),
                    (serde_json::json!("custom"), "Chosen colour".into()),
                    (serde_json::json!("off"), "Off".into()),
                ],
                other: false,
            },
        )
        .group("Highlight box")
        .help("Automatic: the colour of what the widget shows. Off: the plain grey box."),
    );
    settings.push(
        crate::describe::Setting::new(
            "highlight_color",
            "Colour",
            crate::describe::Kind::Color {
                hash: true,
                empty_ok: false,
            },
        )
        .group("Highlight box")
        .help("The box's colour.")
        .shown_when("highlight", serde_json::json!("custom")),
    );
    settings.push(
        crate::describe::Setting::number("highlight_strength", "Strength", 0.0, 3.0, 0.05)
            .group("Highlight box")
            .help("How strong the box is: 1 is the usual, 0 none, 3 the strongest."),
    );

    Describe {
        section: section.into(),
        class: class.into(),
        title: title.into(),
        description: description.into(),
        settings,
        defaults,
        log_file,
    }
}

impl Describe {
    /// Serializes this description for `--describe` and dockctl's describe request.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "describe": DESCRIBE_VERSION,
            "section": self.section,
            "class": self.class,
            "title": self.title,
            "description": self.description,
            "settings": crate::describe::to_json(&self.settings),
            "defaults": self.defaults,
            "log_file": self.log_file.as_ref().map(|path| path.display().to_string()),
        })
    }
}

/// Current version of the widget description JSON format.
pub const DESCRIBE_VERSION: u32 = 1;

impl Describe {
    /// Parses and validates the JSON representation of a widget description.
    pub fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        if v.get("describe").and_then(serde_json::Value::as_u64) != Some(DESCRIBE_VERSION as u64) {
            return Err("unsupported describe version".into());
        }
        let string = |key: &str| {
            v.get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("{key} must be text"))
        };
        let settings = crate::describe::from_json(v.get("settings").ok_or("missing settings")?)?;
        let defaults = v
            .get("defaults")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or("defaults must be an object")?;
        for setting in &settings {
            let value = crate::describe::value_at(&defaults, &setting.key)
                .ok_or_else(|| format!("default missing for {}", setting.key))?;
            crate::describe::check(setting, value)
                .map_err(|error| format!("invalid default for {}: {error}", setting.key))?;
        }
        /// Collects the leaf keys in a defaults object.
        fn leaves(value: &serde_json::Value, prefix: String, output: &mut Vec<String>) {
            if let Some(object) = value.as_object() {
                for (key, value) in object {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    leaves(value, path, output);
                }
            } else {
                output.push(prefix);
            }
        }
        let mut keys = Vec::new();
        leaves(&defaults, String::new(), &mut keys);
        for key in keys {
            if !settings.iter().any(|setting| setting.key == key) {
                return Err(format!("default has no setting description: {key}"));
            }
        }
        let log_file = v
            .get("log_file")
            .and_then(serde_json::Value::as_str)
            .map(std::path::PathBuf::from);
        Ok(Self {
            section: string("section")?,
            class: string("class")?,
            title: string("title")?,
            description: string("description")?,
            settings,
            defaults,
            log_file,
        })
    }
}

/// Writes a description and exits before the widget initializes any runtime state.
pub fn describe_if_asked(describe: fn() -> Describe) {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--describe" {
        let output = serde_json::to_vec_pretty(&describe().to_json()).unwrap_or_default();
        let result = std::fs::write(&args[2], output);
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
}

/// Which of the widget's panels ([`Content::panel_size`]); the right-click panel is Block's own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelKind {
    /// The click / hover panel.
    Main,
    /// A focused picker or other secondary panel.
    Picker,
}

/// Everything [`Content::panel_draw`] needs.
#[derive(Clone, Copy, Debug)]
pub struct PanelFrame<'a> {
    pub look: &'a Look,
    /// The resolved Highlight box setting for this widget.
    pub highlight: Highlight,
    /// Display scale of the panel's monitor (1.0 = 100%).
    pub scale: f32,
    /// The panel's size, px (what [`Content::panel_size`] gave).
    pub w: i32,
    pub h: i32,
}

impl PanelFrame<'_> {
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
    DoubleClick(i32, i32),
    /// It left the panel.
    Leave,
    /// Pressed, but the panel lost the mouse before the release (Esc, another window took it):
    /// end a drag without acting.
    Cancel,
}

/// A normalized panel key and its explicit modifiers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PanelKey {
    pub key: PanelKeyCode,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelKeyCode {
    Enter,
    Escape,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Backspace,
    Delete,
    Left,
    Right,
    Tab,
    A,
    C,
    V,
    X,
    Other(u32),
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
/// neutral one. Drawn under the Content's pixels with the shared highlight shape. How strong is
/// Block's, from the shared levels (`Look::tint`, the `dock` section): the Content only says what
/// state it's in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Tint {
    /// 0xRRGGBB.
    pub rgb: u32,
    /// 0: paused / muted (the quiet level), 1: playing / unmuted (active); in between while it
    /// fades from one to the other. Hovered it's the hover level either way.
    pub active: f32,
    /// Set by Block from the widget's Highlight box Strength setting; a Content leaves it 1.
    pub strength: f32,
}

/// Block-owned choice for the strip's coloured highlight boxes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Highlight {
    pub mode: HighlightMode,
    /// Chosen colour, as 0xRRGGBB.
    pub rgb: u32,
    pub strength: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HighlightMode { Auto, Custom, Off }

impl Default for Highlight {
    fn default() -> Self { Self { mode: HighlightMode::Auto, rgb: 0x0078D4, strength: 1.0 } }
}

impl Highlight {
    /// Resolve a Content-provided automatic tint under Block's mode, colour and strength rules.
    pub fn apply(&self, auto: Option<Tint>, strip_auto_rgb: Option<u32>) -> Option<Tint> {
        match self.mode {
            HighlightMode::Off => None,
            HighlightMode::Auto => auto.map(|mut t| { t.strength = self.strength; t }),
            HighlightMode::Custom => Some(match auto {
                Some(mut t) => {
                    if Some(t.rgb) == strip_auto_rgb { t.rgb = self.rgb; }
                    t.strength = self.strength;
                    t
                }
                None => { let mut t = Tint::new(self.rgb, true); t.strength = self.strength; t }
            }),
        }
    }
}

impl Tint {
    /// `rgb` at the active (playing / unmuted: `true`) or quiet level.
    pub fn new(rgb: u32, active: bool) -> Tint {
        Tint {
            rgb,
            active: if active { 1.0 } else { 0.0 },
            strength: 1.0,
        }
    }

    /// Its opacities (0..1) at rest and hovered, for the levels `l`.
    pub fn strengths(&self, l: &crate::theme::TintLevels) -> (f32, f32) {
        let a = self.active.clamp(0.0, 1.0);
        let k = self.strength.max(0.0);
        (
            ((l.quiet + (l.active - l.quiet) * a) * k).min(1.0),
            (l.hover * k).min(1.0),
        )
    }
}

/// Everything [`Content::measure`] and [`Content::draw`] need.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    pub look: &'a Look,
    /// The resolved Highlight box setting for this widget.
    pub highlight: Highlight,
    /// The strip's height (the taskbar's), px.
    pub height: i32,
    /// Display scale × drag zoom: multiply sizes at 100% by this (the content grows while
    /// dragged).
    pub scale: f32,
    /// Draw the compact mode.
    pub compact: bool,
    /// The width the strip is drawn at, px at this frame's zoom: crowded, for a stretchable strip
    /// ([`Spec::stretch`]), while a compact switch animates the width (the full content, at
    /// every width between compact and full), or with [`Spec::glide_width`] while the width
    /// glides or is held (then it may be wider than the content). Lay out to fit it if you can
    /// (narrow or widen a text column up to its maximum, keep what sits at the right end) and
    /// measure as that width; Block cuts off anything wider, fading the end. None: the natural
    /// width.
    pub fit_width: Option<i32>,
}

impl Frame<'_> {
    /// `v` px at 100%, at this frame's scale, whole px.
    pub fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
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
            Some(Canvas {
                dc,
                w,
                h,
                bits: bits as *mut u32,
                bmp,
                old,
            })
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

/// A command error that dockctl can display.
pub struct CommandError {
    /// Dockctl exits 2 and shows the command's usage.
    pub usage: bool,
    /// The error text.
    pub message: String,
}

/// A command that finishes now or will answer through its token later.
pub enum CommandReply {
    Done(Result<serde_json::Value, CommandError>),
    Pending,
}

pub(crate) struct CommandResult {
    pub requester: isize,
    pub id: u64,
    pub result: Result<serde_json::Value, CommandError>,
    pub usage: Option<&'static str>,
}

/// Posted to Block's main window when a command token is finished.
pub const WM_COMMAND_REPLY: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 0x3F01;

/// Completes one asynchronous dockctl command from any thread.
pub struct ReplyToken {
    main: isize,
    requester: isize,
    id: u64,
    finished: bool,
    usage: Option<&'static str>,
    logger: fn(&str),
}

impl ReplyToken {
    pub(crate) fn new(
        main: isize,
        requester: isize,
        id: u64,
        usage: Option<&'static str>,
        logger: fn(&str),
    ) -> Self {
        Self {
            main,
            requester,
            id,
            finished: false,
            usage,
            logger,
        }
    }

    /// Creates a token that answers nobody, for widget unit tests.
    pub fn detached() -> Self {
        Self {
            main: 0,
            requester: 0,
            id: 0,
            finished: false,
            usage: None,
            logger: |_| {},
        }
    }

    fn post(&mut self, result: Result<serde_json::Value, CommandError>) {
        self.finished = true;
        if self.main == 0 {
            return;
        }
        let payload = Box::new(CommandResult {
            requester: self.requester,
            id: self.id,
            result,
            usage: self.usage,
        });
        let raw = Box::into_raw(payload);
        let posted = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                self.main,
                WM_COMMAND_REPLY,
                0,
                raw as isize,
            )
        };
        if posted == 0 {
            unsafe {
                drop(Box::from_raw(raw));
            }
            (self.logger)("dockctl: dropping command reply because the main window is gone");
        }
    }

    /// Posts the result to Block's main window, which sends it to dockctl.
    pub fn finish(mut self, result: Result<serde_json::Value, CommandError>) {
        self.post(result);
    }
}

impl Drop for ReplyToken {
    fn drop(&mut self) {
        if !self.finished {
            self.post(Err(CommandError {
                usage: false,
                message: "the command ended without an answer".into(),
            }));
        }
    }
}

pub(crate) enum Request {
    Redraw,
    RedrawPanel,
    ResizePanel,
    RedrawTray,
    Every(usize, u32),
    Stop(usize),
    OpenPanel,
    OpenPanelKind(PanelKind),
    TogglePanel,
    ClosePanel,
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
            Request::OpenPanelKind(kind) => write!(f, "OpenPanelKind({kind:?})"),
            Request::TogglePanel => write!(f, "TogglePanel"),
            Request::ClosePanel => write!(f, "ClosePanel"),
            Request::Quit => write!(f, "Quit"),
            Request::After(_) => write!(f, "After(..)"),
        }
    }
}

impl Ctx {
    pub(crate) fn new(main: isize) -> Ctx {
        Ctx {
            reqs: Vec::new(),
            main,
        }
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

    pub fn close_panel(&mut self) {
        self.reqs.push(Request::ClosePanel);
    }

    /// Open a specific content panel as a pinned, focused panel.
    pub fn open_panel_kind(&mut self, kind: PanelKind) {
        self.reqs.push(Request::OpenPanelKind(kind));
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

/// What the widget supplies: what the strip shows and what its clicks do. Only `measure` and
/// `draw` are required. Callbacks with a [`Ctx`] record requests; the others must not change
/// anything outside `self`.
pub trait Content: 'static {
    /// The widget's available `do` commands and their one-line usage.
    fn commands(&self) -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    /// Runs a widget-specific command. Slow work may finish through `token` on any thread.
    fn command(
        &mut self,
        _name: &str,
        _args: &[String],
        _token: ReplyToken,
        _ctx: &mut Ctx,
    ) -> CommandReply {
        CommandReply::Done(Err(CommandError {
            usage: true,
            message: "unknown command".into(),
        }))
    }

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

    /// Supplies only an automatic colour and active state for `area`; Block owns the Highlight box
    /// mode, chosen colour and strength. Return None when no automatic colour is available; Block
    /// then draws its neutral box. Buttons may return a separate automatic colour as well.
    fn tint(&self, _area: Hit, _look: &Look) -> Option<Tint> {
        None
    }

    /// Allows a Content to suppress every coloured highlight when the current display mode
    /// requires it (for example, media uses neutral boxes in Windows high contrast mode).
    fn tint_allowed(&self) -> bool { true }

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

    /// Block's Idle state changed (BLOCK.md "Idle"): pause / resume worker threads and fetches;
    /// on `false` fetch at once. Content timers are paused by Block, don't stop them yourself.
    /// A widget started switched off gets `idle(true)` before [`Content::start`].
    fn idle(&mut self, _idle: bool, _ctx: &mut Ctx) {}

    /// True when the widget's data stays live while idle (it keeps a cheap watch, e.g. Discord's
    /// call, media's sessions): its tray tooltip then stays its own instead of Block's "paused".
    fn live_while_idle(&self) -> bool {
        false
    }

    /// A content timer ([`Ctx::every`]).
    fn timer(&mut self, _id: usize, _ctx: &mut Ctx) {}

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

    /// The mouse on the open panel (Esc, lost focus and right clicks are Block's). Between `Down`
    /// and `Up` the panel has the mouse (a slider can be dragged off it); [`PanelMouse::Cancel`]
    /// if that ends otherwise.
    fn panel_mouse(&mut self, _kind: PanelKind, _ev: PanelMouse, _ctx: &mut Ctx) {}

    /// The panel has just opened (`open`) or closed, for whatever reason: e.g. start or stop a
    /// timer for what moves in it ([`Ctx::every`] + [`Ctx::redraw_panel`]), forget a hover.
    fn panel_shown(&mut self, _kind: PanelKind, _open: bool, _ctx: &mut Ctx) {}

    /// A key sent to the open panel. Return true when handled; Esc otherwise closes it.
    fn panel_key(&mut self, _kind: PanelKind, _key: PanelKey, _ctx: &mut Ctx) -> bool {
        false
    }

    /// Text input sent to the open panel.
    fn panel_char(&mut self, _kind: PanelKind, _text: &str, _ctx: &mut Ctx) {}

    /// Mouse wheel delta sent to the open panel.
    fn panel_wheel(&mut self, _kind: PanelKind, _delta: i32, _ctx: &mut Ctx) {}

    /// Widget-owned panel state, exposed by dockctl.
    fn panel_state(&self, _kind: PanelKind) -> serde_json::Value {
        serde_json::Value::Null
    }

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
        MenuBuilder {
            show_on,
            compact_on,
            startup_on,
            ..Default::default()
        }
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
            self.menu
                .items
                .push(Row::switch(std_ids::SHOW, "Show on taskbar", on));
        }
        self
    }

    /// The Compact mode switch here.
    pub fn compact(&mut self) -> &mut Self {
        self.placed_compact = true;
        self.menu.items.push(Row::switch(
            std_ids::COMPACT,
            "Compact mode",
            self.compact_on,
        ));
        self
    }

    /// The Start with Windows switch here.
    pub fn startup(&mut self) -> &mut Self {
        self.placed_startup = true;
        self.menu.items.push(Row::switch(
            std_ids::STARTUP,
            "Start with Windows",
            self.startup_on,
        ));
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
        self.menu.items.push(menu_panel::settings_and_quit(
            std_ids::SETTINGS,
            std_ids::QUIT,
        ));
        self.menu
    }
}
