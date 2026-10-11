//! The click panel's content on Block's panel window: what it shows, its mouse (buttons, seek
//! bar, volume slider, mute), the app's volume kept current, and what moves in it. Block opens,
//! places, fades, pins and closes it; with nothing playing it's the small "Nothing playing" panel.

use crate::panel_view::{self, Bar, Content, Layout, Part};
use crate::render::{copy_into, Canvas};
use crate::{button_cmd, buttons, current, is_live, realtime, ring_level, App, Cmd, Progress, TIMER_PANEL};
use std::time::Instant;
use taskbar_dock::block::{self, Ctx, PanelFrame, PanelMouse};

/// While open, this often: redraw if something moves (scrolling text, pulsing bar, a drag), else
/// twice a second (the times); the volume follows the Volume Mixer.
const FRAME_MS: u32 = 33;

/// The panel's state.
pub(crate) struct State {
    pub open: bool,
    /// When it opened (scrolling text starts from here).
    pub opened: Instant,
    /// The scale it was last sized or drawn at.
    pub scale: f32,
    /// It was last sized for a session (else "Nothing playing").
    pub session: bool,
    pub hover: Option<Part>,
    pub pressed: Option<Part>,
    /// The progress bar being dragged, at this fraction.
    pub seek: Option<f32>,
    /// Something in it moves (scrolling text, pulsing bar): redraw every frame.
    pub moving: bool,
    pub painted: Instant,
    /// The shown app's Volume Mixer entries (and whose, and when they were looked up), its
    /// volume as last read, and the slider being dragged, at this level.
    pub vol_entries: Vec<windows::Win32::Media::Audio::ISimpleAudioVolume>,
    pub vol_id: String,
    pub vol_at: Option<Instant>,
    pub volume: Option<crate::audio::Volume>,
    pub vol_drag: Option<f32>,
}

impl Default for State {
    fn default() -> Self {
        State { open: false, opened: Instant::now(), scale: 1.0, session: false, hover: None, pressed: None, seek: None, moving: false,
                painted: Instant::now(), vol_entries: Vec::new(), vol_id: String::new(), vol_at: None, volume: None, vol_drag: None }
    }
}

/// What the panel shows, from the session on the strip.
fn content(a: &App) -> Option<Content> {
    let s = current(a)?;
    let level = ring_level(a, s);
    let bar = match s.progress() {
        _ if is_live(a, s) => Bar::Live(level),
        Progress::At { pos, len } if realtime(a, s) => Bar::At { pos, len, seek: s.can_seek },
        _ => Bar::Pulse(level),
    };
    let app = crate::apps::display_name(&s.id);
    // the brand: "YouTube · Firefox", the logo or badge without a cover, the colour
    let id = crate::identity(a, s);
    Some(Content {
        title: if s.title.is_empty() { app } else { s.title.clone() },
        artist: s.artist.clone(),
        album: s.album.clone(),
        source: id.source,
        cover: id.picture,
        badge: id.badge,
        accent: id.color,
        wash: 0.10 * a.cfg.tint_strength.clamp(0.0, 3.0),
        bar,
        buttons: buttons(a, s),
        volume: a.panel.volume,
    })
}

fn layout(a: &App) -> Layout {
    panel_view::layout(a.cfg.click_panel_width, a.panel.scale)
}

pub(crate) fn size(a: &mut App, scale: f32) -> (i32, i32) {
    a.panel.scale = scale;
    a.panel.session = current(a).is_some();
    if a.panel.session {
        let l = layout(a);
        (l.w, l.h)
    } else {
        panel_view::nothing_size(scale)
    }
}

pub(crate) fn draw(a: &mut App, cv: &mut block::Canvas, f: &PanelFrame) {
    a.panel.scale = f.scale;
    let Some(mut mine) = Canvas::new(f.w, f.h) else { return };
    match content(a) {
        Some(c) => {
            let p = &a.panel;
            let l = layout(a);
            a.panel.moving = panel_view::paint(&mut mine, &l, &c, f.look, f.scale, p.opened.elapsed().as_secs_f32(), p.hover, p.seek, p.vol_drag, a.cfg.live_rgb());
        }
        None => {
            panel_view::paint_nothing(&mut mine, f.look, f.scale);
            a.panel.moving = false;
        }
    }
    a.panel.painted = Instant::now();
    let (w, src) = (mine.w, mine.pixels().to_vec());
    let cw = cv.w;
    copy_into(cv.pixels(), cw, &src, w);
}

/// Opened: scrolling starts over, the volume is looked up, the frame timer runs; closed: all
/// that is forgotten.
pub(crate) fn shown(a: &mut App, open: bool, ctx: &mut Ctx) {
    let scale = a.panel.scale;
    a.panel = State { open, scale, session: a.panel.session, ..State::default() };
    if open {
        refresh_volume(a);
        ctx.every(TIMER_PANEL, FRAME_MS);
    } else {
        ctx.stop(TIMER_PANEL);
    }
}

/// Every frame while open: the playing session came or went (resize: "Nothing playing" or the
/// full panel), else redraw when something moves, the volume changed, or every half second.
pub(crate) fn frame(a: &mut App, ctx: &mut Ctx) {
    if !a.panel.open {
        return ctx.stop(TIMER_PANEL);
    }
    if current(a).is_some() != a.panel.session {
        ctx.resize_panel();
        return;
    }
    let volume_changed = refresh_volume(a);
    let p = &a.panel;
    if p.moving || p.seek.is_some() || volume_changed || p.painted.elapsed().as_millis() >= 500 {
        ctx.redraw_panel();
    }
}

/// The panel's volume, kept current (changes made in the Volume Mixer show up): the app's
/// entries are looked up again every second, the volume read twice a second. Not while the
/// slider is being dragged. True if it changed.
fn refresh_volume(a: &mut App) -> bool {
    let Some(s) = current(a) else { return false };
    let id = s.id.clone();
    let p = &mut a.panel;
    let same_app = p.vol_id == id;
    if p.vol_drag.is_some() || p.vol_at.is_some_and(|t| t.elapsed().as_millis() < 500 && same_app) {
        return false;
    }
    if !same_app || p.vol_at.is_none_or(|t| t.elapsed().as_millis() >= 1000) {
        p.vol_entries = crate::audio::entries(&id);
        p.vol_id = id;
    }
    p.vol_at = Some(Instant::now());
    let v = crate::audio::get(&p.vol_entries);
    std::mem::replace(&mut p.volume, v) != v
}

/// Sets the shown app's volume from the slider (unmuting it, like the Volume Mixer).
fn set_volume(a: &mut App, level: f32) {
    let p = &mut a.panel;
    crate::audio::set_level(&p.vol_entries, level);
    if let Some(v) = p.volume.as_mut() {
        v.level = level;
        if level > 0.0 {
            v.muted = false;
        }
    }
}

fn toggle_mute(a: &mut App) {
    let p = &mut a.panel;
    if let Some(v) = p.volume.as_mut() {
        v.muted = !v.muted;
        crate::audio::set_muted(&p.vol_entries, v.muted);
    }
}

/// A button or the seek bar was used: the media worker does it.
fn act_on(a: &App, part: Part, at_fraction: Option<f32>) {
    let cmd = match part {
        // the same buttons as the strip's; a dimmed one does nothing
        Part::Button(i) => button_cmd(a, i),
        Part::Bar => current(a).and_then(|s| match (s.progress(), at_fraction) {
            (Progress::At { len, .. }, Some(f)) => Some(Cmd::Seek(s.id.clone(), f as f64 * len)),
            _ => None,
        }),
        Part::Mute | Part::Volume => None,
    };
    if let Some(cmd) = cmd {
        a.send(cmd);
    }
}

/// Can the bar be dragged now?
fn seekable(a: &App) -> bool {
    content(a).is_some_and(|c| matches!(c.bar, Bar::At { seek: true, .. }))
}

pub(crate) fn mouse(a: &mut App, ev: PanelMouse, ctx: &mut Ctx) {
    if !a.panel.session {
        return; // "Nothing playing": nothing to use
    }
    let l = layout(a);
    match ev {
        PanelMouse::Move(x, y) => {
            let part = l.hit(x, y);
            let changed = std::mem::replace(&mut a.panel.hover, part) != part;
            if a.panel.seek.is_some() {
                a.panel.seek = Some(l.bar_fraction(x));
            }
            // dragging the volume slider: the volume follows the pointer
            if a.panel.vol_drag.is_some() {
                let f = l.volume_fraction(x);
                a.panel.vol_drag = Some(f);
                set_volume(a, f);
            }
            if changed || a.panel.seek.is_some() || a.panel.vol_drag.is_some() {
                ctx.redraw_panel();
            }
        }
        PanelMouse::Leave => {
            a.panel.hover = None;
            ctx.redraw_panel();
        }
        PanelMouse::Down(x, y) => {
            let part = l.hit(x, y);
            a.panel.pressed = part;
            if part == Some(Part::Bar) && seekable(a) {
                a.panel.seek = Some(l.bar_fraction(x));
            }
            if part == Some(Part::Volume) && a.panel.volume.is_some() {
                let f = l.volume_fraction(x);
                a.panel.vol_drag = Some(f);
                set_volume(a, f);
            }
            ctx.redraw_panel();
        }
        PanelMouse::Up(x, y) => {
            let here = l.hit(x, y);
            let (pressed, seek, vol) = (a.panel.pressed.take(), a.panel.seek.take(), a.panel.vol_drag.take());
            if let Some(f) = seek {
                act_on(a, Part::Bar, Some(f));
            } else if let Some(f) = vol {
                set_volume(a, f);
            } else if pressed.is_some() && pressed == here {
                // a button or the speaker acts when released where it was pressed
                match pressed {
                    Some(Part::Button(i)) => act_on(a, Part::Button(i), None),
                    Some(Part::Mute) => toggle_mute(a),
                    _ => {}
                }
            }
            ctx.redraw_panel();
        }
        PanelMouse::DoubleClick(..) => {}
        PanelMouse::Cancel => {
            (a.panel.pressed, a.panel.seek, a.panel.vol_drag) = (None, None, None);
            ctx.redraw_panel();
        }
    }
}
