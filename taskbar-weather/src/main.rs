#![windows_subsystem = "windows"]

//! The weather on the Windows 11 taskbar, on the taskbar-dock library's Block: `App` is the
//! Content (label, flyout, menu rows, tray), plus the `Spec` and `main`.

mod config;
mod knmi;
mod render;
mod weather;
mod fetch;
mod menu;
mod tray;
mod commands;

use config::Config;
use fetch::Fetched;
use render::LabelContent;
use std::ffi::c_void;
use taskbar_dock::block::{self, Canvas, Content, Ctx, Float, Frame, MenuBuilder, PanelFrame, PanelKind, Side, Spec};
use taskbar_dock::dock;
use taskbar_dock::style::Look;
use weather::{deg, describe as weather_desc, Location, Place, Weather};
use std::time::{Instant, SystemTime};
use taskbar_dock::block::{CommandReply, ReplyToken};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;

pub(crate) const WM_DATA: u32 = WM_APP + 2;
pub(crate) const WM_PLACES: u32 = WM_APP + 3;
/// Content timer: the next fetch.
pub(crate) const TIMER_REFRESH: usize = 1;

/// The weather widget's state: Block's Content (the strip, the tray icon, the flyout).
pub(crate) struct App {
    /// Block's hidden main window: fetch results arrive there.
    main: isize,
    cfg: Config,
    loc: Option<(String, Location)>,
    station: Option<(String, knmi::Station)>,
    weather: Option<Weather>,
    /// `Config::location_key` the weather was fetched for (dockctl `now` shows it only for the
    /// current place).
    weather_for: String,
    error: Option<String>,
    fetching: bool,
    idle: bool,
    generation: u64,
    refetch: bool,
    fetched_at: Option<SystemTime>,
    place_reply: Option<PlaceReply>,
    places: Option<Places>,
}

/// A place change waiting for its matching forecast fetch to finish.
pub(crate) struct PlaceReply {
    pub generation: u64,
    /// `Config::location_key` of the place it saved: a later change of place fails it.
    pub location: String,
    pub token: ReplyToken,
    pub saved: serde_json::Value,
}

/// The latest place search, retained so `place <n>` can use its numbered results.
pub(crate) struct Places {
    pub at: Instant,
    pub list: Vec<Place>,
}

fn log_file() -> std::path::PathBuf {
    taskbar_dock::settings::dir().join("taskbar-weather.log")
}

/// Appends a line to taskbar-weather.log in the settings folder.
pub(crate) fn log(line: &str) {
    use std::io::Write;
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let path = log_file();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {line}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    }
}

pub(crate) fn hwnd(h: isize) -> HWND {
    HWND(h as *mut c_void)
}

/// The label sits on the right (stacked against the tray) rather than at the left edge.
pub(crate) fn on_right(cfg: &Config) -> bool {
    match cfg.label_position.as_str() {
        "left" => false,
        "right" => true,
        _ => unsafe { dock::left_aligned() },
    }
}

impl App {
    /// The label's content in full or compact mode.
    fn content(&self, compact: bool) -> LabelContent {
        let mut c = self.full_content();
        if compact {
            c.desc.clear(); // temperature over humidity only
            c.desc2.clear();
        }
        c
    }

    pub(crate) fn full_content(&self) -> LabelContent {
        let text = |line1: &str, line2: String, desc: &str| LabelContent { glyph: "☁".into(), line1: line1.into(), line2, desc: desc.into(), desc2: String::new() };
        match &self.weather {
            Some(wx) => {
                // temperature over humidity, the conditions beside them
                let (glyph, desc) = weather_desc(wx.code, wx.is_day);
                let line2 = if wx.humidity.is_finite() { format!("{:.0}%", wx.humidity) } else { String::new() };
                // the conditions over what it feels like
                let desc2 = if wx.feels.is_finite() { format!("Feels {}", deg(wx.feels)) } else { String::new() };
                LabelContent { glyph: glyph.into(), line1: format!("{}{}", deg(wx.temp), &wx.unit()[2..]), line2, desc: desc.into(), desc2 }
            }
            None if self.error.is_some() => text("--", String::new(), "Offline"),
            None => text("…", String::new(), "Weather"),
        }
    }
}

impl Content for App {
    fn tint(&self, area: block::Hit, look: &Look) -> Option<block::Tint> {
        if area != block::Hit::Strip { return None; }
        let wx = self.weather.as_ref()?;
        let (_, desc) = weather_desc(wx.code, wx.is_day);
        let key = match desc {
            "Clear" if wx.is_day => "weather.clear_day",
            "Clear" => "weather.clear_night",
            "Mostly clear" if wx.is_day => "weather.mostly_clear_day",
            "Mostly clear" => "weather.mostly_clear_night",
            "Partly cloudy" => "weather.partly_cloudy",
            "Cloudy" => "weather.cloudy",
            "Fog" => "weather.fog",
            "Drizzle" | "Light showers" => "weather.drizzle",
            "Light rain" | "Rain" | "Showers" => "weather.rain",
            "Heavy rain" | "Heavy showers" => "weather.heavy_rain",
            "Freezing drizzle" | "Freezing rain" => "weather.freezing",
            "Snow" | "Snow showers" => "weather.snow",
            "Thunderstorm" => "weather.thunderstorm",
            "Thunderstorm, hail" => "weather.hail",
            _ => return None,
        };
        let rgb = taskbar_dock::theme::brand(key, look.light).rgb24();
        Some(block::Tint::new(rgb, true))
    }

    fn commands(&self) -> Vec<(&'static str, &'static str)> {
        commands::commands()
    }

    fn command(&mut self, name: &str, args: &[String], token: ReplyToken, ctx: &mut Ctx) -> CommandReply {
        commands::command(self, name, args, token, ctx)
    }

    fn measure(&mut self, f: &Frame, compact: bool) -> i32 {
        render::measure_label(&self.content(compact), f.height, f.scale, f.look)
    }

    fn draw(&mut self, cv: &mut Canvas, f: &Frame) {
        render::draw_label(cv, &self.content(f.compact), f.height, f.scale, f.look);
    }

    fn side(&self) -> Side {
        if on_right(&self.cfg) { Side::Right } else { Side::Left }
    }

    fn start(&mut self, ctx: &mut Ctx) {
        self.main = ctx.main_window();
        fetch::start_fetch(self);
    }

    fn idle(&mut self, idle: bool, _ctx: &mut Ctx) {
        self.idle = idle;
        if !idle { fetch::start_fetch(self); }
    }

    fn timer(&mut self, id: usize, _ctx: &mut Ctx) {
        if id == TIMER_REFRESH {
            fetch::start_fetch(self);
        }
    }

    fn on_message(&mut self, msg: u32, _wp: usize, lp: isize, ctx: &mut Ctx) -> Option<isize> {
        match msg {
            WM_DATA => fetch::on_data(self, unsafe { Box::from_raw(lp as *mut Fetched) }, ctx),
            WM_PLACES => commands::on_places(self, unsafe { Box::from_raw(lp as *mut commands::PlacesResult) }),
            _ => return None,
        }
        Some(0)
    }

    fn menu(&mut self, m: &mut MenuBuilder) {
        menu::build(self, m);
    }

    fn menu_switch(&mut self, id: usize, ctx: &mut Ctx) -> bool {
        menu::switch(self, id, ctx)
    }

    fn menu_action(&mut self, id: usize, ctx: &mut Ctx) {
        menu::action(self, id, ctx);
    }

    fn settings_changed(&mut self, ctx: &mut Ctx) {
        menu::settings_changed(self, ctx);
    }

    fn panel_size(&mut self, kind: PanelKind, scale: f32) -> Option<(i32, i32)> {
        (kind == PanelKind::Main).then_some(((render::POPUP_W as f32 * scale) as i32, (render::POPUP_H as f32 * scale) as i32))
    }

    fn panel_draw(&mut self, _kind: PanelKind, cv: &mut Canvas, f: &PanelFrame) {
        let dc = windows::Win32::Graphics::Gdi::HDC(cv.dc as _);
        render::paint_panel(dc, f.w, f.h, self.weather.as_ref(), self.error.as_deref(), f.scale, f.look);
    }

    fn tray_icon(&mut self, cv: &mut Canvas, look: &Look) -> bool {
        render::tray_logo(cv, look)
    }

    fn tray_tip(&self) -> String {
        tray::tip(self)
    }
}

const WIDGET_CLASS: &str = "TaskbarWeatherLabel";
const WIDGET_SECTION: &str = "weather";
const WIDGET_TITLE: &str = "Weather";
const WIDGET_DESCRIPTION: &str = "Weather and temperature for your location";
/// Returns the weather widget's stored defaults.
fn defaults() -> serde_json::Value {
    serde_json::to_value(config::Config::default()).unwrap_or_default()
}
/// Describes the weather widget without starting its runtime.
fn describe() -> block::Describe {
    block::describe(
        WIDGET_SECTION,
        WIDGET_CLASS,
        WIDGET_TITLE,
        WIDGET_DESCRIPTION,
        Some(config::settings),
        Some(defaults),
        Some(log_file()),
    )
}

fn main() {
    block::describe_if_asked(describe);
    let mut spec = Spec::new(WIDGET_CLASS, WIDGET_SECTION, WIDGET_TITLE, log);
    spec.description = WIDGET_DESCRIPTION;
    spec.settings = Some(config::settings);
    spec.mutex = "Local\\TaskbarWeather.SingleInstance".into();
    spec.float = Float::Off; // as before: nothing shows until the taskbar can be joined
    spec.log_file = Some(log_file());
    spec.defaults = Some(defaults);
    let app = App {
        main: 0,
        cfg: Config::load(),
        loc: None,
        station: None,
        weather: None,
        weather_for: String::new(),
        error: None,
        fetching: false,
        idle: false,
        generation: 0,
        refetch: false,
        fetched_at: None,
        place_reply: None,
        places: None,
    };
    block::run(spec, app);
}

#[cfg(test)]
mod describe_tests {
    use super::*;

    #[test]
    /// Checks the description against its spec constants and defaults.
    fn description_matches_defaults_and_spec() {
        let description = describe();
        assert_eq!(description.section, WIDGET_SECTION);
        assert_eq!(description.class, WIDGET_CLASS);
        taskbar_dock::describe::validate(&description).unwrap();
    }
}
