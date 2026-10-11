//! Taskbar bus-arrival widget for one stop and line (default: U-OV line 3, Rhônedreef → Utrecht CS).
//!
//! Hover or left-click: next departures · drag: reorder in the dock · right-click: menu.
//! The strip window, docking, panels, right-click panel, tray icon and floating fallback are the
//! shared library's Block (C:\Users\thurs\tools\taskbar-dock\src\block); this file is its Content.
//! Settings: the "bus" section of %APPDATA%\TaskbarDock\settings.json.
//! Data lives in .\data next to the exe (schedule cache, timetable, line colours, pos.txt, log).
#![windows_subsystem = "windows"]

mod bar;
mod data;
mod picker;
mod popup;
mod worker;

use bar::*;
use worker::*;

use std::cell::RefCell;
use std::mem::zeroed;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::{fs, thread, time::Duration};

use taskbar_dock::block::{self, Canvas, Content, Ctx, Float, FloatAt, Frame, MenuBuilder, PanelFrame, PanelKey, PanelKind, PanelMouse, Spec};
use taskbar_dock::menu_panel::{glyph, Row};
use taskbar_dock::style::Look;
use taskbar_dock::theme::{self, Theme};
use taskbar_dock::{badge, draw, wide};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

const POLL_S: i64 = 60; // live update interval (one ~6 KB request)
const LIVE_WINDOW_MIN: i64 = 90; // only fetch live data when a bus is due within this many minutes
const LIVE_MAX_AGE_S: i64 = 600; // older live data is ignored (its expected times would be stale)
const SHOW_IN_BAR: usize = 3;
const SHOW_IN_POPUP: usize = 8;

/// The worker has new data (posted to Block's main window).
const WM_DATA: u32 = WM_APP + 2;
/// Content timer: once a second, the minute countdown and anything else that changed.
const TIMER_TICK: usize = 1;

// the right-click panel's own rows
const ID_REFRESH: usize = 1;
const ID_TIMETABLE: usize = 2;
const ID_STOP: usize = 4;

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
/// Turquoise: a line with no colour in settings, table or timetable (COLORREF).
fn badge_fallback() -> u32 {
    theme::brand("bus.badge_fallback", true).colorref()
}
/// White coverage: strip text is drawn white, then coloured.
const WHITE: u32 = rgb(255, 255, 255);

fn hex_color(s: &str) -> Option<u32> {
    let s = s.trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).ok().filter(|_| s.len() == 6)?;
    Some(rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Line badge (background, text): badge_color/badge_text from the "bus" section of the shared
/// settings.json, else data\line_colors.txt, else the operator's line colour from GTFS, else
/// turquoise. Edits to line_colors.txt apply live (checked each time, reloaded when it changed).
fn badge_colors() -> (u32, u32) {
    thread_local!(static TABLE: RefCell<(Option<std::time::SystemTime>, Vec<data::LineColor>)> = RefCell::new((None, Vec::new())));
    let path = data_dir().join("line_colors.txt");
    let mtime = fs::metadata(&path).and_then(|m| m.modified()).ok();
    let table = TABLE.with(|t| {
        let mut t = t.borrow_mut();
        if t.0 != mtime || (mtime.is_some() && t.1.is_empty()) {
            *t = (mtime, data::load_line_colors(data_dir()));
        }
        t.1.clone()
    });
    let table = &table;
    let (cfg, sched, ..) = current();
    let route = sched.as_ref().map_or(("", ""), |s| (s.color.as_str(), s.text_color.as_str()));
    // the settings' own badge colour wins; its text is given, or the contrast to that colour
    if let Some(bg) = hex_color(&cfg.badge_color) {
        return (bg, hex_color(&cfg.badge_text).unwrap_or_else(|| badge::contrast_text(bg)));
    }
    let (bg, fg) = line_badge_colors_for(table, &cfg.agency, &cfg.line, route.0, route.1);
    (bg, hex_color(&cfg.badge_text).unwrap_or(fg))
}

/// A line's badge colours: line_colors.txt (an entry with a valid colour), else the route's GTFS
/// colour, else turquoise; the text colour given, or the contrast to the badge.
fn line_badge_colors_for(table: &[data::LineColor], agency: &str, line: &str, route_bg: &str, route_fg: &str) -> (u32, u32) {
    let (bg, fg) = data::find_line_color(table, agency, line)
        .filter(|c| hex_color(&c.bg).is_some())
        .map_or((hex_color(route_bg), hex_color(route_fg)), |c| (hex_color(&c.bg), hex_color(&c.fg)));
    let bg = bg.unwrap_or_else(badge_fallback);
    (bg, fg.unwrap_or_else(|| badge::contrast_text(bg)))
}

pub(crate) fn line_badge_color(agency: &str, line: &str, route_bg: &str, route_fg: &str) -> (theme::Rgb, theme::Rgb) {
    thread_local!(static TABLE: RefCell<(Option<std::time::SystemTime>, Vec<data::LineColor>)> = RefCell::new((None, Vec::new())));
    let path = data_dir().join("line_colors.txt");
    let mtime = fs::metadata(&path).and_then(|m| m.modified()).ok();
    let table = TABLE.with(|t| {
        let mut t = t.borrow_mut();
        if t.0 != mtime || (mtime.is_some() && t.1.is_empty()) { *t = (mtime, data::load_line_colors(data_dir())); }
        t.1.clone()
    });
    let (bg, fg) = line_badge_colors_for(&table, agency, line, route_bg, route_fg);
    (theme::Rgb((bg & 255) as u8, ((bg >> 8) & 255) as u8, ((bg >> 16) & 255) as u8),
     theme::Rgb((fg & 255) as u8, ((fg >> 8) & 255) as u8, ((fg >> 16) & 255) as u8))
}

fn log(m: &str) {
    data::log(data_dir(), m);
}

/// The bus widget: Block's Content. The data lives in the worker's `Shared`; this keeps what
/// was last shown, to redraw only on a change.
struct App {
    /// The look as last drawn (for the tooltip's and the panel's clock format).
    look: Look,
    /// The strip texts, badge and panel content as last shown.
    shown: String,
    /// The tray logo as last drawn: line and badge colours.
    logo: String,
    picker: picker::Picker,
}

impl App {
    /// Something to show changed (a minute passed, new data, another stop or colour): redraw
    /// what did.
    fn refresh(&mut self, ctx: &mut Ctx) {
        let cfg = current().0;
        let logo = format!("{}|{:?}", cfg.line, badge_colors());
        let shown = format!("{}|{}|{}|{logo}|{:?}", bar_text_for(false), bar_text_for(true), cfg.label, popup::popup_content(&self.look));
        if shown != self.shown {
            self.shown = shown;
            ctx.redraw();
            ctx.resize_panel(); // nothing if it isn't open; a row more or less resizes it
        }
        if logo != self.logo {
            self.logo = logo;
            ctx.redraw_tray();
        }
    }
}

impl Content for App {
    fn measure(&mut self, f: &Frame, compact: bool) -> i32 {
        bar::measure(f, compact)
    }

    fn draw(&mut self, cv: &mut Canvas, f: &Frame) {
        self.look = f.look.clone();
        bar::draw(cv, f);
    }

    fn start(&mut self, ctx: &mut Ctx) {
        MAIN.store(ctx.main_window(), Relaxed);
        thread::spawn(worker);
        ctx.every(TIMER_TICK, 1000);
    }

    fn timer(&mut self, id: usize, ctx: &mut Ctx) {
        if id == TIMER_TICK {
            self.refresh(ctx);
        }
        self.picker.timer(id, ctx);
    }

    fn on_message(&mut self, msg: u32, _wp: usize, lp: isize, ctx: &mut Ctx) -> Option<isize> {
        if msg == picker::WM_PICKER {
            if lp != 0 { let posted = unsafe { *Box::from_raw(lp as *mut picker::Posted) }; self.picker.accept_result(posted.generation, posted.data, ctx); }
            return Some(0);
        }
        (msg == WM_DATA).then(|| { self.refresh(ctx); 0 })
    }

    fn panel_size(&mut self, kind: PanelKind, scale: f32) -> Option<(i32, i32)> {
        Some(match kind { PanelKind::Main => popup::panel_size(&self.look, scale), PanelKind::Picker => ((theme::size::PANEL_WIDE*scale).round() as i32,(theme::size::PANEL_PICKER_HEIGHT*scale).round() as i32) })
    }

    fn panel_draw(&mut self, kind: PanelKind, cv: &mut Canvas, f: &PanelFrame) {
        self.look = f.look.clone();
        match kind { PanelKind::Main => popup::panel_draw(cv.dc, f.look, f.scale, f.w), PanelKind::Picker => self.picker.draw(cv,f) }
    }

    fn panel_mouse(&mut self, kind: PanelKind, ev: PanelMouse, ctx: &mut Ctx) {
        if kind == PanelKind::Picker { self.picker.mouse(ev,ctx); return; }
        // a click on the departures closes them (as before the port)
        if let PanelMouse::Up(..) = ev {
            ctx.close_panel();
        }
    }

    fn panel_shown(&mut self, kind: PanelKind, open: bool, ctx: &mut Ctx) { if kind == PanelKind::Picker { self.picker.shown(open,ctx); } }
    fn panel_key(&mut self, kind: PanelKind, key: PanelKey, ctx: &mut Ctx) -> bool { kind == PanelKind::Picker && self.picker.key(key,ctx) }
    fn panel_char(&mut self, kind: PanelKind, text: &str, ctx: &mut Ctx) { if kind == PanelKind::Picker { self.picker.character(text,ctx); } }
    fn panel_wheel(&mut self, kind: PanelKind, delta: i32, ctx: &mut Ctx) { if kind == PanelKind::Picker { self.picker.wheel(delta,ctx); } }
    fn panel_state(&self, kind: PanelKind) -> serde_json::Value { if kind == PanelKind::Picker { self.picker.state() } else { serde_json::Value::Null } }

    fn menu(&mut self, m: &mut MenuBuilder) {
        let cfg = current().0;
        m.title(cfg.label.clone()).subtitle(format!("Line {} · {}", cfg.line, bar_text_for(false)));
        m.item(Row::action(ID_STOP, glyph::PIN, "Change stop…"))
            .item(Row::action(ID_REFRESH, glyph::REFRESH, "Refresh live times"))
            .item(Row::action(ID_TIMETABLE, glyph::CALENDAR, "Update timetable"));
    }

    fn menu_action(&mut self, id: usize, ctx: &mut Ctx) {
        match id {
            ID_REFRESH => FORCE_RT.store(true, Relaxed),
            ID_TIMETABLE => FORCE_TT.store(true, Relaxed),
            ID_STOP => ctx.open_panel_kind(PanelKind::Picker),
            _ => {}
        }
    }

    fn settings_changed(&mut self, ctx: &mut Ctx) {
        // Edit settings…, or another widget saving its section: label, stop, colours
        apply_config(data::Config::load(data_dir()));
        self.refresh(ctx);
    }

    fn look_changed(&mut self, ctx: &mut Ctx) {
        self.shown.clear(); // the panel's clock format may have changed
        self.refresh(ctx);
    }

    fn tray_icon(&mut self, cv: &mut Canvas, look: &Look) -> bool {
        bar::tray_logo(cv, look)
    }

    fn tray_tip(&self) -> String {
        let cfg = current().0;
        let tip = format!("{} · line {}: {}", cfg.label, cfg.line, bar_text_for(false));
        tip.chars().take(127).collect()
    }
}

fn main() {
    let mut spec = Spec::new("BusWidgetBar", "bus", "Bus widget", log);
    spec.mutex = "Local\\BusWidgetRhonedreef".into();
    // no taskbar to join: a topmost strip at the x it was last dropped at
    spec.float = Float::Fallback(FloatAt::Saved(|| data_dir().join("pos.txt")));
    spec.log_file = Some(data_dir().join("buswidget.log"));
    spec.defaults = Some(|| data::Config::default().to_json());
    block::run(spec, App { look: Look::current(), shown: String::new(), logo: String::new(), picker: picker::Picker::default() });
}
