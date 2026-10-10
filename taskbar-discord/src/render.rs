//! GDI drawing: the taskbar strip (per-pixel alpha) and the panel.

use crate::avatars::{self, Image};
use crate::voice::{Member, Status, Voice};
use std::ffi::c_void;
use std::sync::Arc;
use taskbar_dock::draw;
use taskbar_dock::style::Look;
use taskbar_dock::theme::{self, size, space, Rgb, Theme};
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

/// Discord's colours (0xRRGGBB), from the theme's brand list.
fn brand24(name: &str) -> u32 {
    theme::brand(name, true).rgb24()
}
const ICON_FONT: &str = theme::ICON_FONT;
pub const GLYPH_HEADSET: &str = "\u{E95B}";
pub const GLYPH_MIC: &str = "\u{E720}";
pub const GLYPH_HEADPHONE: &str = "\u{E7F6}";
pub const GLYPH_HANGUP: &str = "\u{E778}";

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn pcwstr(v: &[u16]) -> PCWSTR {
    PCWSTR(v.as_ptr())
}

pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

fn colorref(rgb24: u32) -> COLORREF {
    rgb((rgb24 >> 16) as u8, (rgb24 >> 8) as u8, rgb24 as u8)
}

pub struct Font(pub HFONT);
impl Font {
    pub fn new(px: i32, weight: i32, face: &str, quality: FONT_QUALITY) -> Font {
        let mut lf = LOGFONTW { lfHeight: -px.max(1), lfWeight: weight, lfQuality: quality, ..Default::default() };
        for (dst, src) in lf.lfFaceName.iter_mut().zip(face.encode_utf16().take(31)) {
            *dst = src;
        }
        Font(unsafe { CreateFontIndirectW(&lf) })
    }
}
impl Drop for Font {
    fn drop(&mut self) {
        unsafe { let _ = DeleteObject(self.0); }
    }
}

/// A 32-bit top-down DIB selected into a memory DC.
struct Canvas {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    w: i32,
    h: i32,
}
impl Canvas {
    fn new(w: i32, h: i32) -> Option<Canvas> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let old = SelectObject(dc, bmp);
            SetBkMode(dc, TRANSPARENT);
            Some(Canvas { dc, bmp, old, bits: bits as *mut u32, w, h })
        }
    }
    fn pixels(&mut self) -> &mut [u32] {
        unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize)
        }
    }
}
impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp);
            let _ = DeleteDC(self.dc);
        }
    }
}

fn text_size(dc: HDC, font: &Font, s: &str) -> (i32, i32) {
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut size = SIZE::default();
    unsafe {
        let old = SelectObject(dc, font.0);
        let _ = GetTextExtentPoint32W(dc, &w, &mut size);
        SelectObject(dc, old);
    }
    (size.cx, size.cy)
}

fn ascent(dc: HDC, font: &Font) -> i32 {
    let mut tm = TEXTMETRICW::default();
    unsafe {
        let old = SelectObject(dc, font.0);
        let _ = GetTextMetricsW(dc, &mut tm);
        SelectObject(dc, old);
    }
    tm.tmAscent
}

fn draw_text(dc: HDC, font: &Font, s: &str, color: COLORREF, x: i32, y: i32, w: i32, h: i32, flags: DRAW_TEXT_FORMAT) {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    let mut rc = RECT { left: x, top: y, right: x + w, bottom: y + h };
    unsafe {
        let old = SelectObject(dc, font.0);
        SetTextColor(dc, color);
        DrawTextW(dc, &mut buf, &mut rc, flags | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

// ---------------------------------------------------------------- the strip

/// What the strip shows: `people` are the ones in the avatar places (`voice::Slots`), `avatars`
/// their images ready so far (same order).
pub struct StripContent<'a> {
    pub voice: &'a Voice,
    pub people: Vec<Member>,
    pub avatars: Vec<Option<Arc<Image>>>,
    pub max_avatars: usize,
    /// The server's logo, if it has one and it's loaded.
    pub logo: Option<Arc<Image>>,
    pub ring: Ring,
    /// Width of the column for the people (px at 100%): media's `max_width`, so the strips match.
    pub text_w: f32,
}

/// The strip's buttons during a call (like media's previous / play / next): each one's
/// clickable width at 100%.
pub const STRIP_BUTTON: f32 = 24.0;
pub const STRIP_BUTTONS: [Button; 3] = [Button::Mute, Button::Deafen, Button::Leave];

/// The strip as drawn: its size and where its buttons are (none outside a call), for hit tests.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StripLayout {
    pub size: SIZE,
    pub buttons_x: i32,
    pub button_w: i32,
    pub buttons: bool,
    /// The scale it was drawn at: a button is hit only inside its highlight box
    /// (`draw::button_box`: glyph plus margin), not its whole column.
    pub scale: f32,
    /// The server logo's ring: left x and diameter (centred vertically; only in a call).
    pub ring_x: i32,
    pub ring_d: i32,
}

impl StripLayout {
    /// Is strip point (`x`, `y`) on the logo with its ring?
    pub fn on_logo(&self, x: i32, y: i32) -> bool {
        self.buttons && x < self.size.cx && draw::contains(draw::centered_box(self.ring_x, self.ring_d, self.size.cy), x, y)
    }

    /// Left and right x of button `i`.
    pub fn button(&self, i: usize) -> (i32, i32) {
        let l = self.buttons_x + i as i32 * self.button_w;
        (l, l + self.button_w)
    }

    /// The button under strip point (`x`, `y`): inside its highlight box only (None: the strip
    /// itself). The part cut off while collapsing isn't clickable.
    pub fn hit(&self, x: i32, y: i32) -> Option<Button> {
        if !self.buttons {
            return None;
        }
        (0..STRIP_BUTTONS.len())
            .find(|&i| {
                x < self.size.cx && draw::contains(draw::button_box(self.button(i), self.size.cy, self.scale), x, y)
            })
            .map(|i| STRIP_BUTTONS[i])
    }
}

/// The two text lines of the full strip.
pub fn strip_lines(v: &Voice) -> (String, String) {
    match &v.channel {
        Some(c) => {
            let mut line2 = format!("{} in call", v.count());
            if v.self_deaf {
                line2 += " · Deafened";
            } else if v.self_mute {
                line2 += " · Muted";
            }
            (if c.name.is_empty() { "Voice".into() } else { c.name.clone() }, line2)
        }
        None if v.status == Status::Ready => ("Discord".into(), "Not in a call".into()),
        None => ("Discord".into(), v.status.text()),
    }
}

/// How the logo's ring looks: pulsing while we're unmuted (`level` 0..1, as media's ring while
/// playing), steady and dim while muted, red while deafened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ring {
    Pulse(f32),
    Dim,
    Red,
}

/// Media's ring and cover geometry (taskbar-media layout.rs), so both strips line up.
const RING_GAP: f32 = 1.0;
const RING_WIDTH: f32 = 2.0;
const TEXT_GAP: f32 = 6.0;
const PAD_RIGHT: f32 = 2.0;
/// Overlap of the stacked avatars: each one's step as a part of its width.
const STACK_STEP: f32 = 0.62;
/// ... but never more than this (a quarter of each avatar still shows).
const MIN_STEP: f32 = 0.25;

/// The rows (top, bottom) the ink of `text` covers in `font`, for sizing like media's cover.
fn ink_rows(font: &Font, text: &str) -> (i32, i32) {
    let dc = unsafe { CreateCompatibleDC(None) };
    let (w, h) = text_size(dc, font, text);
    unsafe { let _ = DeleteDC(dc); }
    let Some(mut cv) = Canvas::new(w, h) else { return (0, h) };
    draw_text(cv.dc, font, text, rgb(255, 255, 255), 0, 0, w, h, DT_LEFT | DT_TOP);
    let (cw, ch) = (cv.w, cv.h);
    draw::ink_bounds(cv.pixels(), cw, ch).map_or((0, h), |(_, t, _, b)| (t, b))
}

/// The logo's size and the even margin around its ring inside the hover highlight (`highlight`
/// px tall), as media's `layout::margin`.
fn cover_margin(wanted: i32, highlight: i32, around: i32) -> (i32, i32) {
    let mut cover = wanted;
    if (highlight - (cover + 2 * around)) % 2 != 0 {
        cover -= 1;
    }
    (cover, ((highlight - (cover + 2 * around)) / 2).max(0))
}

/// Text (or a glyph) drawn onto the strip's finished pixels in `color` (0xRRGGBB).
#[allow(clippy::too_many_arguments)]
fn blit_text(buf: &mut [u32], width: i32, x: i32, y: i32, w: i32, h: i32, font: &Font, text: &str, color: u32) {
    if text.is_empty() || w <= 0 || h <= 0 {
        return;
    }
    let Some(mut cv) = Canvas::new(w, h) else { return };
    draw_text(cv.dc, font, text, rgb(255, 255, 255), 0, 0, w, h, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    let src = cv.pixels().to_vec();
    let height = buf.len() as i32 / width.max(1);
    for (i, p) in src.iter().enumerate() {
        let a = ((*p >> 16) & 0xff).max((*p >> 8) & 0xff).max(*p & 0xff);
        let (px, py) = (x + i as i32 % w, y + i as i32 / w);
        if a > 0 && px >= 0 && px < width && py >= 0 && py < height {
            avatars::over(&mut buf[(py * width + px) as usize], 0xFF00_0000 | color, a as f32 / 255.0);
        }
    }
}

/// A filled capsule (rounded all the way) over the strip's pixels.
fn fill_capsule(buf: &mut [u32], width: i32, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let height = buf.len() as i32 / width.max(1);
    let r = h as f32 / 2.0;
    for py in y.max(0)..(y + h).min(height) {
        for px in x.max(0)..(x + w).min(width) {
            let (fx, fy) = (px as f32 + 0.5 - x as f32, py as f32 + 0.5 - y as f32);
            let cx = fx.clamp(r, w as f32 - r);
            let dist = ((fx - cx).powi(2) + (fy - r).powi(2)).sqrt();
            let cover = (r - dist + 0.5).clamp(0.0, 1.0);
            if cover > 0.0 {
                avatars::over(&mut buf[(py * width + px) as usize], 0xFF00_0000 | color, cover);
            }
        }
    }
}

/// Draws the strip into its layered window. `scale` is the DPI scale times the drag zoom;
/// `hover`: the mouse is over the strip, `hot`: over that button (its highlight only); `clip`
/// cuts it at a width (compact switch animation); `measure_only` returns the layout only;
/// `grow`: the logo's hover / press factor (`anim::Feel`, 1.0 = at rest).
///
/// In a call it is laid out like the media strip (same logo/ring, text-column and button sizes,
/// so the two have the same width): `| logo+ring | stacked avatars (+N) | mute deafen leave |`,
/// compact without the avatars. Otherwise a headset and two lines of text.
#[allow(clippy::too_many_arguments)]
pub fn update_strip(hwnd: HWND, c: &StripContent, compact: bool, height: i32, scale: f32, look: &Look, hover: bool, hot: Option<Button>, clip: Option<i32>, measure_only: bool, grow: f32) -> StripLayout {
    let s = |v: f32| (v * scale).round() as i32;
    let v = c.voice;
    let f = Font::new(s(look.font_size), 400, &look.font, ANTIALIASED_QUALITY);
    let icon_font = Font::new(theme::GLYPH_RASTER_PX, 400, ICON_FONT, ANTIALIASED_QUALITY);
    let in_call = v.channel.is_some();
    let (icon_top, gw) = draw::icon_box(height, scale);
    let (radius, inset) = draw::highlight_shape(look, scale);
    let members: &[Member] = v.channel.as_ref().map_or(&[], |ch| &ch.members);

    let dc = unsafe { CreateCompatibleDC(None) };
    let asc = ascent(dc, &f);
    let (_, line_h) = text_size(dc, &f, "Ag");
    let (idle1, idle2) = if in_call { (String::new(), String::new()) } else { strip_lines(v) };
    let idle_w = |t: &str| if t.is_empty() { 0 } else { text_size(dc, &f, t).0.min(s(150.0)) };
    let (idle_w1, idle_w2) = (idle_w(&idle1), idle_w(&idle2));
    unsafe { let _ = DeleteDC(dc); }

    // the geometry shared with the media strip
    let (top1, top2) = draw::two_line_tops(height, scale, asc, asc);
    let (cap_top, _) = ink_rows(&f, "H");
    let (_, desc_bottom) = ink_rows(&f, "g");
    let around = s(RING_GAP + RING_WIDTH);
    let (cover, margin) = cover_margin((top2 + desc_bottom) - (top1 + cap_top), height - 2 * inset, around);
    let cover_top = inset + margin + around;
    let ring_d = cover + 2 * around;
    let (ring_x, button_w, text_gap) = (margin, s(STRIP_BUTTON), s(TEXT_GAP));

    let (text_x, text_w, buttons_x, natural);
    if in_call {
        text_x = ring_x + ring_d + if compact { 0 } else { text_gap };
        text_w = if compact { 0 } else { (c.text_w * scale).round() as i32 };
        buttons_x = text_x + text_w + if compact { s(2.0) } else { text_gap };
        natural = buttons_x + STRIP_BUTTONS.len() as i32 * button_w + s(PAD_RIGHT);
    } else {
        let pad = s(8.0);
        text_x = pad + gw + s(6.0);
        text_w = idle_w1.max(idle_w2);
        buttons_x = 0;
        natural = text_x + text_w + pad;
    }
    let mut lay = StripLayout { size: SIZE { cx: natural, cy: height }, buttons_x, button_w, buttons: in_call, scale, ring_x, ring_d };
    if measure_only {
        return lay;
    }
    let width = clip.map_or(natural, |cl| cl.clamp(1, natural));
    lay.size.cx = width;
    let Some(mut cv) = Canvas::new(width, height) else { return StripLayout::default() };
    let white = rgb(255, 255, 255);

    if in_call {
        // tray-icon sized glyphs centred in their columns; muted / deafened ones slashed
        let bf = Font::new(gw, 400, ICON_FONT, ANTIALIASED_QUALITY);
        for (i, b) in STRIP_BUTTONS.iter().enumerate() {
            let (bl, br) = lay.button(i);
            let glyph = match b {
                Button::Mute => GLYPH_MIC,
                Button::Deafen => GLYPH_HEADPHONE,
                Button::Leave => GLYPH_HANGUP,
            };
            draw_text(cv.dc, &bf, glyph, white, bl, icon_top, br - bl, gw, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            if strip_button_on(v, *b) {
                slash(cv.dc, bl + (br - bl - gw) / 2, icon_top, gw, white, scale);
            }
        }
    } else {
        let ell = DT_LEFT | DT_TOP | DT_SINGLELINE | DT_END_ELLIPSIS;
        if idle2.is_empty() {
            draw_text(cv.dc, &f, &idle1, white, text_x, (height - line_h + 1) / 2, text_w, line_h, ell);
        } else {
            draw_text(cv.dc, &f, &idle1, white, text_x, top1, text_w, line_h, ell);
            draw_text(cv.dc, &f, &idle2, white, text_x, top2, text_w, line_h, ell);
        }
        // the headset, drawn large and fitted into the tray-icon box (shared rule)
        if let Some(mut big) = Canvas::new(96, 96) {
            draw_text(big.dc, &icon_font, GLYPH_HEADSET, white, 0, 0, 96, 96, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            let src = big.pixels().to_vec();
            draw::fit_icon(cv.pixels(), width, s(8.0), icon_top, gw, &src, 96, 96);
        }
    }
    let (fg, _) = look.text_rgb();
    draw::text_to_color(cv.pixels(), width, height, |_| fg);
    let fg24 = (fg.0 << 16) | (fg.1 << 8) | fg.2;

    let buf = cv.pixels();
    if in_call {
        // switched on (we're muted / deafened): red, like Discord's own buttons
        for (i, b) in STRIP_BUTTONS.iter().enumerate() {
            if strip_button_on(v, *b) {
                let (bl, br) = lay.button(i);
                recolor(buf, width, (bl, icon_top, br, icon_top + gw), brand24("discord.red"));
            }
        }
        // the server's logo (a blurple disc with a headset without one), and the ring around it
        let logo = c.logo.as_deref();
        avatars::draw_circle(buf, width, (ring_x + around) as f32, cover_top as f32, cover as f32, logo, brand24("discord.blurple"), 255);
        if logo.is_none() {
            let glyph = (cover as f32 * 0.62).round() as i32;
            let icons = Font::new(glyph, 400, ICON_FONT, ANTIALIASED_QUALITY);
            blit_text(buf, width, ring_x + around + (cover - glyph) / 2, cover_top + (cover - glyph) / 2, glyph, glyph, &icons, GLYPH_HEADSET, Theme::new(look).color.on_fill.rgb24());
        }
        let (rgb24, alpha) = match c.ring {
            Ring::Pulse(level) => (fg24, (90.0 + 140.0 * level.clamp(0.0, 1.0)).round() as u32),
            Ring::Dim => (fg24, 110),
            Ring::Red => (brand24("discord.red"), 255),
        };
        avatars::draw_ring_a(buf, width, ring_x as f32, (cover_top - around) as f32, ring_d as f32, s(RING_WIDTH) as f32, rgb24, alpha);
        // hovered / pressed: the logo and its ring grow / dip about their centre
        draw::grow_pixels(buf, width, draw::centered_box(ring_x, ring_d, height), grow);

        // the people in the places, stacked, the first on top; everyone else (muted, deafened,
        // quiet longest, or more than fit): "+N"
        if !compact && !members.is_empty() {
            let ad = ring_d as f32;
            let pill_h = (ad * 0.8).round() as i32;
            let pill_font = Font::new(s(look.font_size) * 9 / 10, 600, &look.font, ANTIALIASED_QUALITY);
            let pill_text = |extra: usize| format!("+{extra}");
            let pill_w = |extra: usize| {
                let dc = unsafe { CreateCompatibleDC(None) };
                let w = text_size(dc, &pill_font, &pill_text(extra)).0 + s(12.0);
                unsafe { let _ = DeleteDC(dc); }
                w.max(pill_h)
            };
            let mut shown = c.people.len().min(c.max_avatars.max(1));
            let extra_of = |n: usize| members.len() - n;
            // the avatars overlap as much as it takes for all of them and the pill to fit the
            // column; only when that would hide too much of each one does one drop out
            let reserve = |n: usize| if extra_of(n) > 0 { s(4.0) as f32 + pill_w(extra_of(n)) as f32 } else { 0.0 };
            let step_for = |n: usize| if n > 1 { ((text_w as f32 - ad - reserve(n)) / (n - 1) as f32).min(ad * STACK_STEP).floor() } else { 0.0 };
            while shown > 1 && step_for(shown) < ad * MIN_STEP {
                shown -= 1;
            }
            let step = step_for(shown);
            let stack = |n: usize| if n == 0 { -s(4.0) as f32 } else { ad + step * (n - 1) as f32 };
            let y = (height as f32 - ad) / 2.0;
            let gap = (1.5 * scale).max(1.0);
            for i in (0..shown).rev() {
                let m = &c.people[i];
                let x = text_x as f32 + step * i as f32;
                if i + 1 < shown {
                    avatars::cut_circle(buf, width, x - gap, y - gap, ad + 2.0 * gap);
                }
                let (ring, d) = (2.0 * scale, ad);
                let inner = d - 2.0 * (ring + 1.0 * scale);
                let img = c.avatars.get(i).cloned().flatten();
                avatars::draw_circle(buf, width, x + ring + scale, y + ring + scale, inner, img.as_deref(), brand24("discord.blurple"), 255);
                if m.speaking {
                    avatars::draw_ring(buf, width, x, y, d, ring, brand24("discord.green"));
                }
            }
            let extra = extra_of(shown);
            if extra > 0 {
                let pw = pill_w(extra);
                let px = text_x + stack(shown).round() as i32 + s(4.0);
                let py = (height - pill_h) / 2;
                let t = Theme::new(look);
                let (bg, txt) = (t.color.chip.rgb24(), t.color.chip_text.rgb24());
                fill_capsule(buf, width, px, py, pw, pill_h, bg);
                blit_text(buf, width, px, py, pw, pill_h, &pill_font, &pill_text(extra), txt);
            }
        }
    } else if v.status.needs_user() {
        let d = 6.0 * scale;
        avatars::draw_circle(buf, width, s(8.0) as f32 + gw as f32 - d * 0.5, icon_top as f32 - d * 0.5, d, None, Theme::new(look).brand("discord.attention").rgb24(), 255);
    }
    if width < natural {
        // cut off while collapsing or expanding: fades out at the right end
        fade_right(buf, width, height, s(12.0));
    }
    // Highlights in Discord's colours (blurple; red for Leave and switched-on buttons). The whole
    // strip always sits in a blurple box (stronger on hover); a hovered button gets a box of
    // just its glyph plus the shared margin (`draw::button_box`) on top.
    draw::background_rect(buf, width, height, None, look.light, radius);
    let (rest, over) = if look.light { (70.0, 105.0) } else { (85.0, 125.0) };
    // (each box goes under the pixels already there, so the button's is drawn first)
    if let Some(i) = STRIP_BUTTONS.iter().position(|b| Some(*b) == hot).filter(|_| in_call) {
        let b = STRIP_BUTTONS[i];
        let tint = if b == Button::Leave || strip_button_on(v, b) { brand24("discord.red") } else { brand24("discord.blurple") };
        tint_highlight(buf, width, draw::button_box(lay.button(i), height, scale), tint, over, radius);
    }
    tint_highlight(buf, width, (0, inset, width, height - inset), brand24("discord.blurple"), if hover { over } else { rest }, radius);

    let size = SIZE { cx: width, cy: height };
    let src = POINT::default();
    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::UpdateLayeredWindow(hwnd, None, None, Some(&size), cv.dc, Some(&src), COLORREF(0), Some(&blend), windows::Win32::UI::WindowsAndMessaging::ULW_ALPHA);
    }
    lay
}

/// A highlight box over `rect` in `rgb24` (a Discord colour) at `alpha` (0..255), with the
/// highlight's `radius`: the shared tinted box, under the strip's pixels.
fn tint_highlight(buf: &mut [u32], width: i32, rect: (i32, i32, i32, i32), rgb24: u32, alpha: f32, radius: f32) {
    let rgb = (rgb24 >> 16 & 0xff, rgb24 >> 8 & 0xff, rgb24 & 0xff);
    draw::tint_box(buf, width, rect, rgb, alpha / 255.0, radius);
}

/// Is the strip's button switched on (we're muted / deafened)?
fn strip_button_on(v: &Voice, b: Button) -> bool {
    match b {
        Button::Mute => v.self_mute,
        Button::Deafen => v.self_deaf,
        Button::Leave => false,
    }
}

/// Premultiplied pixels in `rect` (left, top, right, bottom) take colour `rgb24`, keeping alpha.
fn recolor(buf: &mut [u32], width: i32, rect: (i32, i32, i32, i32), rgb24: u32) {
    let height = buf.len() as i32 / width.max(1);
    let c = (rgb24 >> 16 & 0xff, rgb24 >> 8 & 0xff, rgb24 & 0xff);
    for y in rect.1.max(0)..rect.3.min(height) {
        for x in rect.0.max(0)..rect.2.min(width) {
            let p = &mut buf[(y * width + x) as usize];
            let a = *p >> 24;
            *p = a << 24 | (c.0 * a / 255) << 16 | (c.1 * a / 255) << 8 | (c.2 * a / 255);
        }
    }
}

/// A slash across a `size` px glyph box at (x, y), drawn before the text is coloured.
fn slash(dc: HDC, x: i32, y: i32, size: i32, color: COLORREF, scale: f32) {
    unsafe {
        let pen = CreatePen(PS_SOLID, (2.0 * scale).round().max(1.0) as i32, color);
        let op = SelectObject(dc, pen);
        let m = size / 6;
        let _ = MoveToEx(dc, x + m, y + m, None);
        let _ = LineTo(dc, x + size - m, y + size - m);
        SelectObject(dc, op);
        let _ = DeleteObject(pen);
    }
}

/// Premultiplied pixels fade to transparent over the last `fade` columns.
fn fade_right(buf: &mut [u32], width: i32, height: i32, fade: i32) {
    for y in 0..height {
        for i in 0..fade.min(width) {
            let keep = (i as f32 + 0.5) / fade as f32;
            let p = &mut buf[(y * width + width - 1 - i) as usize];
            let ch = |s: u32| ((((*p >> s) & 0xff) as f32 * keep).round() as u32) << s;
            *p = ch(24) | ch(16) | ch(8) | ch(0);
        }
    }
}

// ---------------------------------------------------------------- the panel

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Button {
    Mute,
    Deafen,
    Leave,
}

pub const PANEL_W: f32 = size::PANEL_STANDARD;
const ROW_H: f32 = space::ROW_HEIGHT_LARGE;
const HEAD_H: f32 = 64.0;
const BUTTONS_H: f32 = 60.0;
const MAX_ROWS: usize = 10;

/// The panel's size and its buttons (client coordinates), for drawing and hit-testing.
pub struct PanelLayout {
    pub w: i32,
    pub h: i32,
    pub buttons: Vec<(Button, RECT)>,
}

impl PanelLayout {
    pub fn hit(&self, x: i32, y: i32) -> Option<Button> {
        self.buttons.iter().find(|(_, r)| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map(|(b, _)| *b)
    }
}

pub fn panel_layout(v: &Voice, scale: f32) -> PanelLayout {
    let s = |x: f32| (x * scale).round() as i32;
    let w = s(PANEL_W);
    match &v.channel {
        Some(c) => {
            let rows = c.members.len().min(MAX_ROWS) + usize::from(c.members.len() > MAX_ROWS);
            let list_top = s(HEAD_H) + s(6.0);
            let by = list_top + s(ROW_H) * rows as i32 + s(10.0);
            let (pad, gap) = (s(16.0), s(8.0));
            let bw = (w - 2 * pad - 2 * gap) / 3;
            let bh = s(36.0);
            let buttons = [Button::Mute, Button::Deafen, Button::Leave]
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let x = pad + (bw + gap) * i as i32;
                    (*b, RECT { left: x, top: by, right: x + bw, bottom: by + bh })
                })
                .collect();
            PanelLayout { w, h: by + s(BUTTONS_H) - s(10.0), buttons }
        }
        None => PanelLayout { w, h: s(if v.status == Status::NotConfigured { 250.0 } else { 150.0 }), buttons: Vec::new() },
    }
}

struct Palette {
    bg: COLORREF,
    fg: COLORREF,
    dim: COLORREF,
    line: COLORREF,
    button: COLORREF,
    button_hover: COLORREF,
}

/// The theme's colours as GDI COLORREFs.
fn palette(t: &Theme) -> Palette {
    let cr = |c: Rgb| COLORREF(c.colorref());
    let c = &t.color;
    Palette { bg: cr(c.bg), fg: cr(c.fg), dim: cr(c.dim), line: cr(c.line), button: cr(c.button), button_hover: cr(c.button_hover) }
}

fn fill(dc: HDC, r: RECT, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        FillRect(dc, &r, b);
        let _ = DeleteObject(b);
    }
}

fn round_fill(dc: HDC, r: RECT, radius: i32, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        let p = CreatePen(PS_SOLID, 1, c);
        let ob = SelectObject(dc, b);
        let op = SelectObject(dc, p);
        let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        let _ = DeleteObject(b);
        let _ = DeleteObject(p);
    }
}

/// A glyph with a red slash through it (muted mic, deafened headphones).
fn slashed(dc: HDC, font: &Font, glyph: &str, x: i32, y: i32, size: i32, color: COLORREF, scale: f32) {
    draw_text(dc, font, glyph, color, x, y, size, size, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    unsafe {
        let pen = CreatePen(PS_SOLID, (2.0 * scale).round() as i32, color);
        let op = SelectObject(dc, pen);
        let m = size / 5;
        let _ = MoveToEx(dc, x + m, y + m, None);
        let _ = LineTo(dc, x + size - m, y + size - m);
        SelectObject(dc, op);
        let _ = DeleteObject(pen);
    }
}

/// Draws an avatar circle (with the speaking ring) into an opaque DC at (x, y), `d` px.
fn panel_avatar(dc: HDC, x: i32, y: i32, d: i32, img: Option<&Image>, speaking: bool, bg: COLORREF, scale: f32) {
    let Some(mut cv) = Canvas::new(d, d) else { return };
    let buf = cv.pixels();
    let ring = 2.0 * scale;
    avatars::draw_circle(buf, d, ring + scale, ring + scale, d as f32 - 2.0 * (ring + scale), img, brand24("discord.blurple"), 255);
    if speaking {
        avatars::draw_ring(buf, d, 0.0, 0.0, d as f32, ring, brand24("discord.green"));
    }
    // onto the panel's background (premultiplied → opaque)
    let (br, bgc, bb) = (bg.0 & 0xff, (bg.0 >> 8) & 0xff, (bg.0 >> 16) & 0xff);
    for p in buf.iter_mut() {
        let inv = 255 - (*p >> 24);
        let ch = |s: u32, b: u32| (((*p >> s) & 0xff) + b * inv / 255).min(255) << s;
        *p = ch(16, br) | ch(8, bgc) | ch(0, bb);
    }
    unsafe { let _ = BitBlt(dc, x, y, d, d, cv.dc, 0, 0, SRCCOPY); }
}

/// The server logo in the panel's header, px at 100%.
const LOGO_D: f32 = 36.0;

/// Draws the server's logo (a blurple disc with a headset without one) into an opaque DC at
/// (x, y), `d` px, as the strip does.
fn panel_logo(dc: HDC, x: i32, y: i32, d: i32, img: Option<&Image>, icons: &Font, bg: COLORREF, theme: &Theme) {
    let Some(mut cv) = Canvas::new(d, d) else { return };
    let buf = cv.pixels();
    avatars::draw_circle(buf, d, 0.0, 0.0, d as f32, img, brand24("discord.blurple"), 255);
    // onto the panel's background (premultiplied → opaque)
    let (br, bgc, bb) = (bg.0 & 0xff, (bg.0 >> 8) & 0xff, (bg.0 >> 16) & 0xff);
    for p in buf.iter_mut() {
        let inv = 255 - (*p >> 24);
        let ch = |s: u32, b: u32| (((*p >> s) & 0xff) + b * inv / 255).min(255) << s;
        *p = ch(16, br) | ch(8, bgc) | ch(0, bb);
    }
    unsafe { let _ = BitBlt(dc, x, y, d, d, cv.dc, 0, 0, SRCCOPY); }
    if img.is_none() {
        draw_text(dc, icons, GLYPH_HEADSET, COLORREF(theme.color.on_fill.colorref()), x, y, d, d, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    }
}

const SETUP: &str = "Discord only shares voice chat with an app you register yourself (free, one time):\n\
1. Open discord.com/developers/applications → New Application.\n\
2. OAuth2: copy the Client ID; Reset Secret and copy the Client Secret.\n\
3. OAuth2 → Redirects: add http://localhost and save.\n\
4. Right-click this widget → Edit settings… and paste both into \"discord\": client_id, client_secret.\n\
5. Discord then asks once: click Authorize.";

/// Paints the panel. `avatar` gives a member's image if it's ready, `logo` the server's;
/// `hover` is the button under the mouse. `scale` = DPI / 96.
pub fn paint_panel(hwnd: HWND, v: &Voice, avatar: &dyn Fn(&Member) -> Option<Arc<Image>>, logo: Option<&Image>, hover: Option<Button>, scale: f32, look: &Look) {
    let s = |x: f32| (x * scale).round() as i32;
    let theme = Theme::new(look);
    let pal = palette(&theme);
    let cr = |c: Rgb| COLORREF(c.colorref());
    let font = |t: theme::TextStyle, face: &str, q| Font::new(s(t.px), t.weight, face, q);
    let lay = panel_layout(v, scale);
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        let dc = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(dc, bmp);
        SetBkMode(dc, TRANSPARENT);
        fill(dc, rc, pal.bg);

        let q = CLEARTYPE_QUALITY;
        let small = font(theme::CAPTION, &theme.font.text, q);
        let body = font(theme::BODY, &theme.font.text, q);
        let title = font(theme::TITLE, &theme.font.text, q);
        let icons = font(theme::ICON, theme.font.icon, q);
        let pad = s(space::PANEL_PADDING);
        let inner = w - 2 * pad;
        let one = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS;
        let red = colorref(brand24("discord.red"));

        match &v.channel {
            Some(c) => {
                // the server's logo at the top left (as on the strip), the channel over the server beside it
                let ld = s(LOGO_D);
                panel_logo(dc, pad, s(12.0) + (s(40.0) - ld) / 2, ld, logo, &icons, pal.bg, &theme);
                let tx = pad + ld + s(space::M);
                let tw = w - pad - tx - s(70.0);
                let name = if c.name.is_empty() { "Voice" } else { &c.name };
                draw_text(dc, &title, name, pal.fg, tx, s(12.0), tw, s(22.0), DT_LEFT | one);
                let place = c.guild.clone().unwrap_or_else(|| if c.guild_id.is_some() { String::new() } else { "Private call".into() });
                draw_text(dc, &small, &place, pal.dim, tx, s(34.0), tw, s(18.0), DT_LEFT | one);
                let count = format!("{} in call", c.members.len());
                draw_text(dc, &small, &count, pal.dim, pad, s(12.0), inner, s(22.0), DT_RIGHT | one);
                fill(dc, RECT { left: pad, top: s(HEAD_H), right: w - pad, bottom: s(HEAD_H) + s(1.0).max(1) }, pal.line);

                let top = s(HEAD_H) + s(6.0);
                for (i, m) in c.members.iter().take(MAX_ROWS).enumerate() {
                    let y = top + s(ROW_H) * i as i32;
                    let d = s(30.0);
                    panel_avatar(dc, pad, y + (s(ROW_H) - d) / 2, d, avatar(m).as_deref(), m.speaking, pal.bg, scale);
                    let me = v.me.as_deref() == Some(m.id.as_str());
                    let label = if me { format!("{} (you)", m.name) } else { m.name.clone() };
                    let color = if m.deafened { pal.dim } else { pal.fg };
                    draw_text(dc, &body, &label, color, pad + d + s(10.0), y, inner - d - s(70.0), s(ROW_H), DT_LEFT | one);
                    let isz = s(18.0);
                    let iy = y + (s(ROW_H) - isz) / 2;
                    let mut ix = w - pad - isz;
                    if m.deafened {
                        slashed(dc, &icons, GLYPH_HEADPHONE, ix, iy, isz, red, scale);
                        ix -= isz + s(6.0);
                    }
                    if m.muted {
                        slashed(dc, &icons, GLYPH_MIC, ix, iy, isz, red, scale);
                        ix -= isz + s(6.0);
                    }
                    if m.local_mute {
                        draw_text(dc, &icons, "\u{E74F}", pal.dim, ix, iy, isz, isz, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                    }
                }
                if c.members.len() > MAX_ROWS {
                    let y = top + s(ROW_H) * MAX_ROWS as i32;
                    let more = format!("+{} more", c.members.len() - MAX_ROWS);
                    draw_text(dc, &small, &more, pal.dim, pad, y, inner, s(ROW_H), DT_LEFT | one);
                }

                for (b, r) in &lay.buttons {
                    let on = match b {
                        Button::Mute => v.self_mute,
                        Button::Deafen => v.self_deaf,
                        Button::Leave => false,
                    };
                    let hot = hover == Some(*b);
                    let bg = match (b, on, hot) {
                        (Button::Leave, _, false) => cr(theme.status.danger_fill),
                        (Button::Leave, _, true) => cr(theme.status.danger_fill_hover),
                        (_, true, _) => cr(theme.status.danger_soft),
                        (_, false, true) => pal.button_hover,
                        _ => pal.button,
                    };
                    // GDI RoundRect takes the corner's diameter: 2 × the shared control radius
                    round_fill(dc, *r, (2.0 * draw::highlight_shape(look, scale).0) as i32, bg);
                    let (glyph, label) = match (b, on) {
                        (Button::Mute, false) => (GLYPH_MIC, "Mute"),
                        (Button::Mute, true) => (GLYPH_MIC, "Unmute"),
                        (Button::Deafen, false) => (GLYPH_HEADPHONE, "Deafen"),
                        (Button::Deafen, true) => (GLYPH_HEADPHONE, "Undeafen"),
                        (Button::Leave, _) => ("\u{E711}", "Leave"),
                    };
                    let fg = match (b, on) {
                        (Button::Leave, _) => cr(theme.color.on_fill),
                        (_, true) => red,
                        _ => pal.fg,
                    };
                    let (lw, _) = text_size(dc, &small, label);
                    let isz = s(16.0);
                    let total = isz + s(6.0) + lw;
                    let x0 = r.left + (r.right - r.left - total) / 2;
                    let iy = r.top + (r.bottom - r.top - isz) / 2;
                    if on {
                        slashed(dc, &icons, glyph, x0, iy, isz, fg, scale);
                    } else {
                        draw_text(dc, &icons, glyph, fg, x0, iy, isz, isz, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                    }
                    draw_text(dc, &small, label, fg, x0 + isz + s(6.0), r.top, lw + 2, r.bottom - r.top, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
                }
            }
            None => {
                draw_text(dc, &icons, GLYPH_HEADSET, pal.fg, pad, s(14.0), s(20.0), s(22.0), DT_LEFT | DT_SINGLELINE | DT_VCENTER);
                draw_text(dc, &title, "Discord voice", pal.fg, pad + s(28.0), s(14.0), inner, s(22.0), DT_LEFT | one);
                let (msg, body_text) = match &v.status {
                    Status::NotConfigured => ("Not set up yet".to_string(), SETUP.to_string()),
                    Status::Ready => ("Not in a call".into(), "The widget shows the call here when you join a voice channel.".into()),
                    Status::NoDiscord => ("Discord isn't running".into(), "It connects by itself when Discord starts.".into()),
                    Status::Authorizing => ("Authorize in Discord".into(), "Discord shows a popup asking to authorize this app: click Authorize.".into()),
                    Status::NeedsSignIn(why) => ("Signed out".into(), format!("{why}\nRight-click → Sign in to Discord.")),
                    Status::Error(e) => ("Problem".into(), format!("{e}\nCheck client_id / client_secret / redirect_uri (right-click → Edit settings…).")),
                    Status::Connecting => ("Connecting…".into(), String::new()),
                };
                draw_text(dc, &small, &msg, pal.dim, pad, s(40.0), inner, s(18.0), DT_LEFT | one);
                fill(dc, RECT { left: pad, top: s(HEAD_H), right: w - pad, bottom: s(HEAD_H) + s(1.0).max(1) }, pal.line);
                draw_text(dc, &small, &body_text, pal.fg, pad, s(HEAD_H) + s(12.0), inner, h - s(HEAD_H) - s(20.0), DT_LEFT | DT_WORDBREAK);
            }
        }

        let _ = BitBlt(hdc, 0, 0, w, h, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(dc);
        let _ = EndPaint(hwnd, &ps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn in_call(n: usize) -> Voice {
        let mut v = Voice { status: Status::Ready, ..Default::default() };
        let states: Vec<_> = (0..n).map(|i| json!({ "nick": format!("u{i}"), "user": { "id": i.to_string() } })).collect();
        v.set_channel(&json!({ "id": "1", "name": "General", "voice_states": states }));
        v
    }

    #[test]
    fn lines() {
        let mut v = in_call(3);
        assert_eq!(strip_lines(&v), ("General".into(), "3 in call".into()));
        v.self_mute = true;
        assert_eq!(strip_lines(&v).1, "3 in call · Muted");
        v.self_deaf = true;
        assert_eq!(strip_lines(&v).1, "3 in call · Deafened");
        assert_eq!(strip_lines(&Voice::default()).1, "Not set up");
    }

    #[test]
    fn strip_buttons_hit() {
        // 48 px taskbar at 100%: each button's box is 24 × 24 (rows 12..36), as wide as its column
        let lay = StripLayout { size: SIZE { cx: 200, cy: 48 }, buttons_x: 120, button_w: 24, buttons: true, scale: 1.0, ring_x: 4, ring_d: 22 };
        assert_eq!(lay.hit(119, 24), None);
        assert_eq!(lay.hit(120, 24), Some(Button::Mute));
        assert_eq!(lay.hit(150, 24), Some(Button::Deafen));
        assert_eq!(lay.hit(190, 24), Some(Button::Leave));
        assert_eq!(lay.hit(200, 24), None);
        // the logo's ring: x 4..26, rows 13..35
        assert!(lay.on_logo(15, 24) && lay.on_logo(4, 13) && lay.on_logo(25, 34));
        assert!(!lay.on_logo(3, 24) && !lay.on_logo(26, 24) && !lay.on_logo(15, 12) && !lay.on_logo(15, 35));
        assert!(!StripLayout { buttons: false, ..lay }.on_logo(15, 24));
        // above and below the box is the strip itself
        assert_eq!(lay.hit(150, 12), Some(Button::Deafen));
        assert_eq!(lay.hit(150, 11), None);
        assert_eq!(lay.hit(150, 35), Some(Button::Deafen));
        assert_eq!(lay.hit(150, 36), None);
        // cut off while collapsing: the hidden part isn't clickable
        assert_eq!(StripLayout { size: SIZE { cx: 150, cy: 48 }, ..lay }.hit(170, 24), None);
        assert_eq!(StripLayout { buttons: false, ..lay }.hit(130, 24), None);
    }

    #[test]
    fn panel_buttons_hit() {
        let lay = panel_layout(&in_call(2), 1.0);
        assert_eq!(lay.buttons.len(), 3);
        let (b, r) = lay.buttons[2];
        assert_eq!(b, Button::Leave);
        assert_eq!(lay.hit(r.left + 1, r.top + 1), Some(Button::Leave));
        assert_eq!(lay.hit(0, 0), None);
        assert!(r.bottom < lay.h && r.right <= lay.w);
        // more people, taller panel, capped
        assert!(panel_layout(&in_call(5), 1.0).h > lay.h);
        assert_eq!(panel_layout(&in_call(30), 1.0).h, panel_layout(&in_call(40), 1.0).h);
    }
}
