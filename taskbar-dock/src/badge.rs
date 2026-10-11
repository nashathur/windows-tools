//! A badge: a coloured, rounded shape with short text in it (e.g. the bus line number "30"),
//! the same for every widget.
//!
//! - As tall as the tray's icons and at the same height ([`draw::icon_box`]), so it lines up
//!   with them; as wide as its text plus [`PAD`] px each side, never narrower than it is tall.
//! - Text in the shared font, bold, 1 px larger than the body text ([`font`]); centred by its
//!   visible pixels (not the font's line box), so the padding looks even.
//! - Corners: the shared `control_radius` (Windows 11 controls: 4 px).
//! - Text colour: given, or black/white for contrast with the background ([`contrast_text`]).
//!
//! Use, in a strip drawn white-on-black then coloured (see crate::draw):
//! 1. `let f = badge::font(&look, scale)` and `let b = Badge::measure(dc, f, "30", height, scale)`;
//!    `b.width` tells the layout how much room it takes.
//! 2. After `draw::text_to_color`: `b.paint(dc, buf, width, x, f, bg, fg, &look, scale)`.
//! 3. `DeleteObject(f)`.

use crate::draw;
use crate::style::Look;
use windows_sys::Win32::Foundation::{COLORREF, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;

/// Space between the text's visible pixels and the badge's sides, px at 100%.
pub const PAD: f32 = 4.0;

/// Badge size and where its text goes, for text whose visible pixels are `ink` (left, top,
/// right, bottom within its text cell): (width, top, height, text x, text y), x relative to
/// the badge's left edge.
pub fn layout(height: i32, scale: f32, ink: (i32, i32, i32, i32)) -> (i32, i32, i32, i32, i32) {
    let (top, size) = draw::icon_box(height, scale);
    let (iw, ih) = (ink.2 - ink.0, ink.3 - ink.1);
    let width = size.max(iw + 2 * (PAD * scale).round() as i32);
    (width, top, size, (width - iw + 1) / 2 - ink.0, top + (size - ih + 1) / 2 - ink.1)
}

/// The badge font: the shared font, bold, 1 px larger than the body text. Delete it after use.
pub unsafe fn font(look: &Look, scale: f32) -> HFONT {
    let face = crate::wide(&look.font);
    CreateFontW(-font_px(look, scale), 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, ANTIALIASED_QUALITY as u32, 0, face.as_ptr())
}

/// The badge font's size in px at `scale` ([`font`]).
pub fn font_px(look: &Look, scale: f32) -> i32 {
    ((look.font_size + 1.0) * scale).round() as i32
}

/// Black or white, whichever reads better on `bg` (a COLORREF, 0x00BBGGRR).
pub fn contrast_text(bg: COLORREF) -> COLORREF {
    let (r, g, b) = (bg & 0xff, (bg >> 8) & 0xff, (bg >> 16) & 0xff);
    if r * 299 + g * 587 + b * 114 > 150_000 { 0 } else { 0x00ff_ffff }
}

pub struct Badge {
    text: Vec<u16>,
    /// Total width in px.
    pub width: i32,
    top: i32,
    height: i32,
    text_x: i32,
    text_y: i32,
}

impl Badge {
    /// Measures `text` in `font` (from [`font`]) for a strip `height` px tall at `scale`
    /// (display scale × drag zoom). `dc` is any memory DC.
    pub unsafe fn measure(dc: HDC, font: HFONT, text: &str, height: i32, scale: f32) -> Badge {
        let text: Vec<u16> = text.encode_utf16().collect();
        let ink = text_ink(dc, font, &text);
        let (width, top, h, text_x, text_y) = layout(height, scale, ink);
        Badge { text, width, top, height: h, text_x, text_y }
    }

    /// Draws it with its left edge at `x` into the strip's DIB (selected into `dc`, pixels
    /// `buf`, `buf_width` wide). Call after `draw::text_to_color`: the badge is opaque colour.
    /// `fg` None = black or white for contrast.
    pub unsafe fn paint(&self, dc: HDC, buf: &mut [u32], buf_width: i32, x: i32, font: HFONT, bg: COLORREF, fg: Option<COLORREF>, look: &Look, scale: f32) {
        let rc = RECT { left: x, top: self.top, right: x + self.width, bottom: self.top + self.height };
        let brush = CreateSolidBrush(bg);
        FillRect(dc, &rc, brush);
        DeleteObject(brush);
        let old = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, fg.unwrap_or_else(|| contrast_text(bg)));
        TextOutW(dc, x + self.text_x, self.text_y, self.text.as_ptr(), self.text.len() as i32);
        SelectObject(dc, old);
        GdiFlush();
        draw::round_rect(buf, buf_width, (rc.left, rc.top, rc.right, rc.bottom), (look.control_radius * scale).round());
    }
}

/// Where the visible pixels of `text` in `font` are, within its text cell.
unsafe fn text_ink(dc: HDC, font: HFONT, text: &[u16]) -> (i32, i32, i32, i32) {
    let old_font = SelectObject(dc, font);
    let mut sz = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut sz);
    SelectObject(dc, old_font);
    let (w, h) = (sz.cx.max(1), sz.cy.max(1));
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB as _;
    let mem = CreateCompatibleDC(dc);
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, 0, 0);
    if dib == 0 || bits.is_null() {
        DeleteDC(mem);
        return (0, 0, w, h);
    }
    let old = SelectObject(mem, dib);
    SelectObject(mem, font);
    SetBkMode(mem, TRANSPARENT as _);
    SetTextColor(mem, 0x00ff_ffff);
    TextOutW(mem, 0, 0, text.as_ptr(), text.len() as i32);
    GdiFlush();
    let buf = std::slice::from_raw_parts(bits as *const u32, (w * h) as usize);
    let ink = draw::ink_bounds(buf, w, h).unwrap_or((0, 0, w, h));
    SelectObject(mem, old);
    DeleteObject(dib);
    DeleteDC(mem);
    ink
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_is_tray_icon_tall_and_fits_its_text() {
        // one narrow digit (7 px of ink): a 16 px square in the tray-icon box
        assert_eq!(layout(48, 1.0, (1, 4, 8, 14)).0, 16);
        let (w, top, h, tx, ty) = layout(48, 1.0, (1, 4, 16, 14)); // "30": 15 × 10 px of ink
        assert_eq!((w, top, h), (23, 16, 16));
        // text centred: 4 px each side, 3 px above and below
        assert_eq!((tx + 1, ty + 4), (4, 16 + 3));
        // at 150 %
        assert_eq!(layout(72, 1.5, (1, 4, 16, 14)).0, 27);
    }

    #[test]
    fn contrast() {
        assert_eq!(contrast_text(0x0000_d9fc), 0); // yellow → black text
        assert_eq!(contrast_text(0x002b_00e4), 0x00ff_ffff); // red → white text
    }
}
