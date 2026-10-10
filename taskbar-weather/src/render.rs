//! GDI drawing: the tray icon, the taskbar label (per-pixel alpha) and the forecast flyout.

use crate::weather::{deg, describe, Weather};
use taskbar_dock::{draw, emoji};
use taskbar_dock::style::Look;
use taskbar_dock::theme::{self, font_override, size, space, Rgb, Theme};
use std::ffi::c_void;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
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
            SetTextColor(dc, rgb(255, 255, 255));
            Some(Canvas { dc, bmp, old, bits: bits as *mut u32, w, h })
        }
    }
    fn pixels(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize) }
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

/// Distance from the top of a line of text to its baseline.
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
        DrawTextW(dc, &mut buf, &mut rc, flags | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

/// Tray icon showing the temperature as text, e.g. "31°".
pub fn tray_icon(text: &str, size: i32, light: bool) -> Option<HICON> {
    let mut cv = Canvas::new(size, size)?;
    // Largest font whose text fits the icon; drop the degree sign if it gets too small.
    let mut chosen = None;
    for candidate in [text.to_string(), text.trim_end_matches('°').to_string()] {
        for px in (size / 2..=size).rev() {
            let f = Font::new(px, 600, font_override::WEATHER_TRAY, ANTIALIASED_QUALITY);
            let (w, _) = text_size(cv.dc, &f, &candidate);
            if w <= size {
                if px >= size * 7 / 10 || chosen.is_none() {
                    chosen = Some((candidate.clone(), f, px));
                }
                break;
            }
        }
        if matches!(chosen, Some((_, _, px)) if px >= size * 7 / 10) {
            break;
        }
    }
    let (s, font, _) = chosen?;
    draw_text(cv.dc, &font, &s, rgb(255, 255, 255), -2, 0, size + 4, size, DT_CENTER | DT_VCENTER);
    let fg = if light { (0, 0, 0) } else { (255, 255, 255) };
    // Icons take straight (non-premultiplied) alpha.
    for p in cv.pixels().iter_mut() {
        let a = ((*p >> 16) & 0xFF).max((*p >> 8) & 0xFF).max(*p & 0xFF);
        *p = a << 24 | (fg.0 as u32) << 16 | (fg.1 as u32) << 8 | fg.2 as u32;
    }
    unsafe {
        let mask_bits = vec![0u8; ((size + 15) / 16 * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr() as *const c_void));
        let info = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: cv.bmp };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(mask);
        icon
    }
}

pub struct LabelContent {
    pub glyph: String,
    /// Temperature, with the humidity (dimmer) under it at the same x.
    pub line1: String,
    pub line2: String,
    /// The conditions beside the two lines (empty in compact mode): over `desc2` (dimmer, "Feels
    /// 31°") on the same baselines, or vertically centred without it.
    pub desc: String,
    pub desc2: String,
}

/// Draws the taskbar label into a layered window with per-pixel alpha. Returns its size.
/// `scale` is the DPI scale times the drag zoom (the label grows a little while dragged);
/// `height` stays the taskbar's. Font, colours and shapes come from the shared look.
/// `clip`: cut the label off at this width (collapse/expand animation; the text fades out at
/// the cut). `measure_only`: only return the size, draw nothing.
#[allow(clippy::too_many_arguments)]
pub fn update_label(hwnd: HWND, content: &LabelContent, height: i32, scale: f32, look: &Look, hover: bool, clip: Option<i32>, measure_only: bool) -> SIZE {
    let s = |v: f32| (v * scale).round() as i32;
    // the weather symbol, drawn large and then fitted into the tray-icon box (shared rule), so
    // it has the same size and height as the tray's icons whatever the glyph's own spacing
    let glyph_font = Font::new(theme::GLYPH_RASTER_PX, 400, Theme::new(look).font.emoji, ANTIALIASED_QUALITY);
    // the shared style: the taskbar clock's font and size, regular weight
    let f1 = Font::new(s(look.font_size), 400, &look.font, ANTIALIASED_QUALITY);
    let f2 = Font::new(s(look.font_size), 400, &look.font, ANTIALIASED_QUALITY);

    let measure = unsafe { CreateCompatibleDC(None) };
    let (w1, h1) = text_size(measure, &f1, &content.line1);
    // compact: no second line (never measure or draw empty text)
    let one_line = content.line2.is_empty();
    let (w2, h2) = if one_line { (0, 0) } else { text_size(measure, &f2, &content.line2) };
    let (w3, h3) = if content.desc.is_empty() { (0, 0) } else { text_size(measure, &f1, &content.desc) };
    // the feels-like line under the conditions (only with them, and only when there are two lines)
    let two_desc = w3 > 0 && !one_line && !content.desc2.is_empty();
    let (w4, h4) = if two_desc { text_size(measure, &f2, &content.desc2) } else { (0, 0) };
    let (a1, a2) = (ascent(measure, &f1), ascent(measure, &f2));
    unsafe { let _ = DeleteDC(measure); }

    let pad = s(8.0);
    let gap = s(6.0);
    let (icon_top, gw) = draw::icon_box(height, scale);
    let col = w1.max(w2);
    let desc_gap = if w3 > 0 { s(10.0) } else { 0 };
    let desc_w = w3.max(w4);
    let natural = pad + gw + gap + col + desc_gap + desc_w + pad;
    if measure_only {
        return SIZE { cx: natural, cy: height };
    }
    let width = clip.map_or(natural, |c| c.clamp(1, natural));
    let Some(mut cv) = Canvas::new(width, height) else { return SIZE::default() };
    let white = rgb(255, 255, 255);
    // in colour (drawn over the text's colours below); one-colour GDI only without Direct2D
    let colour_glyph = emoji::render(&content.glyph, theme::GLYPH_RASTER_PX as f32, 96, 96);
    let mono = if colour_glyph.is_none() { Canvas::new(96, 96) } else { None };
    if let Some(mut big) = mono {
        draw_text(big.dc, &glyph_font, &content.glyph, white, 0, 0, 96, 96, DT_CENTER | DT_VCENTER);
        unsafe { let _ = GdiFlush(); }
        let src = big.pixels().to_vec();
        unsafe { let _ = GdiFlush(); }
        draw::fit_icon(cv.pixels(), width, pad, icon_top, gw, &src, 96, 96);
    }
    let tx = pad + gw + gap;
    // on the clock's baselines
    let (top1, top2) = if one_line { ((height - h1 + 1) / 2, 0) } else { draw::two_line_tops(height, scale, a1, a2) };
    // temperature over humidity at the same x; the conditions beside them, centred
    draw_text(cv.dc, &f1, &content.line1, white, tx, top1, col, h1, DT_LEFT | DT_TOP);
    if !one_line {
        draw_text(cv.dc, &f2, &content.line2, white, tx, top2, col, h2, DT_LEFT | DT_TOP);
    }
    let dx = tx + col + desc_gap;
    if two_desc {
        // conditions over feels-like, on the same baselines as temperature over humidity
        draw_text(cv.dc, &f1, &content.desc, white, dx, top1, w3, h3, DT_LEFT | DT_TOP);
        draw_text(cv.dc, &f2, &content.desc2, white, dx, top2, w4, h4, DT_LEFT | DT_TOP);
    } else if w3 > 0 {
        draw_text(cv.dc, &f1, &content.desc, white, dx, (height - h3 + 1) / 2, w3, h3, DT_LEFT | DT_TOP);
    }
    if width < natural {
        // cut off while collapsing or expanding: the text fades out at the right end
        unsafe { let _ = GdiFlush(); }
        taskbar_dock::marquee::fade_edges(cv.pixels(), width, (tx, 0, width, height), s(12.0), false, true, 0);
    }

    // coverage into colour: the humidity and feels-like line dimmer
    let (fg, dim) = look.text_rgb();
    let hum = if one_line { (0, 0, 0, 0) } else { (tx, top2, dx, top2 + h2) };
    let feels = if two_desc { (dx, top2, dx + w4, top2 + h4) } else { (0, 0, 0, 0) };
    let inside = |r: (i32, i32, i32, i32), x: i32, y: i32| x >= r.0 && x < r.2 && y >= r.1 && y < r.3;
    for (i, p) in cv.pixels().iter_mut().enumerate() {
        let (x, y) = (i as i32 % width, i as i32 / width);
        let c = if inside(hum, x, y) || inside(feels, x, y) { dim } else { fg };
        let a = ((*p >> 16) & 0xff).max((*p >> 8) & 0xff).max(*p & 0xff);
        *p = a << 24 | (c.0 * a / 255) << 16 | (c.1 * a / 255) << 8 | (c.2 * a / 255);
    }
    if let Some(img) = &colour_glyph {
        emoji::fit_over(cv.pixels(), width, pad, icon_top, gw, img);
    }
    // background with the hover highlight, like the taskbar's buttons (shared look)
    let (radius, inset) = draw::highlight_shape(look, scale);
    draw::background(cv.pixels(), width, height, hover, look.light, radius, inset);

    let size = SIZE { cx: width, cy: height };
    let src = POINT::default();
    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
    unsafe {
        let _ = UpdateLayeredWindow(hwnd, None, None, Some(&size), cv.dc, Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
    }
    size
}
struct Palette {
    bg: COLORREF,
    fg: COLORREF,
    dim: COLORREF,
    line: COLORREF,
    rain: COLORREF,
    track: COLORREF,
    bar: COLORREF,
}

/// The theme's colours as GDI COLORREFs; rain and the temperature bar are brand entries.
fn palette(t: &Theme) -> Palette {
    let cr = |c: Rgb| COLORREF(c.colorref());
    Palette { bg: cr(t.color.bg), fg: cr(t.color.fg), dim: cr(t.color.dim), line: cr(t.color.line),
              rain: cr(t.brand("weather.rain")), track: cr(t.color.track), bar: cr(t.brand("weather.temp_bar")) }
}

/// Rain chance (%) shown from this on, and in the rain colour from `RAIN_LIKELY` (below: dim).
const RAIN_SHOWN: f64 = 10.0;
const RAIN_LIKELY: f64 = 50.0;

/// The colour for a rain chance of `p` %.
fn rain_color(pal: &Palette, p: f64) -> COLORREF {
    if p >= RAIN_LIKELY { pal.rain } else { pal.dim }
}

pub const POPUP_W: i32 = size::PANEL_WIDE as i32;
pub const POPUP_H: i32 = 494;

fn fill(dc: HDC, x: i32, y: i32, w: i32, h: i32, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        FillRect(dc, &RECT { left: x, top: y, right: x + w, bottom: y + h }, b);
        let _ = DeleteObject(b);
    }
}

fn round_fill(dc: HDC, x: i32, y: i32, w: i32, h: i32, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        let p = CreatePen(PS_SOLID, 1, c);
        let ob = SelectObject(dc, b);
        let op = SelectObject(dc, p);
        let _ = RoundRect(dc, x, y, x + w, y + h, h, h);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        let _ = DeleteObject(b);
        let _ = DeleteObject(p);
    }
}

/// An emoji in colour in the panel: `text` at `px` in the box (x, y, w, h), centred, or with
/// its visible left edge at x for DT_LEFT `flags`, on the panel's background `bg`. Without
/// Direct2D: GDI in `font`, one colour (`fg`).
#[allow(clippy::too_many_arguments)]
fn draw_emoji(dc: HDC, font: &Font, text: &str, px: i32, fg: COLORREF, bg: Rgb, x: i32, y: i32, w: i32, h: i32, flags: DRAW_TEXT_FORMAT) {
    let Some(img) = emoji::render(text, px as f32, w, h) else {
        return draw_text(dc, font, text, fg, x, y, w, h, flags);
    };
    let left = (flags & (DT_CENTER | DT_RIGHT)).0 == 0;
    let shift = if left { emoji::ink(&img).map_or(0, |b| b.0) } else { 0 };
    let Some(mut cv) = Canvas::new(w, h) else { return };
    cv.pixels().copy_from_slice(&emoji::flatten(&img, bg));
    unsafe { let _ = BitBlt(dc, x - shift, y, w, h, cv.dc, 0, 0, SRCCOPY); }
}

/// Paints the forecast flyout. `scale` = DPI / 96.
pub fn paint_popup(hwnd: HWND, weather: Option<&Weather>, status: Option<&str>, scale: f32, look: &Look) {
    let s = |v: f32| (v * scale).round() as i32;
    let theme = Theme::new(look);
    let pal = palette(&theme);
    let (text, emoji_face) = (theme.font.text.as_str(), theme.font.emoji);
    let font = |t: theme::TextStyle, face: &str, q| Font::new(s(t.px), t.weight, face, q);
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        // Double buffer to avoid flicker.
        let dc = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(dc, bmp);
        SetBkMode(dc, TRANSPARENT);
        fill(dc, 0, 0, w, h, pal.bg);

        let q = CLEARTYPE_QUALITY;
        let small = font(theme::CAPTION, text, q);
        let small_b = font(theme::CAPTION_STRONG, text, q);
        let body = font(theme::BODY, text, q);
        let body_b = font(theme::TITLE, text, q);
        let big = font(theme::DISPLAY, text, q);
        let emoji_big = font(theme::EMOJI_LARGE, emoji_face, q);
        let emoji = font(theme::EMOJI, emoji_face, q);
        let pad = s(space::PANEL_PADDING);
        let inner = w - 2 * pad;
        let l = DT_LEFT | DT_VCENTER;
        let r = DT_RIGHT | DT_VCENTER;
        let c = DT_CENTER | DT_VCENTER;

        match weather {
            None => {
                let msg = status.unwrap_or("Loading weather…");
                draw_text(dc, &body, msg, pal.dim, pad, 0, inner, h, c);
            }
            Some(wx) => {
                let (glyph, desc) = describe(wx.code, wx.is_day);
                draw_text(dc, &small_b, &wx.location, pal.dim, pad, s(14.0), inner * 2 / 3, s(20.0), l | DT_END_ELLIPSIS);
                let upd = match (status, &wx.observed, &wx.obs_error) {
                    (Some(e), _, _) => format!("⚠ {e}"),
                    (None, Some(o), _) => format!("Measured {o}"),
                    (None, None, Some(e)) => format!("⚠ {e}"),
                    _ => format!("Updated {}", hm(look, &wx.updated)),
                };
                draw_text(dc, &small, &upd, pal.dim, pad + inner / 3, s(14.0), inner - inner / 3, s(20.0), r | DT_END_ELLIPSIS);

                // Current conditions.
                let y = s(42.0);
                draw_emoji(dc, &emoji_big, glyph, s(theme::EMOJI_LARGE.px), pal.fg, theme.color.bg, pad, y, s(60.0), s(64.0), l);
                let t = deg(wx.temp);
                draw_text(dc, &big, &t, pal.fg, pad + s(64.0), y, s(130.0), s(64.0), l);
                let (tw, _) = text_size(dc, &big, &t);
                draw_text(dc, &small, wx.unit(), pal.dim, pad + s(64.0) + tw + s(2.0), y + s(10.0), s(30.0), s(18.0), l);
                draw_text(dc, &body_b, desc, pal.fg, pad, y + s(8.0), inner, s(22.0), r | DT_END_ELLIPSIS);
                let feels = format!("Feels like {}", deg(wx.feels));
                draw_text(dc, &small, &feels, pal.dim, pad, y + s(32.0), inner, s(20.0), r);

                let details = format!(
                    "Humidity {:.0}%   ·   Wind {:.0} {}   ·   Rain chance {}",
                    wx.humidity, wx.wind, wx.wind_unit(),
                    wx.today_precip().map(|p| format!("{p:.0}%")).unwrap_or("--".into())
                );
                draw_text(dc, &small, &details, pal.dim, pad, s(112.0), inner, s(20.0), l | DT_END_ELLIPSIS);
                fill(dc, pad, s(142.0), inner, s(1.0).max(1), pal.line);

                // Next hours.
                draw_text(dc, &small_b, "NEXT HOURS", pal.dim, pad, s(152.0), inner, s(18.0), l);
                let n = wx.hourly.len().max(1) as i32;
                let col = inner / n;
                for (i, hr) in wx.hourly.iter().enumerate() {
                    let x = pad + col * i as i32;
                    draw_text(dc, &small, &hour_label(look, &hr.time), pal.dim, x, s(174.0), col, s(18.0), c);
                    draw_emoji(dc, &emoji, describe(hr.code, true).0, s(theme::EMOJI.px), pal.fg, theme.color.bg, x, s(193.0), col, s(28.0), c);
                    draw_text(dc, &small_b, &deg(hr.temp), pal.fg, x, s(222.0), col, s(18.0), c);
                    if let Some(p) = hr.precip.filter(|p| *p >= RAIN_SHOWN) {
                        draw_text(dc, &small, &format!("{p:.0}%"), rain_color(&pal, p), x, s(240.0), col, s(16.0), c);
                    }
                }
                fill(dc, pad, s(264.0), inner, s(1.0).max(1), pal.line);

                // Next days with a temperature range bar.
                draw_text(dc, &small_b, "NEXT DAYS", pal.dim, pad, s(274.0), inner, s(18.0), l);
                let lo = wx.daily.iter().map(|d| d.tmin).filter(|v| !v.is_nan()).fold(f64::INFINITY, f64::min);
                let hi = wx.daily.iter().map(|d| d.tmax).filter(|v| !v.is_nan()).fold(f64::NEG_INFINITY, f64::max);
                for (i, d) in wx.daily.iter().enumerate() {
                    let y = s(296.0) + s(31.0) * i as i32;
                    let rh = s(31.0);
                    draw_text(dc, &body, &d.label, pal.fg, pad, y, s(70.0), rh, l);
                    draw_emoji(dc, &emoji, describe(d.code, true).0, s(theme::EMOJI.px), pal.fg, theme.color.bg, pad + s(70.0), y, s(32.0), rh, c);
                    if let Some(p) = d.precip.filter(|p| *p >= RAIN_SHOWN) {
                        draw_text(dc, &small, &format!("{p:.0}%"), rain_color(&pal, p), pad + s(104.0), y, s(40.0), rh, l);
                    }
                    let bar_x = pad + s(186.0);
                    let bar_w = s(86.0);
                    draw_text(dc, &body, &deg(d.tmin), pal.dim, pad + s(140.0), y, s(40.0), rh, r);
                    draw_text(dc, &body, &deg(d.tmax), pal.fg, bar_x + bar_w + s(6.0), y, w - pad - (bar_x + bar_w + s(6.0)), rh, r);
                    let bh = s(5.0);
                    let by = y + (rh - bh) / 2;
                    round_fill(dc, bar_x, by, bar_w, bh, pal.track);
                    if hi > lo && !d.tmin.is_nan() && !d.tmax.is_nan() {
                        let x0 = bar_x + ((d.tmin - lo) / (hi - lo) * bar_w as f64) as i32;
                        let x1 = bar_x + ((d.tmax - lo) / (hi - lo) * bar_w as f64) as i32;
                        round_fill(dc, x0, by, (x1 - x0).max(bh), bh, pal.bar);
                    }
                }
                let model = match wx.model.as_str() {
                    "best_match" => "best match",
                    "knmi_seamless" => "KNMI HARMONIE",
                    "dmi_seamless" => "DMI HARMONIE",
                    m => m,
                };
                let credit = format!(
                    "Open-Meteo.com ({model}){} · CC BY 4.0",
                    if wx.observed.is_some() { " · KNMI" } else { "" }
                );
                draw_text(dc, &small, &credit, pal.dim, pad, h - s(30.0), inner, s(18.0), l | DT_END_ELLIPSIS);
            }
        }

        let _ = BitBlt(hdc, 0, 0, w, h, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(dc);
        let _ = EndPaint(hwnd, &ps);
    }
}

pub fn pcwstr(v: &[u16]) -> PCWSTR {
    PCWSTR(v.as_ptr())
}

/// An hourly forecast label ("HH:00" from Open-Meteo), compact: "9 PM" or "21:00".
fn hour_label(look: &Look, t: &str) -> String {
    match t.split(':').next().and_then(|h| h.trim().parse::<u32>().ok()) {
        Some(h) => look.hour(h),
        None => t.to_string(),
    }
}

/// An "HH:MM" time from Open-Meteo in the Windows time format (or the shared style's), e.g.
/// "7:45 PM" or "19:45".
fn hm(look: &Look, t: &str) -> String {
    let mut p = t.split(':').filter_map(|x| x.trim().parse::<u32>().ok());
    match (p.next(), p.next()) {
        (Some(h), Some(m)) => look.time(h, m),
        _ => t.to_string(),
    }
}