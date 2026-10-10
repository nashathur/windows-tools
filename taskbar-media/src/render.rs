//! Drawing the taskbar strip into its layered window (per-pixel alpha), with the shared drawing
//! rules from taskbar_dock::draw: text on the clock's baselines, icons in the tray-icon box.

use crate::cover::{self, Cover};
use crate::layout::{self, Part, Strip};
use std::ffi::c_void;
use taskbar_dock::draw;
use taskbar_dock::style::Look;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

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
    /// The brand colour behind the strip and how strong (0..1 opacity), fading with playback.
    pub tint: Option<((u32, u32, u32), f32)>,
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

/// Paints a colour at `alpha` (0..1) under what's in the strip's buffer, over the whole strip in
/// the hover highlight's shape (`radius`, `inset` from top and bottom), so text and icons stay
/// on top.
fn tint_under(buf: &mut [u32], width: i32, height: i32, rgb: (u32, u32, u32), alpha: f32, radius: f32, inset: i32) {
    draw::tint_box(buf, width, (0, inset, width, height - inset), rgb, alpha, radius);
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

/// Draws the strip: `height` is the taskbar's, `scale` = display scale × drag zoom, `opacity` 0..1
/// for fading in and out, `floating`: not in the taskbar, so it brings its own backdrop.
/// `width`: draw the full strip narrowed to this many px (collapse/expand animation; see
/// `layout::at_width`). `grow`: the cover's hover / press factor (`anim::Feel`, 1.0 = at rest).
/// Returns the layout (for its size and for hit testing).
#[allow(clippy::too_many_arguments)]
pub fn update_strip(hwnd: HWND, c: &StripContent, height: i32, scale: f32, look: &Look, hover: Option<Part>,
                    compact: bool, max_width: f32, width: Option<i32>, live: (u32, u32, u32), opacity: f32, floating: bool, grow: f32) -> Strip {
    let s = |v: f32| (v * scale).round() as i32;
    // the shared style: the taskbar clock's font and size
    let text_font = Font::new(s(look.font_size), 400, &look.font);
    let (icon_top, icon) = draw::icon_box(height, scale);
    let icon_font = Font::new(icon, 400, ICON_FONT);

    let measure = unsafe { CreateCompatibleDC(None) };
    let (title, _) = fit(measure, &text_font, &c.title, s(max_width));
    let (artist, _) = fit(measure, &text_font, &c.artist, s(max_width));
    let (_, h1) = text_size(measure, &text_font, "Ag");
    let h2 = h1;
    let asc = ascent(measure, &text_font);
    unsafe { let _ = DeleteDC(measure); }

    // on the taskbar clock's baselines: title on the time's, artist on the date's
    let (top1, top2) = draw::two_line_tops(height, scale, asc, asc);
    // the cover is as tall as both lines (the top of the title's capitals to the bottom of the
    // artist's descenders); its ring sits in the middle of the hover highlight, as far from
    // its left edge as from its top and bottom
    let (cap_top, _) = ink_rows(&text_font, "H");
    let (_, desc_bottom) = ink_rows(&text_font, "g");
    let (hl_radius, inset) = draw::highlight_shape(look, scale);
    let (cover, margin) = layout::margin((top2 + desc_bottom) - (top1 + cap_top), height - 2 * inset, scale);
    let cover_top = inset + margin + layout::ring_around(scale);

    // a static strip: the text column is always the full maximum width, so the strip (and the
    // buttons, and the widgets next to it) never move when the track changes
    let text_w = s(max_width);
    let mut l = match width {
        Some(w) if !compact => layout::at_width(scale, cover, margin, text_w, w),
        _ => layout::strip(scale, cover, margin, compact, text_w),
    };
    l.height = height; // for hit testing: a button is hit inside its highlight box
    l.scale = scale;
    // narrowed (collapse/expand): the lines end with "…" at the narrower column
    let (title, artist) = if l.text_w > 0 && l.text_w < text_w {
        let dc = unsafe { CreateCompatibleDC(None) };
        let cut = (fit(dc, &text_font, &c.title, l.text_w).0, fit(dc, &text_font, &c.artist, l.text_w).0);
        unsafe { let _ = DeleteDC(dc); }
        cut
    } else {
        (title, artist)
    };
    let Some(mut cv) = Canvas::new(l.width, height) else { return l };
    let white = rgb(255, 255, 255);

    if l.text_w > 0 {
        let flags = DT_LEFT | DT_TOP;
        // no artist: the title alone, centred between the two lines
        let t1 = if artist.trim().is_empty() { (top1 + top2) / 2 } else { top1 };
        draw_text(cv.dc, &text_font, &title, white, RECT { left: l.text_x, top: t1, right: l.text_x + l.text_w, bottom: t1 + h1 }, flags);
        // the artist line dimmer (78%, like the shared second-line colour), to set it apart
        draw_text(cv.dc, &text_font, &artist, rgb(199, 199, 199), RECT { left: l.text_x, top: top2, right: l.text_x + l.text_w, bottom: top2 + h2 }, flags);
    }
    // buttons: tray-icon sized glyphs centred in their columns; unsupported ones dimmed
    for (i, (g, enabled)) in c.buttons.iter().enumerate() {
        let (bl, br) = l.button(i);
        let off = (taskbar_dock::theme::Theme::new(look).strip.disabled * 255.0).round() as u8;
        let col = if *enabled { white } else { rgb(off, off, off) };
        draw_text(cv.dc, &icon_font, &g.to_string(), col, RECT { left: bl, top: icon_top, right: br, bottom: icon_top + icon }, DT_CENTER | DT_VCENTER);
    }
    if c.cover.is_none() && c.badge.is_none() {
        // no picture and no brand: a tray-icon sized note in the middle of the cover's place
        let note = glyph::MUSIC.to_string();
        let t = cover_top + (cover - icon) / 2;
        draw_text(cv.dc, &icon_font, &note, white, RECT { left: l.cover_x, top: t, right: l.cover_x + cover, bottom: t + icon }, DT_CENTER | DT_VCENTER);
    }

    let (fg, _) = look.text_rgb();
    let width = l.width;
    let corner = hl_radius as i32; // same corner as the hover highlight
    let px = cv.pixels();
    draw::text_to_color(px, width, height, |_| fg);
    if let Some(pic) = &c.cover {
        paint_cover(px, width, &pic.0, l.cover_x, cover_top, cover);
        draw::round_rect(px, width, (l.cover_x, cover_top, l.cover_x + cover, cover_top + cover), corner as f32);
    } else if let Some((color, label)) = &c.badge {
        let tile = badge_tile(*color, label, cover, look, scale);
        let pic = Cover { size: cover, pixels: tile };
        paint_cover(px, width, &pic, l.cover_x, cover_top, cover);
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
    // hovered / pressed: the cover and its ring grow / dip about their centre
    draw::grow_pixels(px, width, (l.ring_x, cover_top - around, l.ring_x + l.ring, cover_top - around + l.ring), grow);
    // the brand tint behind everything, in the highlight's shape
    if let Some((rgb, alpha)) = c.tint {
        tint_under(px, width, height, rgb, alpha, hl_radius, inset);
    }
    // hover highlight like the taskbar's buttons: one button, or the whole strip
    let hl = match hover {
        Some(Part::Button(i)) => Some(draw::button_box(l.button(i), height, scale)),
        Some(Part::Body | Part::Cover) => Some((0, inset, width, height - inset)),
        None => None,
    };
    draw::background_rect(px, width, height, hl, look.light, hl_radius);
    // floating over other windows: a taskbar-coloured backdrop keeps the text readable, in the
    // highlight's shape (inset, so it leaves the taskbar's top border alone)
    if floating {
        let bg = if look.light { (238, 238, 238) } else { (32, 32, 32) };
        tint_under(px, width, height, bg, 0.96, hl_radius, inset);
    }

    let size = SIZE { cx: width, cy: height };
    let src = POINT::default();
    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: (opacity.clamp(0.0, 1.0) * 255.0).round() as u8, AlphaFormat: AC_SRC_ALPHA as u8 };
    unsafe {
        let _ = UpdateLayeredWindow(hwnd, None, None, Some(&size), cv.dc, Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
    }
    l
}
