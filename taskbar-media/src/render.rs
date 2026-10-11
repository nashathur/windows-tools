//! Drawing the taskbar strip's content for Block (text on the clock's baselines, icons in the
//! tray-icon box, the cover with its progress ring), the tray logo, and the GDI helpers the panel
//! shares. Block puts the background, hover highlight, brand tint and cover feel under it.

use crate::cover::{self, Cover};
use crate::layout::{self, Strip};
use std::ffi::c_void;
use taskbar_dock::draw;
use taskbar_dock::style::Look;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// Segoe Fluent Icons glyphs (Windows 11's own icon font).
pub mod glyph {
    pub const PREVIOUS: char = '\u{E892}';
    pub const NEXT: char = '\u{E893}';
    pub const PLAY: char = '\u{E768}';
    pub const PAUSE: char = '\u{E769}';
    pub const REWIND: char = '\u{EB9E}';
    pub const FORWARD: char = '\u{EB9D}';
    pub const MUSIC: char = '\u{E8D6}';
    pub const MUTED: char = '\u{E74F}';
    /// A speaker with 0 to 3 sound waves.
    pub const VOLUME: [char; 4] = ['\u{E992}', '\u{E993}', '\u{E994}', '\u{E995}'];
}
pub const ICON_FONT: &str = taskbar_dock::theme::ICON_FONT;

pub struct Font(pub HFONT);
impl Font {
    pub fn new(px: i32, weight: i32, face: &str) -> Font {
        let mut lf = LOGFONTW { lfHeight: -px.max(1), lfWeight: weight, lfQuality: ANTIALIASED_QUALITY, ..Default::default() };
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
pub struct Canvas {
    pub dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    pub w: i32,
    pub h: i32,
}
impl Canvas {
    pub fn new(w: i32, h: i32) -> Option<Canvas> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w.max(1),
                    biHeight: -h.max(1),
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
            Some(Canvas { dc, bmp, old, bits: bits as *mut u32, w: w.max(1), h: h.max(1) })
        }
    }
    pub fn pixels(&mut self) -> &mut [u32] {
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

/// Copies `src` (`sw` wide) into Block's canvas `dst` (`dw` wide), top left, as far as both reach.
pub fn copy_into(dst: &mut [u32], dw: i32, src: &[u32], sw: i32) {
    let (dh, sh) = (dst.len() as i32 / dw.max(1), src.len() as i32 / sw.max(1));
    let w = dw.min(sw).max(0) as usize;
    for y in 0..dh.min(sh) {
        let (d, s) = ((y * dw) as usize, (y * sw) as usize);
        dst[d..d + w].copy_from_slice(&src[s..s + w]);
    }
}

pub fn text_size(dc: HDC, font: &Font, s: &str) -> (i32, i32) {
    if s.is_empty() {
        return (0, 0); // never hand Windows an empty text (see draw_text)
    }
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut size = SIZE::default();
    unsafe {
        let old = SelectObject(dc, font.0);
        let _ = GetTextExtentPoint32W(dc, &w, &mut size);
        SelectObject(dc, old);
    }
    (size.cx, size.cy)
}

/// Distance from the top of a line of text to its baseline.
pub fn ascent(dc: HDC, font: &Font) -> i32 {
    let mut tm = TEXTMETRICW::default();
    unsafe {
        let old = SelectObject(dc, font.0);
        let _ = GetTextMetricsW(dc, &mut tm);
        SelectObject(dc, old);
    }
    tm.tmAscent
}

/// Draws one line of text. Empty text draws nothing: DrawTextW given an empty text (a
/// zero-length buffer, whose pointer points nowhere) with DT_END_ELLIPSIS crashed the widget
/// inside USER32, e.g. when a new Firefox tab briefly reported no artist.
pub fn draw_text(dc: HDC, font: &Font, s: &str, color: COLORREF, rc: RECT, flags: DRAW_TEXT_FORMAT) {
    if s.is_empty() {
        return;
    }
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    let mut rc = rc;
    unsafe {
        let old = SelectObject(dc, font.0);
        SetTextColor(dc, color);
        DrawTextW(dc, &mut buf, &mut rc, flags | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

/// `s` cut to fit `max` px with "…" (whole characters), and its width.
pub fn fit(dc: HDC, font: &Font, s: &str, max: i32) -> (String, i32) {
    let (w, _) = text_size(dc, font, s);
    if w <= max {
        return (s.to_string(), w);
    }
    let chars: Vec<char> = s.chars().collect();
    let cut = |n: usize| format!("{}…", chars[..n].iter().collect::<String>().trim_end());
    // the longest prefix that fits, by halving
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if text_size(dc, font, &cut(mid)).0 <= max { lo = mid } else { hi = mid - 1 }
    }
    let t = cut(lo);
    let w = text_size(dc, font, &t).0;
    (t, w)
}

/// What the strip shows.
#[derive(Clone, PartialEq, Debug)]
pub struct StripContent {
    pub title: String,
    pub artist: String,
    /// The cover, or else the app's or site's logo.
    pub cover: Option<crate::media::CoverRef>,
    /// No picture at all: a badge in the brand's colour with a short label (else a note).
    pub badge: Option<((u32, u32, u32), String)>,
    /// The three buttons: glyph, and whether the app supports it.
    pub buttons: [(char, bool); 3],
    pub ring: Ring,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ring {
    /// 0..1 along.
    At(f32),
    /// No position to show (Firefox while playing) or a live stream (red): a full ring, its
    /// brightness `level` (0..1) pulsing slowly while playing (taskbar_dock::anim::pulse).
    Full { level: f32, live: bool },
}

/// Rows (top, bottom; bottom exclusive) of the visible pixels of `text` in `font`, within its
/// text cell.
fn ink_rows(font: &Font, text: &str) -> (i32, i32) {
    let measure = unsafe { CreateCompatibleDC(None) };
    let (w, h) = text_size(measure, font, text);
    unsafe { let _ = DeleteDC(measure); }
    let Some(mut cv) = Canvas::new(w, h) else { return (0, h) };
    draw_text(cv.dc, font, text, rgb(255, 255, 255), RECT { left: 0, top: 0, right: w, bottom: h }, DT_LEFT | DT_TOP);
    let (cw, ch) = (cv.w, cv.h);
    draw::ink_bounds(cv.pixels(), cw, ch).map_or((0, h), |(_, t, _, b)| (t, b))
}

/// A `size` px square in a brand colour with a short label centred on it (black or white,
/// whichever reads better), bold in the shared font: shown when there's no picture.
pub fn badge_tile(color: (u32, u32, u32), label: &str, size: i32, look: &Look, scale: f32) -> Vec<u32> {
    let Some(mut cv) = Canvas::new(size, size) else { return vec![0; (size * size).max(0) as usize] };
    let bg = rgb(color.0 as u8, color.1 as u8, color.2 as u8);
    let text = taskbar_dock::badge::contrast_text(bg.0);
    unsafe {
        let brush = CreateSolidBrush(bg);
        FillRect(cv.dc, &RECT { left: 0, top: 0, right: size, bottom: size }, brush);
        let _ = DeleteObject(brush);
    }
    // the label as large as fits: about 40 % of the tile for two letters
    let font = Font::new(((size as f32 * 0.42).max(8.0 * scale)).round() as i32, 700, &look.font);
    draw_text(cv.dc, &font, label, COLORREF(text), RECT { left: 0, top: 0, right: size, bottom: size }, DT_CENTER | DT_VCENTER);
    cv.pixels().iter().map(|p| p | 0xff00_0000).collect()
}

/// Copies `c` scaled to `size` into the premultiplied buffer at (x, y), opaque.
fn paint_cover(buf: &mut [u32], width: i32, c: &Cover, x: i32, y: i32, size: i32) {
    let px = cover::resample(&c.pixels, c.size, (0, 0, c.size), size);
    let height = buf.len() as i32 / width.max(1);
    for dy in 0..size {
        for dx in 0..size {
            let (bx, by) = (x + dx, y + dy);
            if bx >= 0 && by >= 0 && bx < width && by < height {
                buf[(by * width + bx) as usize] = px[(dy * size + dx) as usize];
            }
        }
    }
}

/// The strip's vertical geometry for a taskbar `height` px tall at `scale` (display × drag
/// zoom), from the shared font's ink: the cover, its ring's margin, the text lines.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Geo {
    pub height: i32,
    pub scale: f32,
    /// The cover's size, and the even margin around its ring inside the hover highlight (also
    /// the ring's distance from the strip's left edge).
    pub cover: i32,
    pub margin: i32,
    pub cover_top: i32,
    /// Tops of the title and artist lines (on the taskbar clock's baselines), and a line's height.
    pub top1: i32,
    pub top2: i32,
    pub line_h: i32,
}

pub fn geo(height: i32, scale: f32, look: &Look) -> Geo {
    let s = |v: f32| (v * scale).round() as i32;
    // the shared style: the taskbar clock's font and size
    let text_font = Font::new(s(look.font_size), 400, &look.font);
    let measure = unsafe { CreateCompatibleDC(None) };
    let (_, line_h) = text_size(measure, &text_font, "Ag");
    let asc = ascent(measure, &text_font);
    unsafe { let _ = DeleteDC(measure); }
    let (top1, top2) = draw::two_line_tops(height, scale, asc, asc);
    // the cover is as tall as both lines (the top of the title's capitals to the bottom of the
    // artist's descenders); its ring sits in the middle of the hover highlight, as far from
    // its left edge as from its top and bottom
    let (cap_top, _) = ink_rows(&text_font, "H");
    let (_, desc_bottom) = ink_rows(&text_font, "g");
    let (_, inset) = draw::highlight_shape(look, scale);
    let (cover, margin) = layout::margin((top2 + desc_bottom) - (top1 + cap_top), height - 2 * inset, scale);
    let cover_top = inset + margin + layout::ring_around(scale);
    Geo { height, scale, cover, margin, cover_top, top1, top2, line_h }
}

/// The layout: full (the text column `max_width` px at 100%, always that wide, so the buttons
/// and the widgets beside it don't move when the track changes) or `compact`; `fit` (Block's
/// [`taskbar_dock::block::Frame::fit_width`]): the full strip narrowed to that width, the text
/// column first.
pub fn strip_layout(g: &Geo, compact: bool, max_width: f32, fit: Option<i32>) -> Strip {
    let text_w = (max_width * g.scale).round() as i32;
    let mut l = match fit {
        Some(w) if !compact => layout::at_width(g.scale, g.cover, g.margin, text_w, w),
        _ => layout::strip(g.scale, g.cover, g.margin, compact, text_w),
    };
    l.height = g.height; // for hit testing: a button is hit inside its highlight box
    l.scale = g.scale;
    l
}

/// The cover with its ring, as drawn (Block's cover box: it grows on hover, dips on press).
pub fn ring_box(l: &Strip, g: &Geo) -> (i32, i32, i32, i32) {
    let top = g.cover_top - (l.cover_x - l.ring_x);
    (l.ring_x, top, l.ring_x + l.ring, top + l.ring)
}

/// Draws the strip's content laid out as `l` into Block's canvas `out` (`l.width` wide):
/// text, buttons, the cover (or badge, or a note) and the progress ring; `live` is the ring's
/// colour for a live stream.
pub fn draw_strip(out: &mut [u32], out_w: i32, c: &StripContent, l: &Strip, g: &Geo, look: &Look, live: (u32, u32, u32)) {
    let (height, scale) = (g.height, g.scale);
    let s = |v: f32| (v * scale).round() as i32;
    let text_font = Font::new(s(look.font_size), 400, &look.font);
    let (icon_top, icon) = draw::icon_box(height, scale);
    let icon_font = Font::new(icon, 400, ICON_FONT);
    let Some(mut cv) = Canvas::new(l.width, height) else { return };
    let white = rgb(255, 255, 255);

    if l.text_w > 0 {
        let (title, artist) = (fit(cv.dc, &text_font, &c.title, l.text_w).0, fit(cv.dc, &text_font, &c.artist, l.text_w).0);
        let flags = DT_LEFT | DT_TOP;
        // no artist: the title alone, centred between the two lines
        let t1 = if artist.trim().is_empty() { (g.top1 + g.top2) / 2 } else { g.top1 };
        draw_text(cv.dc, &text_font, &title, white, RECT { left: l.text_x, top: t1, right: l.text_x + l.text_w, bottom: t1 + g.line_h }, flags);
        // the artist line dimmer (78%, like the shared second-line colour), to set it apart
        draw_text(cv.dc, &text_font, &artist, rgb(199, 199, 199), RECT { left: l.text_x, top: g.top2, right: l.text_x + l.text_w, bottom: g.top2 + g.line_h }, flags);
    }
    // buttons: tray-icon sized glyphs centred in their columns; unsupported ones dimmed
    let off = (taskbar_dock::theme::Theme::new(look).strip.disabled * 255.0).round() as u8;
    for (i, (gl, enabled)) in c.buttons.iter().enumerate() {
        let (bl, br) = l.button(i);
        let col = if *enabled { white } else { rgb(off, off, off) };
        draw_text(cv.dc, &icon_font, &gl.to_string(), col, RECT { left: bl, top: icon_top, right: br, bottom: icon_top + icon }, DT_CENTER | DT_VCENTER);
    }
    let (cover, cover_top) = (g.cover, g.cover_top);
    if c.cover.is_none() && c.badge.is_none() {
        // no picture and no brand: a tray-icon sized note in the middle of the cover's place
        let t = cover_top + (cover - icon) / 2;
        draw_text(cv.dc, &icon_font, &glyph::MUSIC.to_string(), white, RECT { left: l.cover_x, top: t, right: l.cover_x + cover, bottom: t + icon }, DT_CENTER | DT_VCENTER);
    }

    let (fg, _) = look.text_rgb();
    let (width, hl_radius) = (l.width, draw::highlight_shape(look, scale).0);
    let corner = hl_radius as i32; // same corner as the hover highlight
    let px = cv.pixels();
    draw::text_to_color(px, width, height, |_| fg);
    if let Some(pic) = &c.cover {
        paint_cover(px, width, &pic.0, l.cover_x, cover_top, cover);
        draw::round_rect(px, width, (l.cover_x, cover_top, l.cover_x + cover, cover_top + cover), corner as f32);
    } else if let Some((color, label)) = &c.badge {
        let tile = badge_tile(*color, label, cover, look, scale);
        paint_cover(px, width, &Cover { size: cover, pixels: tile }, l.cover_x, cover_top, cover);
        draw::round_rect(px, width, (l.cover_x, cover_top, l.cover_x + cover, cover_top + cover), corner as f32);
    }
    // the progress ring around the cover: taskbar text colour (white on a dark taskbar), its
    // unfilled part faint; without a position it's full and pulses; a live stream's is red
    let around = l.cover_x - l.ring_x;
    let (rx, ry, rs) = (l.ring_x as f32, (cover_top - around) as f32, l.ring as f32);
    let ring_rc = (rx, ry, rx + rs, ry + rs);
    let (thick, radius) = (s(layout::RING_WIDTH) as f32, (corner + around) as f32);
    match c.ring {
        Ring::At(f) => draw::progress_ring(px, width, ring_rc, thick, radius, f, (fg.0, fg.1, fg.2, 255), (fg.0, fg.1, fg.2, 64)),
        Ring::Full { level, live: is_live } => {
            let (r, g, b) = if is_live { live } else { fg };
            // 35% at its dimmest, 90% at its brightest
            let a = (90.0 + 140.0 * level.clamp(0.0, 1.0)).round() as u32;
            draw::progress_ring(px, width, ring_rc, thick, radius, 1.0, (r, g, b, a), (0, 0, 0, 0));
        }
    }
    copy_into(out, out_w, px, width);
}

/// The tray icon: the play glyph (Segoe Fluent), mono in the taskbar's text colour, as weather's
/// cloud and discord's headset: a static logo, not the track.
pub fn tray_logo(out: &mut [u32], w: i32, h: i32, look: &Look) -> bool {
    let n = 96;
    let Some(mut big) = Canvas::new(n, n) else { return false };
    let font = Font::new(taskbar_dock::theme::GLYPH_RASTER_PX, 400, ICON_FONT);
    draw_text(big.dc, &font, &glyph::PLAY.to_string(), rgb(255, 255, 255), RECT { left: 0, top: 0, right: n, bottom: n }, DT_CENTER | DT_VCENTER);
    let src = big.pixels().to_vec();
    draw::fit_icon(out, w, 0, 0, w.min(h), &src, n, n);
    let (fg, _) = look.text_rgb();
    draw::text_to_color(out, w, h, |_| fg);
    true
}
