#![windows_subsystem = "windows"]

//! Discord voice chat on the Windows 11 taskbar, on the taskbar-dock library's Block: `App` is
//! the Content (strip, panel, menu rows, tray logo), plus the `Spec` and `main`.

mod auth;
mod avatars;
mod config;
mod commands;
mod menu;
mod open;
mod render;
mod rpc;
mod voice;

use config::Config;
use render::{Button, Ring, StripContent};
use std::ffi::c_void;
use std::sync::mpsc::Sender;
use std::time::Instant;
use taskbar_dock::block::{self, Box4, Canvas, CommandReply, Content, Ctx, Float, Frame, Hit, MenuBuilder, PanelFrame, PanelKind, PanelMouse, ReplyToken, Spec, Tint};
use taskbar_dock::style::Look;
use voice::{Status, Voice};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// A new `Voice` from the RPC worker (lparam: Box<Voice>).
const WM_VOICE: u32 = WM_APP + 2;
/// An avatar or server logo finished loading.
const WM_AVATAR: u32 = WM_APP + 3;
/// Content timer: the logo's ring pulsing (25 frames a second while unmuted in a call).
const TIMER_BEAT: usize = 1;

/// The Discord widget's state: Block's Content (the strip, the panel, the tray icon).
pub(crate) struct App {
    /// Block's hidden main window: the RPC worker and the avatar loader post there.
    main: isize,
    cfg: Config,
    /// The call as last reported by the RPC worker.
    voice: Voice,
    /// Who has the strip's avatar places.
    slots: voice::Slots,
    rpc: Option<Sender<rpc::Cmd>>,
    /// The strip as last drawn (its buttons and logo, for hit tests).
    strip: Option<render::StripLayout>,
    /// Since when the ring has been pulsing (from dim), and whether its timer runs.
    ring_since: Option<Instant>,
    beating: bool,
    /// Windows' animations are on (the ring pulses only then).
    animations: bool,
    /// The media strip's text column (px at 100%): this strip is as wide as that one.
    media_w: f32,
    /// The panel's button under the mouse, and the scale the panel was sized at.
    panel_hover: Option<Button>,
    panel_scale: f32,
}

fn log_file() -> std::path::PathBuf {
    taskbar_dock::settings::dir().join("taskbar-discord.log")
}

/// Appends a line to taskbar-discord.log in the settings folder. Never tokens or secrets.
pub(crate) fn log(line: &str) {
    use std::io::Write;
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    // one write per line: the RPC worker logs too, and appends of whole lines don't interleave
    let text = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} {line}\r\n", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_file()) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// The media strip's text column, px at 100% (its `max_width`): our strip is as wide as its own.
pub(crate) fn media_text_width() -> f32 {
    taskbar_dock::settings::section("media")
        .and_then(|m| m.get("max_width").and_then(|v| v.as_f64()))
        .filter(|w| *w > 0.0)
        .map_or(136.0, |w| w as f32)
}

/// Tells the UI thread an avatar is ready (from the loader thread).
fn notify_avatar(main: usize) {
    unsafe { let _ = PostMessageW(HWND(main as *mut c_void), WM_AVATAR, WPARAM(0), LPARAM(0)); }
}

/// The server's logo if it has one and it's loaded (else starts the download; `main` gets
/// WM_AVATAR when it's ready). The strip and the panel both show it.
fn guild_logo(voice: &Voice, main: usize) -> Option<std::sync::Arc<avatars::Image>> {
    voice.channel.as_ref().and_then(|c| {
        let (id, url) = (c.guild_id.as_ref()?, c.guild_icon.clone()?);
        let stem: String = url.rsplit('/').next().unwrap_or("").chars().take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_').collect();
        avatars::get_url(format!("guild_{id}_{stem}"), url, move || notify_avatar(main))
    })
}

impl App {
    /// Sends a command to the RPC worker.
    pub(crate) fn send(&self, cmd: rpc::Cmd) {
        if let Some(tx) = &self.rpc {
            let _ = tx.send(cmd);
        }
    }

    /// Sends a command whose caller needs to know whether the worker is still running.
    fn try_send(&self, cmd: rpc::Cmd) -> bool {
        self.rpc.as_ref().is_some_and(|tx| tx.send(cmd).is_ok())
    }

    /// Mute, Deafen or Leave, from the strip or the panel: Discord does it and reports back.
    fn press(&self, b: Button) {
        match b {
            Button::Mute => self.send(rpc::Cmd::Mute(!self.voice.self_mute)),
            Button::Deafen => self.send(rpc::Cmd::Deaf(!self.voice.self_deaf)),
            Button::Leave => self.send(rpc::Cmd::Leave),
        }
    }

    /// The ring pulses (from dim) while we're unmuted in a call, with animations on.
    fn pulsing(&self) -> bool {
        self.voice.in_call() && !self.voice.self_mute && !self.voice.self_deaf && self.animations
    }

    /// Runs the ring's timer exactly while it pulses.
    fn sync_beat(&mut self, ctx: &mut Ctx) {
        let on = self.pulsing();
        if on != std::mem::replace(&mut self.beating, on) {
            if on {
                ctx.every(TIMER_BEAT, 40);
            } else {
                ctx.stop(TIMER_BEAT);
                self.ring_since = None;
            }
        }
    }

    /// What the strip shows; `images`: with the avatars and logo (drawing), not for measuring.
    fn content(&mut self, ring: Ring, images: bool) -> (Vec<voice::Member>, Vec<Option<std::sync::Arc<avatars::Image>>>, Option<std::sync::Arc<avatars::Image>>, Ring) {
        let main = self.main as usize;
        let people = self.slots.people(self.voice.channel.as_ref().map_or(&[], |c| &c.members));
        let avatars = if images { people.iter().map(|m| avatars::get(m, move || notify_avatar(main))).collect() } else { Vec::new() };
        let logo = if images { guild_logo(&self.voice, main) } else { None };
        (people, avatars, logo, ring)
    }

    /// A new state from Discord: the strip shows or hides (`visible`), redraws, the panel resizes.
    fn on_voice(&mut self, v: Voice, ctx: &mut Ctx) {
        if self.voice.status != v.status {
            log(&format!("status: {}", v.status.text()));
        }
        if self.voice.in_call() != v.in_call() {
            log(if v.in_call() { "joined a voice channel" } else { "left the voice channel" });
        }
        self.slots.update(v.channel.as_ref().map_or(&[], |c| &c.members), self.cfg.max_avatars);
        self.voice = v;
        self.sync_beat(ctx);
        ctx.redraw();
        ctx.resize_panel();
    }
}

impl Content for App {
    fn commands(&self) -> Vec<(&'static str, &'static str)> {
        commands::commands()
    }

    fn command(&mut self, name: &str, args: &[String], token: ReplyToken, _ctx: &mut Ctx) -> CommandReply {
        commands::command(self, name, args, token)
    }

    fn measure(&mut self, f: &Frame, compact: bool) -> i32 {
        let (people, avatars, logo, ring) = self.content(Ring::Dim, false);
        let c = StripContent { voice: &self.voice, people, avatars, max_avatars: self.cfg.max_avatars, logo, ring, text_w: self.media_w, fixed: self.cfg.fixed_width };
        render::strip(None, &c, compact, f.fit_width, f.height, f.scale, f.look).size.cx
    }

    fn draw(&mut self, cv: &mut Canvas, f: &Frame) {
        self.animations = f.look.animations;
        // the ring: pulses (from dim) while we're unmuted, steady while muted, red while deafened
        let ring = if self.voice.self_deaf {
            Ring::Red
        } else if self.voice.self_mute {
            Ring::Dim
        } else if !self.pulsing() {
            Ring::Pulse(0.6)
        } else {
            let t = *self.ring_since.get_or_insert_with(Instant::now);
            Ring::Pulse(taskbar_dock::anim::pulse(t.elapsed().as_secs_f32()))
        };
        let (people, avatars, logo, ring) = self.content(ring, true);
        let c = StripContent { voice: &self.voice, people, avatars, max_avatars: self.cfg.max_avatars, logo, ring, text_w: self.media_w, fixed: self.cfg.fixed_width };
        let lay = render::strip(Some(cv), &c, f.compact, f.fit_width, f.height, f.scale, f.look);
        self.strip = Some(lay);
    }

    fn hit(&self, x: i32, y: i32) -> Hit {
        self.strip.map_or(Hit::Strip, |l| l.block_hit(x, y))
    }

    fn cover(&self) -> Option<Box4> {
        self.strip.and_then(|l| l.cover())
    }

    fn tint(&self, area: Hit, _look: &Look) -> Option<Tint> {
        render::tint(&self.voice, area)
    }

    /// During a call, or when the user must act (setup, sign-in, an error).
    fn visible(&self) -> bool {
        self.voice.in_call() || self.voice.status.needs_user()
    }

    fn start(&mut self, ctx: &mut Ctx) {
        self.main = ctx.main_window();
        let main = self.main as usize;
        self.rpc = Some(rpc::start(self.cfg.connection_key(), Box::new(move |v: Voice| {
            let msg = Box::into_raw(Box::new(v));
            unsafe {
                if PostMessageW(HWND(main as *mut c_void), WM_VOICE, WPARAM(0), LPARAM(msg as isize)).is_err() {
                    drop(Box::from_raw(msg));
                }
            }
        })));
    }

    fn timer(&mut self, id: usize, ctx: &mut Ctx) {
        if id == TIMER_BEAT {
            ctx.redraw();
            self.sync_beat(ctx);
        }
    }

    fn button(&mut self, id: usize, _ctx: &mut Ctx) {
        if let Some(&b) = render::STRIP_BUTTONS.get(id) {
            self.press(b);
        }
    }

    fn cover_click(&mut self, ctx: &mut Ctx) {
        ctx.after(open::open_discord);
    }

    fn on_message(&mut self, msg: u32, _wp: usize, lp: isize, ctx: &mut Ctx) -> Option<isize> {
        match msg {
            WM_VOICE => self.on_voice(*unsafe { Box::from_raw(lp as *mut Voice) }, ctx),
            WM_AVATAR => {
                ctx.redraw();
                ctx.redraw_panel();
            }
            _ => return None,
        }
        Some(0)
    }

    fn panel_size(&mut self, kind: PanelKind, scale: f32) -> Option<(i32, i32)> {
        if kind != PanelKind::Main { return None; }
        self.panel_scale = scale;
        let lay = render::panel_layout(&self.voice, scale);
        Some((lay.w, lay.h))
    }

    fn panel_draw(&mut self, _kind: PanelKind, cv: &mut Canvas, f: &PanelFrame) {
        self.panel_scale = f.scale;
        let main = self.main as usize;
        let avatar = move |m: &voice::Member| avatars::get(m, move || notify_avatar(main));
        let logo = guild_logo(&self.voice, main);
        let dc = windows::Win32::Graphics::Gdi::HDC(cv.dc as _);
        render::paint_panel(dc, f.w, f.h, &self.voice, &avatar, logo.as_deref(), self.panel_hover, f.scale, f.look);
    }

    /// The panel's buttons: highlighted under the mouse, pressed on release.
    fn panel_mouse(&mut self, _kind: PanelKind, ev: PanelMouse, ctx: &mut Ctx) {
        let at = |x: i32, y: i32| render::panel_layout(&self.voice, self.panel_scale).hit(x, y);
        match ev {
            PanelMouse::Move(x, y) => {
                let b = at(x, y);
                if std::mem::replace(&mut self.panel_hover, b) != b {
                    ctx.redraw_panel();
                }
            }
            PanelMouse::Leave => {
                if self.panel_hover.take().is_some() {
                    ctx.redraw_panel();
                }
            }
            PanelMouse::Up(x, y) => {
                if let Some(b) = at(x, y) {
                    self.press(b);
                }
            }
            PanelMouse::Down(..) | PanelMouse::DoubleClick(..) | PanelMouse::Cancel => {}
        }
    }

    fn menu(&mut self, m: &mut MenuBuilder) {
        menu::build(self, m);
    }

    fn menu_switch(&mut self, id: usize, _ctx: &mut Ctx) -> bool {
        menu::switch(self, id)
    }

    fn menu_action(&mut self, id: usize, _ctx: &mut Ctx) {
        menu::action(self, id);
    }

    fn settings_changed(&mut self, ctx: &mut Ctx) {
        menu::settings_changed(self, ctx);
    }

    fn look_changed(&mut self, ctx: &mut Ctx) {
        self.animations = Look::current().animations;
        self.sync_beat(ctx);
    }

    fn tray_icon(&mut self, cv: &mut Canvas, look: &Look) -> bool {
        render::tray_logo(cv, look)
    }

    /// The call state: "General · 4 in call", "Discord · Not in a call", ...
    fn tray_tip(&self) -> String {
        let (l1, l2) = render::strip_lines(&self.voice);
        format!("{l1} · {l2}")
    }
}

const WIDGET_CLASS: &str = "TaskbarDiscordStrip";
const WIDGET_SECTION: &str = "discord";
const WIDGET_TITLE: &str = "Discord voice";
const WIDGET_DESCRIPTION: &str = "Who is in your Discord voice call";
/// Returns the Discord widget's stored defaults.
fn defaults() -> serde_json::Value {
    serde_json::to_value(config::Config::default()).unwrap_or_default()
}
/// Describes the Discord widget without starting its runtime.
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
    spec.mutex = "Local\\TaskbarDiscord.SingleInstance".into();
    spec.float = Float::Off; // as before: only on the taskbar
    // in a call the avatar column follows who's in it: Block glides the width (unless `fixed_width`)
    spec.glide_width = true;
    spec.log_file = Some(log_file());
    spec.defaults = Some(defaults);
    let cfg = Config::load();
    let status = if cfg.configured() || std::env::var_os("TASKBAR_DISCORD_TEST").is_some() { Status::Connecting } else { Status::NotConfigured };
    log("started");
    let app = App {
        main: 0,
        cfg,
        voice: Voice { status, ..Default::default() },
        slots: voice::Slots::default(),
        rpc: None,
        strip: None,
        ring_since: None,
        beating: false,
        animations: Look::current().animations,
        media_w: media_text_width(),
        panel_hover: None,
        panel_scale: 1.0,
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
