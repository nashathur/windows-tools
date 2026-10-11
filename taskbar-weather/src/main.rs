#![windows_subsystem = "windows"]

mod config;
mod knmi;
mod render;
mod weather;
mod fetch;
mod menu;
mod tray;

use config::Config;
use fetch::Fetched;
use render::LabelContent;
use std::ffi::c_void;
use taskbar_dock::block::{self, Canvas, Content, Ctx, Float, Frame, MenuBuilder, PanelFrame, PanelKind, Side, Spec};
use taskbar_dock::dock;
use taskbar_dock::style::Look;
use weather::{deg, describe, Location, Weather};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;

pub(crate) const WM_DATA: u32 = WM_APP + 2;
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
    error: Option<String>,
    fetching: bool,
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
                let (glyph, desc) = describe(wx.code, wx.is_day);
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

    fn timer(&mut self, id: usize, _ctx: &mut Ctx) {
        if id == TIMER_REFRESH {
            fetch::start_fetch(self);
        }
    }

    fn on_message(&mut self, msg: u32, _wp: usize, lp: isize, ctx: &mut Ctx) -> Option<isize> {
        match msg {
            WM_DATA => fetch::on_data(self, unsafe { Box::from_raw(lp as *mut Fetched) }, ctx),
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

fn main() {
    let mut spec = Spec::new("TaskbarWeatherLabel", "weather", "Weather", log);
    spec.mutex = "Local\\TaskbarWeather.SingleInstance".into();
    spec.float = Float::Off; // as before: nothing shows until the taskbar can be joined
    spec.log_file = Some(log_file());
    let app = App {
        main: 0,
        cfg: Config::load(),
        loc: None,
        station: None,
        weather: None,
        error: None,
        fetching: false,
    };
    block::run(spec, app);
}
