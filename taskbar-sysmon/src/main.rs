#![windows_subsystem = "windows"]
mod config;
mod format;
mod history;
mod mem;
mod net;
mod nvml;
mod pdh;
mod render;
mod sampler;
mod temp;
use config::Config;
use sampler::{Shared, Snapshot, Worker, WM_SAMPLE};
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use taskbar_dock::{
    block::{
        self, Canvas, CommandError, CommandReply, Content, Ctx, Frame, MenuBuilder, PanelFrame,
        PanelKind, PanelMouse, ReplyToken, Spec,
    },
    menu_panel::{Item, Row},
    settings,
    style::Look,
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::HWND,
        UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    },
};
const CLASS: &str = "TaskbarSysmonStrip";
const SECTION: &str = "sysmon";
const TITLE: &str = "System monitor";
const ID_OPEN: usize = 1;
const ID_RESET: usize = 2;
const ID_SPEED: usize = 3;
const PANEL_RESET: usize = 30;
pub(crate) fn log(line: &str) {
    taskbar_dock::log::append(&taskbar_dock::log::path("taskbar-sysmon"), line)
}
fn defaults() -> serde_json::Value {
    Config::defaults()
}
fn describe() -> block::Describe {
    block::describe(
        SECTION,
        CLASS,
        TITLE,
        "CPU, GPU, memory and network use in the taskbar",
        Some(config::descriptions),
        Some(defaults),
        Some(taskbar_dock::log::path("taskbar-sysmon")),
    )
}
pub(crate) struct App {
    main: isize,
    cfg: Config,
    shared: Shared,
    worker: Option<Worker>,
    include_virtual: Arc<AtomicBool>,
    paused: bool,
    idle: bool,
    panel_shown: bool,
    panel_scale: f32,
    drawn_tint_level: Option<u8>,
    fake: bool,
    hover: Option<usize>,
    pressed: Option<usize>,
}
impl App {
    fn sync(&self) {
        if let Some(w) = &self.worker {
            w.set(self.cfg.speed.clone(), self.paused, self.idle)
        }
    }
    fn current(&self) -> Snapshot {
        self.fake_snapshot(self.shared.snapshot())
    }
    fn fake_snapshot(&self, mut s: Snapshot) -> Snapshot {
        if self.fake {
            s.cpu = Some(99.);
            s.gpu = Some(87.);
            s.ram = Some(97.);
            s.down = Some(1_200_000.);
            s.up = Some(80_000.);
            s.cpu_temp = Some(93.);
            s.gpu_temp = Some(85.);
        }
        s
    }
    fn tint_level(&self) -> u8 {
        let s = self.current();
        [
            format::level(s.cpu, self.cfg.warn_pct, self.cfg.crit_pct),
            format::level(s.gpu, self.cfg.warn_pct, self.cfg.crit_pct),
            if s.gpu_temp.is_some_and(|v| v > self.cfg.gpu_temp_warn) {
                1
            } else {
                0
            },
            format::level(s.ram, self.cfg.warn_pct, self.cfg.crit_pct),
            format::level(s.cpu_temp, self.cfg.cpu_temp_warn, self.cfg.cpu_temp_crit),
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
    }
    fn reset(&self) {
        self.shared.clear()
    }
    fn speed_menu_item(&self) -> Item {
        let selected = if self.paused {
            3
        } else {
            match self.cfg.speed.as_str() {
                "high" => 0,
                "low" => 2,
                _ => 1,
            }
        };
        Row::choice(
            ID_SPEED,
            "Speed",
            &["High", "Normal", "Low", "Paused"],
            selected,
        )
    }
    fn set_speed_option(&mut self, option: usize) -> usize {
        match option {
            0 => {
                self.cfg.speed = "high".into();
                self.paused = false;
                self.cfg.save("speed", serde_json::json!("high"));
            }
            1 => {
                self.cfg.speed = "normal".into();
                self.paused = false;
                self.cfg.save("speed", serde_json::json!("normal"));
            }
            2 => {
                self.cfg.speed = "low".into();
                self.paused = false;
                self.cfg.save("speed", serde_json::json!("low"));
            }
            3 => self.paused = true,
            _ => {
                return if self.paused {
                    3
                } else {
                    match self.cfg.speed.as_str() {
                        "high" => 0,
                        "low" => 2,
                        _ => 1,
                    }
                }
            }
        }
        self.sync();
        option
    }
    fn set_range(&mut self, r: &str) {
        if ["1m", "5m", "1h"].contains(&r) {
            self.cfg.range = r.into();
            self.cfg.save("range", serde_json::json!(r));
        }
    }
    fn open_taskmgr() {
        unsafe {
            let exe: Vec<u16> = "taskmgr.exe".encode_utf16().chain(Some(0)).collect();
            let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
            let _ = ShellExecuteW(
                HWND(0 as *mut c_void),
                PCWSTR(verb.as_ptr()),
                PCWSTR(exe.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
    }
    fn now_json(&self) -> serde_json::Value {
        let (snapshot, len) = {
            let d = self.shared.data.lock().unwrap();
            (d.latest.clone(), d.history.points.len())
        };
        self.fake_snapshot(snapshot)
            .json(&self.cfg.speed, self.paused, self.idle, len)
    }
    fn panel_action(&mut self, id: usize, ctx: &mut Ctx) {
        match id {
            10 => self.set_range("1m"),
            11 => self.set_range("5m"),
            12 => self.set_range("1h"),
            20 => {
                self.cfg.speed = "high".into();
                self.paused = false;
                self.cfg.save("speed", serde_json::json!("high"));
                self.sync()
            }
            21 => {
                self.cfg.speed = "normal".into();
                self.paused = false;
                self.cfg.save("speed", serde_json::json!("normal"));
                self.sync()
            }
            22 => {
                self.cfg.speed = "low".into();
                self.paused = false;
                self.cfg.save("speed", serde_json::json!("low"));
                self.sync()
            }
            23 => {
                self.paused = !self.paused;
                self.sync()
            }
            PANEL_RESET | ID_RESET => {
                self.reset();
                ctx.redraw();
            }
            31 | ID_OPEN => ctx.after(Self::open_taskmgr),
            _ => return,
        }
        ctx.redraw_panel();
    }
}
impl Content for App {
    fn commands(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("now", "now: return current readings as JSON"),
            ("fake", "fake on|off: show synthetic threshold spikes"),
            ("reset", "reset: clear graphs and network session totals"),
        ]
    }
    fn command(
        &mut self,
        name: &str,
        args: &[String],
        token: ReplyToken,
        ctx: &mut Ctx,
    ) -> CommandReply {
        let result = match (name, args) {
            ("now", []) => Ok(self.now_json()),
            ("fake", [v]) if v == "on" || v == "off" => {
                self.fake = v == "on";
                if self.fake {
                    self.shared.fake_spike();
                }
                ctx.redraw();
                ctx.redraw_panel();
                Ok(serde_json::json!({"fake":self.fake}))
            }
            ("reset", []) => {
                self.reset();
                ctx.redraw();
                ctx.redraw_panel();
                Ok(serde_json::json!({"reset":true,"speed":self.cfg.speed}))
            }
            ("fake", _) => Err("usage: fake on|off".to_owned()),
            ("now", _) => Err("usage: now".to_owned()),
            ("reset", _) => Err("usage: reset".to_owned()),
            _ => Err("unknown command".to_owned()),
        };
        drop(token);
        CommandReply::Done(result.map_err(|message| CommandError {
            usage: true,
            message,
        }))
    }
    fn measure(&mut self, f: &Frame, compact: bool) -> i32 {
        render::measure(f, compact, self.cfg.max_width)
    }
    fn draw(&mut self, cv: &mut Canvas, f: &Frame) {
        let s = self.current();
        let d = self.shared.data.lock().unwrap();
        render::strip(cv, f, &s, &d.history, &self.cfg, self.paused);
    }
    fn tint(&self, _area: block::Hit, look: &Look) -> Option<block::Tint> {
        let level = self.tint_level();
        if level == 0 {
            return None;
        }
        Some(block::Tint::new(
            if level == 2 {
                look_color(look, true)
            } else {
                look_color(look, false)
            },
            true,
        ))
    }
    fn start(&mut self, ctx: &mut Ctx) {
        let _ = settings::fill_missing(SECTION, &defaults());
        self.cfg = Config::load();
        self.main = ctx.main_window();
        self.include_virtual
            .store(self.cfg.include_virtual, Ordering::Relaxed);
        let hwnd = self.main;
        self.shared
            .set(self.cfg.speed.clone(), self.paused, self.idle);
        self.worker = Some(Worker::start(
            self.shared.clone(),
            hwnd,
            self.include_virtual.clone(),
        ));
        self.sync();
    }
    fn idle(&mut self, idle: bool, ctx: &mut Ctx) {
        self.idle = idle;
        self.sync();
        if !idle {
            ctx.redraw();
            if self.panel_shown {
                ctx.redraw_panel();
            }
        }
    }
    fn on_message(&mut self, msg: u32, wp: usize, _lp: isize, ctx: &mut Ctx) -> Option<isize> {
        if msg == WM_SAMPLE {
            let level = self.tint_level();
            let threshold_changed = self.drawn_tint_level != Some(level);
            self.drawn_tint_level = Some(level);
            if wp != 0 || threshold_changed {
                ctx.redraw();
            }
            if self.panel_shown {
                ctx.redraw_panel();
            }
            Some(0)
        } else {
            None
        }
    }
    fn closing(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
    }
    fn panel_size(&mut self, k: PanelKind, s: f32) -> Option<(i32, i32)> {
        (k == PanelKind::Main).then(|| render::panel_size(s))
    }
    fn panel_draw(&mut self, _k: PanelKind, cv: &mut Canvas, f: &PanelFrame) {
        self.panel_scale = f.scale;
        let s = self.current();
        let d = self.shared.data.lock().unwrap();
        render::panel(
            cv,
            f,
            &s,
            &d.history,
            &self.cfg,
            &self.cfg.range,
            self.paused,
            self.hover,
            self.pressed,
            f.look,
        );
    }
    fn panel_mouse(&mut self, _k: PanelKind, ev: PanelMouse, ctx: &mut Ctx) {
        let scale = self.panel_scale;
        match ev {
            PanelMouse::Move(x, y) => {
                let id = self.panel_hit(x, y, scale);
                if self.hover != id {
                    self.hover = id;
                    ctx.redraw_panel()
                }
            }
            PanelMouse::Down(x, y) => {
                self.pressed = self.panel_hit(x, y, scale);
                ctx.redraw_panel()
            }
            PanelMouse::Up(x, y) => {
                let hit = self.panel_hit(x, y, scale);
                let act = if hit == self.pressed { hit } else { None };
                self.pressed = None;
                if let Some(id) = act {
                    self.panel_action(id, ctx)
                } else {
                    ctx.redraw_panel()
                }
            }
            PanelMouse::Leave => {
                self.hover = None;
                ctx.redraw_panel()
            }
            PanelMouse::Cancel => {
                self.pressed = None;
                ctx.redraw_panel()
            }
            PanelMouse::DoubleClick(x, y) => {
                if let Some(id) = self.panel_hit(x, y, scale) {
                    self.panel_action(id, ctx)
                }
            }
        }
    }
    fn panel_shown(&mut self, k: PanelKind, open: bool, _ctx: &mut Ctx) {
        if k == PanelKind::Main {
            self.panel_shown = open;
            self.hover = None;
            self.pressed = None;
            self.sync();
        }
    }
    fn panel_state(&self, k: PanelKind) -> serde_json::Value {
        if k == PanelKind::Main {
            serde_json::json!({"range":self.cfg.range,"speed":self.cfg.speed,"paused":self.paused,"fake":self.fake})
        } else {
            serde_json::Value::Null
        }
    }
    fn menu(&mut self, m: &mut MenuBuilder) {
        m.item(Row::action(
            ID_OPEN,
            taskbar_dock::menu_panel::glyph::OPEN,
            "Open Task Manager",
        ));
        m.item(Row::action(
            ID_RESET,
            taskbar_dock::menu_panel::glyph::REFRESH,
            "Reset graphs",
        ));
        m.item(self.speed_menu_item());
        m.sep();
    }
    fn menu_action(&mut self, id: usize, ctx: &mut Ctx) {
        match id {
            ID_OPEN => ctx.after(Self::open_taskmgr),
            ID_RESET => {
                self.reset();
                ctx.redraw();
                if self.panel_shown {
                    ctx.redraw_panel()
                }
            }
            _ => {}
        }
    }
    fn menu_choice(&mut self, id: usize, option: usize, ctx: &mut Ctx) -> usize {
        if id != ID_SPEED {
            return option;
        }
        let selected = self.set_speed_option(option);
        ctx.redraw();
        ctx.redraw_panel();
        selected
    }
    fn settings_changed(&mut self, ctx: &mut Ctx) {
        self.cfg = Config::load();
        self.include_virtual
            .store(self.cfg.include_virtual, Ordering::Relaxed);
        self.sync();
        ctx.redraw();
        if self.panel_shown {
            ctx.redraw_panel()
        }
    }
    fn tray_icon(&mut self, cv: &mut Canvas, look: &Look) -> bool {
        render::tray(cv, look)
    }
    fn tray_tip(&self) -> String {
        let s = self.current();
        if self.paused {
            return "System monitor: paused".into();
        }
        format!(
            "System monitor: CPU {} · GPU {} · RAM {} · ↓ {}",
            format::pct(s.cpu),
            format::pct(s.gpu),
            format::pct(s.ram),
            format::rate(s.down)
        )
    }
}
fn look_color(look: &Look, danger: bool) -> u32 {
    let t = taskbar_dock::theme::Theme::new(look);
    if danger {
        t.status.danger.rgb24()
    } else {
        t.status.warn.rgb24()
    }
}
impl App {
    fn panel_hit(&self, x: i32, y: i32, scale: f32) -> Option<usize> {
        render::panel_control_hit(&Look::current(), scale, x, y)
    }
}
fn main() {
    block::describe_if_asked(describe);
    let mut spec = Spec::new(CLASS, SECTION, TITLE, log);
    spec.description = "CPU, GPU, memory and network use in the taskbar";
    spec.settings = Some(config::descriptions);
    spec.defaults = Some(defaults);
    spec.log_file = Some(taskbar_dock::log::path("taskbar-sysmon"));
    spec.compact_when_crowded = true;
    let cfg = Config::load();
    let app = App {
        main: 0,
        include_virtual: Arc::new(AtomicBool::new(cfg.include_virtual)),
        cfg,
        shared: Shared::new(),
        worker: None,
        paused: false,
        idle: false,
        panel_shown: false,
        panel_scale: 1.,
        drawn_tint_level: None,
        fake: false,
        hover: None,
        pressed: None,
    };
    block::run(spec, app)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paused_speed_is_not_persisted() {
        let cfg = Config::default();
        assert_eq!(cfg.speed, "normal");
        let mut app = App {
            main: 0,
            cfg,
            shared: Shared::new(),
            worker: None,
            include_virtual: Arc::new(AtomicBool::new(false)),
            paused: true,
            idle: false,
            panel_shown: false,
            panel_scale: 1.,
            drawn_tint_level: None,
            fake: false,
            hover: None,
            pressed: None,
        };
        app.cfg.speed = "low".into();
        app.paused = false;
        app.set_speed_option(3);
        assert_eq!(app.cfg.speed, "low");
        assert!(app.paused);
        app.set_speed_option(0);
        assert_eq!(app.cfg.speed, "high");
        assert!(!app.paused);
    }
}
