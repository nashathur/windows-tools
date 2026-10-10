//! The two panels: what goes where, and drawing them.
//!
//! Click panel (left click; full control, fixed `click_panel_width`, 360 px, fixed height):
//! ```text
//! [cover 96]  Title / Artist / Album / Source (each one line, scrolls; empty ones close up)
//! ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  (drag to seek where supported)
//! 1:02                                                 3:57
//!                    ⏮      ⏯      ⏭
//! 🔊 ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━●        70
//! ```
//! Menu panel (right click, in place of a Windows menu; fixed `menu_panel_width`, 300 px):
//! ```text
//! [cover 48]  Title / Artist · Album / Source
//! ───────────────────────────────────
//! ↗  Open Spotify
//! ⧉  Copy "Artist – Title"
//! ───────────────────────────────────
//!    Compact mode              [ on ]
//!    Start with Windows        [off ]
//! ───────────────────────────────────
//! ⚙  Edit settings…    ⏻  Quit
//! ```
//! The progress bar follows the strip's ring: a position the app reports live → filled to
//! there, with times; none (Firefox) → full and pulsing, no times; a live stream → full, red,
//! pulsing, "LIVE".

use crate::cover::{self, Cover};
use taskbar_dock::draw::{self, fill_round};
use crate::render::{draw_text, rgb, text_size, Canvas, Font, ICON_FONT};
use crate::timeline;
use taskbar_dock::marquee::{self, Marquee};
use taskbar_dock::style::Look;
use taskbar_dock::theme::{self, space, Rgb, Theme};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Click,
    Menu,
}

/// The menu panel's rows, in order (see Part::Row).
pub const ROW_OPEN: usize = 0;
pub const ROW_COPY: usize = 1;
pub const ROW_COMPACT: usize = 2;
pub const ROW_STARTUP: usize = 3;
pub const ROW_SETTINGS: usize = 4;
pub const ROW_QUIT: usize = 5;

/// A rectangle in panel px: left, top, width, height.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rc {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rc {
    fn new(x: i32, y: i32, w: i32, h: i32) -> Rc {
        Rc { x, y, w, h }
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    fn rect(&self) -> RECT {
        RECT { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Layout {
    pub mode: Mode,
    pub w: i32,
    pub h: i32,
    pub cover: Rc,
    /// Text lines, top to bottom (click: title, artist, album, source; menu: title,
    /// artist · album, source).
    pub lines: [Rc; 4],
    /// Menu panel only: its rows (ROW_…) and the separator lines' y.
    pub rows: [Rc; 6],
    pub seps: [i32; 3],
    pub bar: Rc,
    /// Where a press grabs the bar (taller than it).
    pub bar_hit: Rc,
    pub times: Rc,
    /// Click panel only: the three buttons.
    pub buttons: [Rc; 3],
    /// Click panel only: the volume row, and in it the speaker (mute) button, the slider
    /// (its track, and where a press grabs it) and the percentage.
    pub volume: Rc,
    pub mute: Rc,
    pub slider: Rc,
    pub slider_hit: Rc,
    pub percent: Rc,
}

/// What the mouse is over in the panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    Button(usize),
    Bar,
    Mute,
    Volume,
    /// A menu panel row (ROW_…).
    Row(usize),
}

pub fn layout(mode: Mode, width: f32, scale: f32) -> Layout {
    let s = |v: f32| (v * scale).round() as i32;
    let w = s(width);
    match mode {
        Mode::Menu => {
            let (pad, cov) = (s(10.0), s(44.0));
            let tx = pad + cov + s(10.0);
            let tw = w - tx - pad;
            // rows span the panel less a small inset, like Windows 11's menus
            let (inset, rh) = (s(4.0), s(32.0));
            let row = |y: f32| Rc::new(inset, s(y), w - 2 * inset, rh);
            let half = (w - 2 * inset) / 2;
            Layout {
                mode,
                w,
                h: s(250.0),
                cover: Rc::new(pad, pad, cov, cov),
                lines: [Rc::new(tx, s(10.0), tw, s(20.0)), Rc::new(tx, s(30.0), tw, s(16.0)), Rc::new(tx, s(45.0), tw, s(14.0)), Rc::default()],
                rows: [row(66.0), row(98.0), row(138.0), row(170.0), Rc::new(inset, s(210.0), half, rh), Rc::new(inset + half, s(210.0), w - 2 * inset - half, rh)],
                seps: [s(62.0), s(134.0), s(206.0)],
                bar: Rc::default(),
                bar_hit: Rc::default(),
                times: Rc::default(),
                buttons: [Rc::default(); 3],
                volume: Rc::default(),
                mute: Rc::default(),
                slider: Rc::default(),
                slider_hit: Rc::default(),
                percent: Rc::default(),
            }
        }
        Mode::Click => {
            let (pad, cov) = (s(16.0), s(96.0));
            let tx = pad + cov + s(14.0);
            let tw = w - tx - pad;
            let bar = Rc::new(pad, s(124.0), w - 2 * pad, s(4.0));
            let (b, step, by) = (s(36.0), s(52.0), s(150.0));
            let button = |i: i32| Rc::new(w / 2 + (i - 1) * step - b / 2, by, b, b);
            let volume = Rc::new(pad, s(192.0), w - 2 * pad, s(28.0));
            let (mute_w, pct_w) = (s(32.0), s(40.0));
            let slider = Rc::new(volume.x + mute_w + s(10.0), volume.y + (volume.h - s(4.0)) / 2, volume.w - mute_w - s(10.0) - pct_w - s(8.0), s(4.0));
            Layout {
                mode,
                w,
                h: s(232.0),
                cover: Rc::new(pad, pad, cov, cov),
                lines: [Rc::new(tx, s(19.0), tw, s(26.0)), Rc::new(tx, s(46.0), tw, s(21.0)), Rc::new(tx, s(66.0), tw, s(21.0)), Rc::new(tx, s(88.0), tw, s(18.0))],
                rows: [Rc::default(); 6],
                seps: [0; 3],
                bar,
                bar_hit: Rc::new(pad - s(6.0), s(114.0), w - 2 * pad + s(12.0), s(24.0)),
                times: Rc::new(pad, s(132.0), w - 2 * pad, s(16.0)),
                buttons: [button(0), button(1), button(2)],
                volume,
                mute: Rc::new(volume.x, volume.y, mute_w, volume.h),
                slider,
                slider_hit: Rc::new(slider.x - s(8.0), volume.y, slider.w + s(16.0), volume.h),
                percent: Rc::new(volume.x + volume.w - pct_w, volume.y, pct_w, volume.h),
            }
        }
    }
}

impl Layout {
    pub fn hit(&self, x: i32, y: i32) -> Option<Part> {
        if self.mode == Mode::Menu {
            return self.rows.iter().position(|r| r.contains(x, y)).map(Part::Row);
        }
        if let Some(i) = self.buttons.iter().position(|b| b.contains(x, y)) {
            return Some(Part::Button(i));
        }
        if self.mute.contains(x, y) {
            return Some(Part::Mute);
        }
        if self.slider_hit.contains(x, y) {
            return Some(Part::Volume);
        }
        self.bar_hit.contains(x, y).then_some(Part::Bar)
    }

    /// The position (0..1) along the bar for panel x.
    pub fn bar_fraction(&self, x: i32) -> f32 {
        ((x - self.bar.x) as f32 / self.bar.w.max(1) as f32).clamp(0.0, 1.0)
    }

    /// The volume (0..1) on the slider for panel x.
    pub fn volume_fraction(&self, x: i32) -> f32 {
        ((x - self.slider.x) as f32 / self.slider.w.max(1) as f32).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Bar {
    /// `pos` of `len` seconds; `seek`: dragging it seeks.
    At { pos: f64, len: f64, seek: bool },
    /// No live position: full, its brightness pulsing (0..1).
    Pulse(f32),
    /// A live stream: full, red, pulsing.
    Live(f32),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Content {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub source: String,
    pub cover: Option<crate::media::CoverRef>,
    pub bar: Bar,
    pub buttons: [(char, bool); 3],
    /// The app's volume, if it has an entry in the Volume Mixer (else the row stays empty).
    pub volume: Option<crate::audio::Volume>,
    /// No picture: a badge in the brand's colour with its label.
    pub badge: Option<((u32, u32, u32), String)>,
    /// The brand colour: a light wash over the background (`wash`, 0..1) and the progress bar's
    /// and volume slider's fill. None: no tint (off, high contrast).
    pub accent: Option<(u32, u32, u32)>,
    pub wash: f32,
    /// For the menu panel: the app's name ("Open Spotify"), and the two switches.
    pub app: String,
    pub compact: bool,
    pub startup: bool,
}

struct Palette {
    bg: (u32, u32, u32),
    fg: COLORREF,
    dim: COLORREF,
    faint: COLORREF,
    /// Separators.
    line: (u32, u32, u32),
    track: (u32, u32, u32),
    fill: (u32, u32, u32),
    hover: (u32, u32, u32),
    placeholder: (u32, u32, u32),
    /// An off switch's outline and knob.
    knob: (u32, u32, u32),
    /// The progress bar's and volume slider's fill: the brand colour, else `fill`.
    bar: (u32, u32, u32),
}

/// The theme's colours (text as GDI COLORREFs).
fn palette(t: &Theme) -> Palette {
    let (c, cr) = (&t.color, |c: Rgb| COLORREF(c.colorref()));
    Palette { bg: c.bg.tuple(), fg: cr(c.fg), dim: cr(c.dim), faint: cr(c.faint), line: c.line.tuple(), track: c.track.tuple(),
              fill: c.fill.tuple(), hover: c.hover.tuple(), placeholder: c.placeholder.tuple(), knob: c.knob.tuple(), bar: c.fill.tuple() }
}

fn pack((r, g, b): (u32, u32, u32)) -> u32 {
    0xff00_0000 | r << 16 | g << 8 | b
}

/// The cover, scaled into `r` with rounded corners, over the panel background.
fn paint_cover(buf: &mut [u32], width: i32, c: &Cover, r: Rc, radius: f32, bg: (u32, u32, u32)) {
    let px = cover::resample(&c.pixels, c.size, (0, 0, c.size), r.w);
    let mut tile = px.clone();
    taskbar_dock::draw::round_rect(&mut tile, r.w, (0, 0, r.w, r.w), radius);
    let height = buf.len() as i32 / width.max(1);
    for dy in 0..r.h.min(r.w) {
        for dx in 0..r.w {
            let (x, y) = (r.x + dx, r.y + dy);
            if x < 0 || y < 0 || x >= width || y >= height {
                continue;
            }
            let p = tile[(dy * r.w + dx) as usize];
            let a = p >> 24;
            let ch = |s: u32, b: u32| (((p >> s) & 0xff) + b * (255 - a) / 255).min(255) << s;
            buf[(y * width + x) as usize] = 0xff00_0000 | ch(16, bg.0) | ch(8, bg.1) | ch(0, bg.2);
        }
    }
}

/// One line of text in `r`: scrolling (marquee) if it's too long and animations are on, else
/// cut with "…". `t`: seconds since the panel opened. Returns whether it's scrolling now.
fn paint_line(cv: &mut Canvas, font: &Font, text: &str, color: COLORREF, r: Rc, t: f32, scale: f32, animations: bool, bg: u32) -> bool {
    if text.is_empty() || r.w <= 0 {
        return false;
    }
    let (tw, _) = text_size(cv.dc, font, text);
    if tw <= r.w || !animations {
        draw_text(cv.dc, font, text, color, r.rect(), DT_LEFT | DT_TOP | DT_END_ELLIPSIS);
        return false;
    }
    let m = Marquee::at(tw as f32, r.w as f32, t, scale);
    let wtext: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        let old = SelectObject(cv.dc, font.0);
        SetTextColor(cv.dc, color);
        let clip = r.rect();
        // the text, and its start coming round again after the gap (it loops)
        for x in [r.x as f32 - m.offset, r.x as f32 - m.offset + m.span] {
            let _ = ExtTextOutW(cv.dc, x.round() as i32, r.y, ETO_CLIPPED, Some(&clip), windows::core::PCWSTR(wtext.as_ptr()), wtext.len() as u32, None);
        }
        SelectObject(cv.dc, old);
    }
    let (w, fade) = (cv.w, (marquee::FADE * scale).round() as i32);
    marquee::fade_edges(cv.pixels(), w, (r.x, r.y, r.x + r.w, r.y + r.h), fade, m.cut_left(), m.cut_right(), bg & 0xff_ffff);
    m.overflow > 0.0
}

/// A Windows 11 style on/off switch with its right edge at `right`, centred on `cy`.
fn switch(buf: &mut [u32], width: i32, right: f32, cy: f32, on: bool, pal: &Palette, scale: f32) {
    let (w, h) = (40.0 * scale, 20.0 * scale);
    let (l, t) = (right - w, cy - h / 2.0);
    let knob = if on { 6.0 } else { 5.0 } * scale;
    let kx = if on { right - h / 2.0 } else { l + h / 2.0 };
    if on {
        fill_round(buf, width, (l, t, right, t + h), h / 2.0, pal.fill, 1.0);
        fill_round(buf, width, (kx - knob, cy - knob, kx + knob, cy + knob), knob, pal.bg, 1.0);
    } else {
        // an outline: the pill in the dim colour with the background inside it
        let b = 1.0 * scale;
        fill_round(buf, width, (l, t, right, t + h), h / 2.0, pal.knob, 1.0);
        fill_round(buf, width, (l + b, t + b, right - b, t + h - b), h / 2.0 - b, pal.bg, 1.0);
        fill_round(buf, width, (kx - knob, cy - knob, kx + knob, cy + knob), knob, pal.knob, 1.0);
    }
}

/// The menu panel's separators and rows.
fn paint_menu(cv: &mut Canvas, l: &Layout, c: &Content, pal: &Palette, scale: f32, hover: Option<Part>, look: &Look) {
    let s = |v: f32| (v * scale).round() as i32;
    let w = cv.w;
    for y in l.seps {
        fill_round(cv.pixels(), w, (s(space::M) as f32, y as f32, (l.w - s(space::M)) as f32, (y + s(1.0).max(1)) as f32), 0.0, pal.line, 1.0);
    }
    // rows sit `r.x` px inside the panel: concentric with its corner, never below the control radius
    let row_radius = |inset: i32| draw::concentric(draw::PANEL_RADIUS * scale, inset as f32, draw::highlight_shape(look, scale).0);
    let body = Font::new(s(theme::ROW.px), theme::ROW.weight, &look.font);
    let icons = Font::new(s(theme::ICON.px), theme::ICON.weight, ICON_FONT);
    let copy = if c.artist.is_empty() { c.title.clone() } else { format!("{} – {}", c.artist, c.title) };
    let rows: [(Option<char>, String); 6] = [
        (Some('\u{E8A7}'), format!("Open {}", c.app)),
        (Some('\u{E8C8}'), format!("Copy \u{201C}{}\u{201D}", copy)),
        (None, "Compact mode".into()),
        (None, "Start with Windows".into()),
        (Some('\u{E713}'), "Edit settings…".into()),
        (Some('\u{E7E8}'), "Quit".into()),
    ];
    for (i, (glyph, label)) in rows.iter().enumerate() {
        let r = l.rows[i];
        if hover == Some(Part::Row(i)) {
            fill_round(cv.pixels(), w, (r.x as f32, r.y as f32 + 1.0, (r.x + r.w) as f32, (r.y + r.h) as f32 - 1.0), row_radius(r.x), pal.hover, 1.0);
        }
        let text_x = r.x + s(38.0);
        if let Some(g) = glyph {
            draw_text(cv.dc, &icons, &g.to_string(), pal.fg, RECT { left: r.x + s(8.0), top: r.y, right: r.x + s(32.0), bottom: r.y + r.h }, DT_CENTER | DT_VCENTER);
        }
        let label_x = if glyph.is_some() { text_x } else { r.x + s(10.0) };
        let switch_room = if i == ROW_COMPACT || i == ROW_STARTUP { s(56.0) } else { s(8.0) };
        draw_text(cv.dc, &body, label, pal.fg, RECT { left: label_x, top: r.y, right: r.x + r.w - switch_room, bottom: r.y + r.h }, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
        if i == ROW_COMPACT || i == ROW_STARTUP {
            let on = if i == ROW_COMPACT { c.compact } else { c.startup };
            let (right, cy) = ((r.x + r.w - s(10.0)) as f32, r.y as f32 + r.h as f32 / 2.0);
            switch(cv.pixels(), w, right, cy, on, pal, scale);
        }
    }
}

/// Draws a panel into `cv` (the panel's size). `t`: seconds since it opened (scrolling text);
/// `hover`: what the mouse is over; `drag`: the bar being dragged, at this fraction;
/// `vol_drag`: the volume slider being dragged, at this level.
/// Returns whether anything is moving (scrolling text, a pulsing bar), so it needs redrawing.
pub fn paint(cv: &mut Canvas, l: &Layout, c: &Content, look: &Look, scale: f32, t: f32, hover: Option<Part>, drag: Option<f32>,
             vol_drag: Option<f32>, live_rgb: (u32, u32, u32)) -> bool {
    let s = |v: f32| (v * scale).round() as i32;
    let mut pal = palette(&Theme::new(look));
    if let Some(acc) = c.accent {
        // the brand: a light wash over the background, and the bars in its colour (a neutral
        // brand keeps the plain bars)
        let mix = |a: (u32, u32, u32), k: f32| {
            let m = |x: u32, y: u32| (x as f32 * (1.0 - k) + y as f32 * k).round() as u32;
            (m(a.0, acc.0), m(a.1, acc.1), m(a.2, acc.2))
        };
        let k = c.wash.clamp(0.0, 0.5);
        pal.bg = mix(pal.bg, k);
        pal.hover = mix(pal.hover, k);
        pal.placeholder = mix(pal.placeholder, k);
        if acc != crate::brands::NEUTRAL {
            pal.bar = acc;
        }
    }
    let bg = pack(pal.bg);
    let w = cv.w;
    for p in cv.pixels().iter_mut() {
        *p = bg;
    }
    let mut moving = false;

    // the cover (or a placeholder with a note)
    let ctl = draw::highlight_shape(look, scale).0; // the control corner, as the strip's hover highlight
    let radius = draw::card_radius(look, scale, l.cover.w); // cover art: a card, 25% roundness
    match &c.cover {
        Some(pic) => paint_cover(cv.pixels(), w, &pic.0, l.cover, radius, pal.bg),
        None if c.badge.is_some() => {
            // no picture: the brand's badge
            let (color, label) = c.badge.clone().unwrap_or_default();
            let tile = crate::render::badge_tile(color, &label, l.cover.w, look, scale);
            paint_cover(cv.pixels(), w, &Cover { size: l.cover.w, pixels: tile }, l.cover, radius, pal.bg);
        }
        None => {
            let r = l.cover;
            fill_round(cv.pixels(), w, (r.x as f32, r.y as f32, (r.x + r.w) as f32, (r.y + r.h) as f32), radius, pal.placeholder, 1.0);
            let note = Font::new(r.w * 2 / 5, 400, ICON_FONT);
            draw_text(cv.dc, &note, &crate::render::glyph::MUSIC.to_string(), pal.dim, r.rect(), DT_CENTER | DT_VCENTER);
        }
    }

    // text: title bold, the rest dimmer; empty lines (no album, say) close up
    let click = l.mode == Mode::Click;
    let font = |t: theme::TextStyle| Font::new(s(t.px), t.weight, &look.font);
    let title = font(if click { theme::TITLE } else { theme::TITLE_SMALL });
    let body = font(if click { theme::BODY_LARGE } else { theme::CAPTION });
    let small = font(if click { theme::ROW } else { theme::CAPTION });
    let lines: Vec<(&Font, String, COLORREF)> = match l.mode {
        Mode::Menu => {
            let second = [c.artist.as_str(), c.album.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
            vec![(&title, c.title.clone(), pal.fg), (&body, second, pal.dim), (&small, c.source.clone(), pal.faint)]
        }
        Mode::Click => vec![(&title, c.title.clone(), pal.fg), (&body, c.artist.clone(), pal.dim), (&body, c.album.clone(), pal.dim), (&small, c.source.clone(), pal.faint)],
    };
    for (i, (font, text, color)) in lines.iter().filter(|(_, text, _)| !text.is_empty()).enumerate() {
        moving |= paint_line(cv, font, text, *color, l.lines[i], t, scale, look.animations, bg);
    }

    if l.mode == Mode::Menu {
        paint_menu(cv, l, c, &pal, scale, hover, look);
        return moving;
    }

    // the progress bar and its times
    let b = l.bar;
    let (bx, by, bw, bh) = (b.x as f32, b.y as f32, b.w as f32, b.h as f32);
    let r = bh / 2.0;
    fill_round(cv.pixels(), w, (bx, by, bx + bw, by + bh), r, pal.track, 1.0);
    let times = l.times;
    match c.bar {
        Bar::At { pos, len, seek } => {
            let f = drag.unwrap_or((pos / len.max(0.001)) as f32).clamp(0.0, 1.0);
            fill_round(cv.pixels(), w, (bx, by, bx + bw * f, by + bh), r, pal.bar, 1.0);
            if seek && (hover == Some(Part::Bar) || drag.is_some()) {
                // the thumb, while it can be dragged
                let (cx, cy, tr) = (bx + bw * f, by + bh / 2.0, s(6.0) as f32);
                fill_round(cv.pixels(), w, (cx - tr, cy - tr, cx + tr, cy + tr), tr, pal.bar, 1.0);
            }
            let shown = drag.map_or(pos, |d| d as f64 * len);
            draw_text(cv.dc, &small, &timeline::clock(shown), pal.dim, times.rect(), DT_LEFT | DT_TOP);
            draw_text(cv.dc, &small, &timeline::clock(len), pal.dim, times.rect(), DT_RIGHT | DT_TOP);
        }
        Bar::Pulse(level) => {
            fill_round(cv.pixels(), w, (bx, by, bx + bw, by + bh), r, pal.bar, 0.35 + 0.55 * level);
            moving = true;
        }
        Bar::Live(level) => {
            fill_round(cv.pixels(), w, (bx, by, bx + bw, by + bh), r, live_rgb, 0.35 + 0.55 * level);
            draw_text(cv.dc, &small, "LIVE", rgb(live_rgb.0 as u8, live_rgb.1 as u8, live_rgb.2 as u8), times.rect(), DT_LEFT | DT_TOP);
            moving = true;
        }
    }

    // the click panel's buttons
    if l.mode == Mode::Click {
        let icons = Font::new(s(theme::ICON_LARGE.px), theme::ICON_LARGE.weight, ICON_FONT);
        for (i, (g, enabled)) in c.buttons.iter().enumerate() {
            let r = l.buttons[i];
            if *enabled && hover == Some(Part::Button(i)) {
                fill_round(cv.pixels(), w, (r.x as f32, r.y as f32, (r.x + r.w) as f32, (r.y + r.h) as f32), ctl, pal.hover, 1.0);
            }
            let col = if *enabled { pal.fg } else { pal.faint };
            draw_text(cv.dc, &icons, &g.to_string(), col, r.rect(), DT_CENTER | DT_VCENTER);
        }
        // the volume row: speaker (click to mute), slider, percentage
        if let Some(v) = c.volume {
            let level = vol_drag.unwrap_or(v.level).clamp(0.0, 1.0);
            let shown = if v.muted && vol_drag.is_none() { 0.0 } else { level };
            let m = l.mute;
            if hover == Some(Part::Mute) {
                fill_round(cv.pixels(), w, (m.x as f32, m.y as f32, (m.x + m.w) as f32, (m.y + m.h) as f32), ctl, pal.hover, 1.0);
            }
            let speaker = if v.muted && vol_drag.is_none() {
                crate::render::glyph::MUTED
            } else {
                crate::render::glyph::VOLUME[if level <= 0.0 { 0 } else if level < 0.34 { 1 } else if level < 0.67 { 2 } else { 3 }]
            };
            let small_icons = Font::new(s(theme::ICON.px), theme::ICON.weight, ICON_FONT);
            draw_text(cv.dc, &small_icons, &speaker.to_string(), pal.fg, m.rect(), DT_CENTER | DT_VCENTER);
            let t = l.slider;
            let (tx, ty, tw, th) = (t.x as f32, t.y as f32, t.w as f32, t.h as f32);
            fill_round(cv.pixels(), w, (tx, ty, tx + tw, ty + th), th / 2.0, pal.track, 1.0);
            fill_round(cv.pixels(), w, (tx, ty, tx + tw * shown, ty + th), th / 2.0, pal.bar, if v.muted && vol_drag.is_none() { 0.4 } else { 1.0 });
            // the thumb: a ring in the background colour around a filled dot, like Windows' sliders
            let (cx, cy) = (tx + tw * shown, ty + th / 2.0);
            let (outer, inner) = (s(9.0) as f32, if hover == Some(Part::Volume) || vol_drag.is_some() { s(6.0) } else { s(5.0) } as f32);
            fill_round(cv.pixels(), w, (cx - outer, cy - outer, cx + outer, cy + outer), outer, pal.hover, 1.0);
            fill_round(cv.pixels(), w, (cx - inner, cy - inner, cx + inner, cy + inner), inner, pal.bar, 1.0);
            let pct = format!("{}", (level * 100.0).round() as i32);
            draw_text(cv.dc, &small, &pct, pal.dim, l.percent.rect(), DT_RIGHT | DT_VCENTER);
        }
    }
    moving
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_panel_rows() {
        let l = layout(Mode::Menu, 300.0, 1.0);
        assert_eq!((l.w, l.h), (300, 250));
        assert_eq!(l.cover, Rc::new(10, 10, 44, 44));
        assert_eq!(l.hit(150, 80), Some(Part::Row(ROW_OPEN)));
        assert_eq!(l.hit(150, 115), Some(Part::Row(ROW_COPY)));
        assert_eq!(l.hit(150, 150), Some(Part::Row(ROW_COMPACT)));
        assert_eq!(l.hit(150, 185), Some(Part::Row(ROW_STARTUP)));
        assert_eq!(l.hit(60, 225), Some(Part::Row(ROW_SETTINGS)));
        assert_eq!(l.hit(240, 225), Some(Part::Row(ROW_QUIT)));
        assert_eq!(l.hit(150, 30), None); // the header isn't clickable
        assert_eq!(l.hit(150, 63), None); // nor a separator
        assert!(l.rows[ROW_QUIT].y + l.rows[ROW_QUIT].h <= l.h);
    }

    #[test]
    fn click_panel_is_fixed_and_its_buttons_are_centred() {
        let l = layout(Mode::Click, 360.0, 1.0);
        assert_eq!((l.w, l.h), (360, 232));
        assert_eq!(l.cover.w, 96);
        let mid = l.buttons[1].x + l.buttons[1].w / 2;
        assert_eq!(mid, 180);
        assert_eq!(l.buttons[0].x + 52, l.buttons[1].x);
        assert!(l.volume.y + l.volume.h <= l.h);
        assert_eq!(l.hit(180, 168), Some(Part::Button(1)));
        assert_eq!(l.hit(100, 126), Some(Part::Bar));
        assert_eq!(l.hit(100, 60), None);
        assert_eq!(l.hit(l.mute.x + 4, l.mute.y + 4), Some(Part::Mute));
        assert_eq!(l.hit(l.slider.x + 30, l.volume.y + 14), Some(Part::Volume));
        assert_eq!(l.volume_fraction(l.slider.x + l.slider.w / 2), 0.5);
        assert!(l.percent.x >= l.slider.x + l.slider.w); // the percentage is right of the slider
        assert_eq!(l.bar_fraction(l.bar.x + l.bar.w / 4), 0.25);
        assert_eq!(l.bar_fraction(0), 0.0);
    }

    #[test]
    fn scales_with_the_display() {
        let l = layout(Mode::Click, 360.0, 1.5);
        assert_eq!((l.w, l.h, l.cover.w), (540, 348, 144));
    }
}
