#![windows_subsystem = "windows"]

//! Now playing on the Windows 11 taskbar, on the taskbar-dock library's Block (plan step 8): Block
//! owns the strip window, the dock, the panels' windows, the right-click panel's standard rows,
//! the tray icon, floating and following the settings; this is its Content. See PLAN.md.

mod apps;
mod audio;
mod brands;
mod config;
mod cover;
mod iconurl;
mod layout;
mod learned;
mod logos;
mod media;
mod panel;
mod panel_view;
mod render;
mod select;
mod sites;
mod timeline;
mod track;

use track::*;

use config::Config;
use layout::{Part, Strip};
use media::{Cmd, Msg, Session, Snapshot};
use render::{glyph, Geo, Ring, StripContent};
use timeline::Progress;
use std::ffi::c_void;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime};
use taskbar_dock::anim::{self, Appear};
use taskbar_dock::block::{self, Box4, Canvas, Content, Ctx, Float, FloatAt, Frame, Hit, MenuBuilder, PanelFrame, PanelKind, PanelMouse, Spec, Tint};
use taskbar_dock::menu_panel::Row;
use taskbar_dock::style::Look;
use taskbar_dock::draw;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::*;

/// A new snapshot from the media worker (lparam: Box<Snapshot>).
const WM_MEDIA: u32 = WM_APP + 1;

/// Content timers: every second (hide after pause, live streams, the brand file, the ring
/// moving along); the ring pulsing (25 frames a second while a full ring pulses or settles); the
/// strip's tint fading with playback; the open panel's frames.
const TIMER_WATCH: usize = 1;
const TIMER_BEAT: usize = 2;
const TIMER_TINT: usize = 3;
pub(crate) const TIMER_PANEL: usize = 4;
/// The strip's scrolling title / artist (~30 frames a second, as the panel's).
const TIMER_SCROLL: usize = 5;

/// The right-click panel's own rows.
const ID_OPEN: usize = 100;
const ID_COPY: usize = 101;

/// The widget's state: Block's Content.
pub(crate) struct App {
    /// Block's hidden main window: the media worker and the logo loader post there.
    main: isize,
    cfg: Config,
    /// Shared style + Windows settings, kept current.
    look: Look,
    media: Option<Sender<Msg>>,
    sessions: Vec<Session>,
    selector: select::Selector,
    /// The session shown, None = nothing to show (the strip goes).
    shown: Option<String>,
    /// The strip as last drawn (hit tests, the cover box) and its tint then.
    strip: Option<(Strip, Geo)>,
    drawn_tint: Option<Tint>,
    /// Vertical geometry by (height, scale, font, font size): it measures the font's ink.
    geos: Vec<((i32, u32, String, u32), Geo)>,
    /// The strip's lines (title, artist) and since when they're shown: a line too long for its
    /// column scrolls from that moment (marquee).
    scroll_since: Option<((String, String), Instant)>,
    /// The last drawing had a line too long for its column; `scrolling`: TIMER_SCROLL runs.
    scroll_long: bool,
    scrolling: bool,
    /// The ring pulse: since when it's been pulsing (playing), or where it was when playback
    /// paused and when, as it glides down (anim::pulse_settle).
    pulse_since: Option<Instant>,
    settle: Option<(f32, Instant)>,
    /// The shown session was playing at the last check.
    was_playing: bool,
    /// The pulse and tint timers are running.
    beating: bool,
    tinting: bool,
    /// Which apps report their position live (learned, kept in progress.txt).
    learned: learned::Learned,
    /// Tracks seen to be live streams (track_key), while they play.
    live: std::collections::HashSet<String>,
    /// When each track started looking live (confirmed after LIVE_CONFIRM).
    live_since: std::collections::HashMap<String, Instant>,
    /// Tracks seen playing (a paused track that never played is still loading, not live).
    played: std::collections::HashSet<String>,
    /// Tracks seen reporting a real length: never live (rapid pause/play makes Firefox send a
    /// stale paused report with no length that can last past LIVE_CONFIRM).
    has_length: std::collections::HashSet<String>,
    /// The brand file (media_brands.txt), reloaded when it changes.
    brands: brands::Brands,
    brands_mtime: Option<SystemTime>,
    /// How far the strip's tint is from its paused (quiet, 0) to its playing (active, 1) strength,
    /// fading with playback.
    tint: Appear,
    /// The click panel.
    panel: panel::State,
}

fn log_file() -> std::path::PathBuf {
    std::env::current_exe().map(|e| e.with_file_name("taskbar-media.log")).unwrap_or_else(|_| "taskbar-media.log".into())
}

/// Appends a line to taskbar-media.log next to the exe (kept small).
pub fn log(line: &str) {
    let path = log_file();
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let _ = writeln!(f, "{now} {line}");
    }
}

/// Puts text on the clipboard (`owner`: our main window).
fn copy_text(owner: isize, text: &str) {
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        if OpenClipboard(HWND(owner as *mut c_void)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let p = GlobalLock(mem) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                let _ = GlobalUnlock(mem);
                // the clipboard owns the memory from here
                let _ = SetClipboardData(CF_UNICODETEXT, HANDLE(mem.0));
            }
        }
        let _ = CloseClipboard();
    }
}

impl App {
    /// Sends a command to the media worker.
    pub(crate) fn send(&self, cmd: Cmd) {
        if let Some(tx) = &self.media {
            let _ = tx.send(Msg::Cmd(cmd));
        }
    }

    /// The strip's vertical geometry (cached: it renders the font to find its ink).
    fn geo(&mut self, height: i32, scale: f32, look: &Look) -> Geo {
        let key = (height, scale.to_bits(), look.font.clone(), look.font_size.to_bits());
        if let Some((_, g)) = self.geos.iter().find(|(k, _)| *k == key) {
            return *g;
        }
        let g = render::geo(height, scale, look);
        if self.geos.len() >= 8 {
            self.geos.clear(); // a drag zoom makes a new scale every frame
        }
        self.geos.push((key, g));
        g
    }

    /// The text column, px at `scale`: as wide as the longer line (the buttons right after it),
    /// up to `max_width` (plan step 9: Block glides the strip to it); always `max_width` with
    /// `fixed_width`. Measured each time with the call that draws it, so it fits exactly.
    fn text_column(&self, scale: f32, look: &Look) -> (i32, i32) {
        let max = (self.cfg.max_width * scale).round() as i32;
        if self.cfg.fixed_width {
            return (max, max);
        }
        let Some(c) = strip_content(self) else { return (max, max) };
        let w = render::text_width(&c.title, &c.artist, scale, look);
        (w.min(max), max)
    }

    /// A new snapshot from the media worker.
    fn on_media(&mut self, snap: Snapshot, ctx: &mut Ctx) {
        self.sessions = snap.sessions;
        // learn which apps report their position live (see learned.rs)
        let mut changed = false;
        for s in &self.sessions {
            changed |= self.learned.observe(&s.id, s.playing, &s.timeline);
        }
        if changed {
            self.learned.save();
            log("learned which apps report progress live: see progress.txt");
        }
        // remember live streams per track (see update_live)
        update_live(self, Instant::now());
        self.reselect(ctx);
    }

    /// Picks the session to show (the one playing, or a paused one until hide_after_pause_s):
    /// none, and the strip goes (`visible`).
    fn reselect(&mut self, ctx: &mut Ctx) {
        let list: Vec<(&str, bool)> = self.sessions.iter().map(|s| (s.id.as_str(), s.playing)).collect();
        let hide = Duration::from_secs(self.cfg.hide_after_pause_s as u64);
        let shown = self.selector.update(Instant::now(), &list, hide);
        if shown.is_some() != self.shown.is_some() {
            log(if shown.is_some() { "playing: strip shown" } else { "nothing playing: strip hidden" });
        }
        self.shown = shown;
        self.sync(ctx);
        ctx.redraw();
    }

    /// Follows playback: the pulse starts (from dim) when it starts and glides down on pause;
    /// the tint fades to its playing strength with it and back to its paused one on pause. Runs
    /// the pulse and tint timers exactly while needed.
    fn sync(&mut self, ctx: &mut Ctx) {
        let now = Instant::now();
        let playing = current(self).is_some_and(|s| s.playing);
        if playing != self.was_playing {
            if playing {
                self.pulse_since = Some(now);
                self.settle = None;
            } else {
                let level = self.pulse_since.map_or(0.0, |t| anim::pulse(t.elapsed().as_secs_f32()));
                self.settle = Some((level, now));
                self.pulse_since = None;
            }
            self.tint.go(if playing { 1.0 } else { 0.0 }, now, self.look.glide_ms);
            self.was_playing = playing;
        }
        let settling = self.settle.is_some_and(|(from, at)| anim::pulse_settle(from, at.elapsed().as_secs_f32()) > 0.0);
        let full_ring = current(self).is_some_and(|s| matches!(ring(self, s), Ring::Full { .. }));
        let beat = self.look.animations && full_ring && (playing || settling);
        if beat != std::mem::replace(&mut self.beating, beat) {
            if beat { ctx.every(TIMER_BEAT, 40) } else { ctx.stop(TIMER_BEAT) }
        }
        let tinting = self.tint.running(now);
        if tinting != std::mem::replace(&mut self.tinting, tinting) {
            if tinting { ctx.every(TIMER_TINT, 16) } else { ctx.stop(TIMER_TINT) }
        }
        // a line too long for the strip scrolls (as last drawn; the watch looks every second)
        let scroll = self.look.animations && self.shown.is_some() && self.scroll_long;
        if scroll != std::mem::replace(&mut self.scrolling, scroll) {
            if scroll { ctx.every(TIMER_SCROLL, 33) } else { ctx.stop(TIMER_SCROLL) }
        }
    }

    /// Every second: confirm live streams (a paused one sends no reports), reload an edited
    /// brand file, run the hide timer, move the ring along.
    fn watch(&mut self, ctx: &mut Ctx) {
        update_live(self, Instant::now());
        let mtime = brands_mtime();
        if self.brands_mtime != mtime {
            self.brands = brands::Brands::load();
            self.brands_mtime = mtime;
            logos::forget();
            ctx.redraw_panel();
        }
        self.reselect(ctx);
    }
}

impl Content for App {
    fn measure(&mut self, f: &Frame, compact: bool) -> i32 {
        let g = self.geo(f.height, f.scale, f.look);
        let (text_w, max_w) = self.text_column(f.scale, f.look);
        render::strip_layout(&g, compact, text_w, max_w, f.fit_width).width
    }

    fn draw(&mut self, cv: &mut Canvas, f: &Frame) {
        let g = self.geo(f.height, f.scale, f.look);
        let (text_w, max_w) = self.text_column(f.scale, f.look);
        let l = render::strip_layout(&g, f.compact, text_w, max_w, f.fit_width);
        self.scroll_long = false;
        if let Some(c) = strip_content(self) {
            // a line too long scrolls, from the start of each track
            let now = Instant::now();
            let key = (c.title.clone(), c.artist.clone());
            if self.scroll_since.as_ref().is_none_or(|(k, _)| *k != key) {
                self.scroll_since = Some((key, now));
            }
            let t = self.scroll_since.as_ref().map(|(_, t0)| now.duration_since(*t0).as_secs_f32());
            let w = cv.w;
            self.scroll_long = render::draw_strip(cv.pixels(), w, &c, &l, &g, f.look, self.cfg.live_rgb(), t.filter(|_| f.look.animations));
        }
        self.strip = Some((l, g));
        self.drawn_tint = strip_tint(self);
    }

    fn hit(&self, x: i32, y: i32) -> Hit {
        let Some((l, g)) = &self.strip else { return Hit::Strip };
        match l.hit(x, y) {
            Part::Button(i) => Hit::Button(i, draw::button_box(l.button(i), g.height, g.scale)),
            Part::Cover => Hit::Cover(render::ring_box(l, g)),
            Part::Body => Hit::Strip,
        }
    }

    fn cover(&self) -> Option<Box4> {
        self.strip.as_ref().map(|(l, g)| render::ring_box(l, g))
    }

    /// The brand colour behind the whole strip at the shared three strengths: quiet paused,
    /// active playing (fading between them), hover under the mouse. None (tint off, high
    /// contrast): Block's neutral highlight.
    fn tint(&self, area: Hit, _look: &Look) -> Option<Tint> {
        if area != Hit::Strip {
            return None; // a hovered button takes the strip's
        }
        self.drawn_tint
    }

    /// Something playing, or paused less than hide_after_pause_s ago.
    fn visible(&self) -> bool {
        self.shown.is_some()
    }

    fn start(&mut self, ctx: &mut Ctx) {
        self.main = ctx.main_window();
        let main = self.main as usize;
        logos::init(HWND(main as *mut c_void));
        self.media = Some(media::start(move |snap| {
            let p = Box::into_raw(Box::new(snap));
            unsafe {
                if PostMessageW(HWND(main as *mut c_void), WM_MEDIA, WPARAM(0), LPARAM(p as isize)).is_err() {
                    drop(Box::from_raw(p));
                }
            }
        }));
        ctx.every(TIMER_WATCH, 1000);
    }

    fn timer(&mut self, id: usize, ctx: &mut Ctx) {
        match id {
            TIMER_WATCH => self.watch(ctx),
            TIMER_BEAT | TIMER_TINT => {
                ctx.redraw();
                self.sync(ctx);
            }
            TIMER_PANEL => panel::frame(self, ctx),
            TIMER_SCROLL => ctx.redraw(),
            _ => {}
        }
    }

    fn button(&mut self, id: usize, _ctx: &mut Ctx) {
        if let Some(cmd) = button_cmd(self, id) {
            self.send(cmd);
        }
    }

    /// The cover opens the playing app's window.
    fn cover_click(&mut self, ctx: &mut Ctx) {
        if let Some(id) = current(self).map(|s| s.id.clone()) {
            ctx.after(move || apps::activate(&id));
        }
    }

    fn on_message(&mut self, msg: u32, _wp: usize, lp: isize, ctx: &mut Ctx) -> Option<isize> {
        match msg {
            WM_MEDIA => self.on_media(*unsafe { Box::from_raw(lp as *mut Snapshot) }, ctx),
            logos::WM_LOGO => {
                // a logo finished loading: draw it
                ctx.redraw();
                ctx.redraw_panel();
            }
            _ => return None,
        }
        Some(0)
    }

    fn panel_size(&mut self, kind: PanelKind, scale: f32) -> Option<(i32, i32)> {
        if kind != PanelKind::Main { return None; }
        Some(panel::size(self, scale))
    }

    fn panel_draw(&mut self, _kind: PanelKind, cv: &mut Canvas, f: &PanelFrame) {
        panel::draw(self, cv, f);
    }

    fn panel_mouse(&mut self, _kind: PanelKind, ev: PanelMouse, ctx: &mut Ctx) {
        panel::mouse(self, ev, ctx);
    }

    fn panel_shown(&mut self, _kind: PanelKind, open: bool, ctx: &mut Ctx) {
        panel::shown(self, open, ctx);
    }

    /// The track as the header; Open <app> and Copy "Artist – Title" (Block adds the rest).
    fn menu(&mut self, m: &mut MenuBuilder) {
        let Some(s) = current(self) else {
            m.title("Nothing playing");
            return;
        };
        let source = identity(self, s).source;
        let app = apps::display_name(&s.id);
        m.title(if s.title.is_empty() { app.clone() } else { s.title.clone() });
        m.subtitle([s.artist.as_str(), source.as_str()].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join(" · "));
        let copy = if s.artist.is_empty() { s.title.clone() } else { format!("{} – {}", s.artist, s.title) };
        m.item(Row::action(ID_OPEN, taskbar_dock::menu_panel::glyph::OPEN, &format!("Open {app}")));
        m.item(Row::action(ID_COPY, taskbar_dock::menu_panel::glyph::COPY, &format!("Copy \u{201C}{copy}\u{201D}")));
        m.sep();
    }

    fn menu_action(&mut self, id: usize, ctx: &mut Ctx) {
        let Some(s) = current(self) else { return };
        match id {
            ID_OPEN => {
                let id = s.id.clone();
                ctx.after(move || apps::activate(&id));
            }
            ID_COPY => {
                let text = if s.artist.is_empty() { s.title.clone() } else { format!("{} – {}", s.artist, s.title) };
                let main = self.main;
                ctx.after(move || copy_text(main, &text));
            }
            _ => {}
        }
    }

    /// settings.json changed (Edit settings…, another widget): ours again; the strip and panel
    /// follow (max_width, tint, click_panel_width).
    fn settings_changed(&mut self, ctx: &mut Ctx) {
        self.cfg = Config::load();
        self.reselect(ctx);
        ctx.resize_panel();
    }

    fn look_changed(&mut self, ctx: &mut Ctx) {
        self.look = Look::current();
        self.geos.clear();
        self.sync(ctx);
        ctx.redraw();
    }

    fn tray_icon(&mut self, cv: &mut Canvas, look: &Look) -> bool {
        let (w, h) = (cv.w, cv.h);
        render::tray_logo(cv.pixels(), w, h, look)
    }

    /// "Title – Artist", or "Nothing playing".
    fn tray_tip(&self) -> String {
        match current(self) {
            Some(s) if !s.artist.is_empty() => format!("{} – {}", s.title, s.artist),
            Some(s) => s.title.clone(),
            None => "Nothing playing".into(),
        }
    }
}

fn main() {
    unsafe { let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); }
    let mut spec = Spec::new("TaskbarMediaStrip", "media", "Now playing", log);
    spec.mutex = "Local\\TaskbarMedia.SingleInstance".into();
    // attaching fails, or no taskbar for 10 s: left of the tray and the docked widgets
    spec.float = Float::Fallback(FloatAt::BesideDock);
    // `--float`, or the test aid TASKBAR_MEDIA_FLOAT=1 (dev.ps1 passes TASKBAR_* variables on)
    spec.force_float = std::env::args().any(|a| a == "--float") || std::env::var_os("TASKBAR_MEDIA_FLOAT").is_some();
    // crowded: the text column narrows first
    spec.stretch = true;
    // the text column follows the track: Block glides the width (unless `fixed_width`)
    spec.glide_width = true;
    spec.log_file = Some(log_file());
    spec.defaults = Some(|| serde_json::to_value(Config::default()).unwrap_or_default());
    log("started");
    let app = App {
        main: 0,
        cfg: Config::load(),
        look: Look::current(),
        media: None,
        sessions: Vec::new(),
        selector: select::Selector::default(),
        shown: None,
        strip: None,
        drawn_tint: None,
        geos: Vec::new(),
        scroll_since: None,
        scroll_long: false,
        scrolling: false,
        pulse_since: None,
        settle: None,
        was_playing: false,
        beating: false,
        tinting: false,
        learned: learned::Learned::load(),
        live: std::collections::HashSet::new(),
        live_since: std::collections::HashMap::new(),
        played: std::collections::HashSet::new(),
        has_length: std::collections::HashSet::new(),
        brands: brands::Brands::load(),
        brands_mtime: brands_mtime(),
        tint: Appear::hidden(Instant::now()),
        panel: panel::State::default(),
    };
    block::run(spec, app);
}
