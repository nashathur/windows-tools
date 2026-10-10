//! Colour emoji: GDI draws emoji fonts in one colour only (a white shape on a dark taskbar, a
//! black blob on a light one), so emoji are drawn here with Direct2D + DirectWrite colour fonts
//! into pixels, and put into a strip or panel from those.
//!
//! - [`render`]: `text` in the emoji font ([`crate::theme::EMOJI_FONT`]) at `px`, centred in a
//!   `w` × `h` box, as premultiplied 0xAARRGGBB pixels. Cached per thread (strips redraw often).
//! - [`fit_over`]: a rendered emoji's visible part fitted into a strip's tray-icon box (as
//!   [`crate::draw::fit_icon`] does for one-colour glyphs), drawn over premultiplied pixels.
//! - [`flatten`]: a rendered emoji on an opaque background colour (for a GDI panel: BitBlt it).

use crate::theme::{Rgb, EMOJI_FONT};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use windows::core::{w, HSTRING};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;

/// Premultiplied 0xAARRGGBB pixels, `w` × `h`, rows top down.
pub struct Image {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
}

struct Factories {
    wic: IWICImagingFactory,
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
}

thread_local! {
    static FACTORIES: RefCell<Option<Option<Factories>>> = const { RefCell::new(None) };
    static CACHE: RefCell<HashMap<(String, u32, i32, i32), Option<Rc<Image>>>> = RefCell::new(HashMap::new());
}

/// Most cached images kept (weather: the strip's and the panel's handful of symbols).
const CACHE_MAX: usize = 64;

fn factories() -> Option<Factories> {
    unsafe {
        // COM for WIC; already initialised (either apartment) is fine
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let wic = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let d2d = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let dw = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        Some(Factories { wic, d2d, dw })
    }
}

/// `text` in colour at `px` (font size in pixels), centred in a `w` × `h` box. None if
/// Direct2D isn't available (the caller can fall back to GDI).
pub fn render(text: &str, px: f32, w: i32, h: i32) -> Option<Rc<Image>> {
    if w <= 0 || h <= 0 || text.is_empty() {
        return None;
    }
    let key = (text.to_string(), px.to_bits(), w, h);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let img = FACTORIES.with(|f| {
        let mut f = f.borrow_mut();
        let f = f.get_or_insert_with(factories).as_ref()?;
        draw(f, text, px, w, h)
    });
    let img = img.map(Rc::new);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= CACHE_MAX {
            c.clear();
        }
        c.insert(key, img.clone());
    });
    img
}

fn draw(f: &Factories, text: &str, px: f32, w: i32, h: i32) -> Option<Image> {
    unsafe {
        let bmp = f.wic.CreateBitmap(w as u32, h as u32, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad).ok()?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let rt = f.d2d.CreateWicBitmapRenderTarget(&bmp, &props).ok()?;
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        let format = f
            .dw
            .CreateTextFormat(&HSTRING::from(EMOJI_FONT), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, px, w!("en-us"))
            .ok()?;
        format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok()?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok()?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP).ok()?;
        // only for glyphs without colour layers
        let black = D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        let brush = rt.CreateSolidColorBrush(&black, None).ok()?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let rect = D2D_RECT_F { left: 0.0, top: 0.0, right: w as f32, bottom: h as f32 };
        rt.BeginDraw();
        rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        rt.DrawText(&wide, &format, &rect, &brush, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, DWRITE_MEASURING_MODE_NATURAL);
        rt.EndDraw(None, None).ok()?;
        let mut px = vec![0u32; (w * h) as usize];
        let bytes = std::slice::from_raw_parts_mut(px.as_mut_ptr() as *mut u8, px.len() * 4);
        bmp.CopyPixels(std::ptr::null(), (w * 4) as u32, bytes).ok()?;
        Some(Image { w, h, px })
    }
}

/// The box (left, top, right, bottom; exclusive) of the pixels that show (alpha > 8).
pub fn ink(img: &Image) -> Option<(i32, i32, i32, i32)> {
    let (mut l, mut t, mut r, mut b) = (img.w, img.h, -1, -1);
    for y in 0..img.h {
        for x in 0..img.w {
            if img.px[(y * img.w + x) as usize] >> 24 > 8 {
                l = l.min(x);
                t = t.min(y);
                r = r.max(x);
                b = b.max(y);
            }
        }
    }
    (r >= l).then_some((l, t, r + 1, b + 1))
}

/// `src` over `dst`, both premultiplied 0xAARRGGBB.
fn over(dst: u32, src: u32) -> u32 {
    let inv = 255 - (src >> 24);
    let ch = |s: u32| (((src >> s) & 0xff) + ((dst >> s) & 0xff) * inv / 255).min(255) << s;
    ch(24) | ch(16) | ch(8) | ch(0)
}

/// Fits `img`'s visible part into the `size` px square at (`x`, `top`) of `dst` (`dst_w` wide,
/// premultiplied 0xAARRGGBB), centred and smoothly downsampled, drawn over what's there.
pub fn fit_over(dst: &mut [u32], dst_w: i32, x: i32, top: i32, size: i32, img: &Image) {
    let Some((l, t, r, b)) = ink(img) else { return };
    let (iw, ih) = ((r - l) as f32, (b - t) as f32);
    let k = iw.max(ih) / size.max(1) as f32; // source px per destination px
    let (ox, oy) = (x as f32 + (size as f32 - iw / k) / 2.0, top as f32 + (size as f32 - ih / k) / 2.0);
    let dst_h = dst.len() as i32 / dst_w.max(1);
    for dy in top.max(0)..(top + size).min(dst_h) {
        for dx in x.max(0)..(x + size).min(dst_w) {
            // the destination pixel's area in source coordinates; average each channel there
            let (sx0, sy0) = (l as f32 + (dx as f32 - ox) * k, t as f32 + (dy as f32 - oy) * k);
            let (sx1, sy1) = (sx0 + k, sy0 + k);
            let (mut sum, mut area) = ([0.0f32; 4], 0.0f32);
            let mut sy = sy0.floor() as i32;
            while (sy as f32) < sy1 {
                let hy = (sy1.min(sy as f32 + 1.0) - sy0.max(sy as f32)).max(0.0);
                let mut sx = sx0.floor() as i32;
                while (sx as f32) < sx1 {
                    let hx = (sx1.min(sx as f32 + 1.0) - sx0.max(sx as f32)).max(0.0);
                    if sx >= 0 && sy >= 0 && sx < img.w && sy < img.h {
                        let p = img.px[(sy * img.w + sx) as usize];
                        for (i, s) in [24, 16, 8, 0].into_iter().enumerate() {
                            sum[i] += ((p >> s) & 0xff) as f32 * hx * hy;
                        }
                    }
                    area += hx * hy;
                    sx += 1;
                }
                sy += 1;
            }
            let c = |i: usize| (sum[i] / area.max(1e-6)).round().min(255.0) as u32;
            let src = c(0) << 24 | c(1) << 16 | c(2) << 8 | c(3);
            let p = &mut dst[(dy * dst_w + dx) as usize];
            *p = over(*p, src);
        }
    }
}

/// `img` on the opaque colour `bg`, as 0x00RRGGBB pixels (a DIB to BitBlt into a GDI panel).
pub fn flatten(img: &Image, bg: Rgb) -> Vec<u32> {
    let base = bg.pixel();
    img.px.iter().map(|&p| over(base, p) & 0x00ff_ffff).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_and_flatten() {
        // half-transparent red over opaque white
        assert_eq!(over(0xffff_ffff, 0x8080_0000), 0xffff_7f7f);
        let img = Image { w: 2, h: 1, px: vec![0, 0xff00_ff00] };
        assert_eq!(flatten(&img, Rgb::hex(0x202020)), vec![0x202020, 0x00ff00]);
    }

    #[test]
    fn fit_over_centres_the_ink() {
        // a 4×4 opaque square in the middle of 8×8, fitted into 2×2 at (1, 1) of a 4×4 buffer
        let mut px = vec![0u32; 64];
        for y in 2..6 {
            for x in 2..6 {
                px[y * 8 + x] = 0xff11_2233;
            }
        }
        let img = Image { w: 8, h: 8, px };
        let mut dst = vec![0u32; 16];
        fit_over(&mut dst, 4, 1, 1, 2, &img);
        assert_eq!((dst[5], dst[6], dst[9], dst[10]), (0xff11_2233, 0xff11_2233, 0xff11_2233, 0xff11_2233));
        assert_eq!((dst[0], dst[15]), (0, 0));
    }

    #[test]
    fn renders_a_colour_emoji() {
        let img = render("🌧", 40.0, 48, 48).expect("Direct2D");
        assert_eq!((img.w, img.h), (48, 48));
        // some pixels show, and they're not all grey (colour layers were used)
        let shown: Vec<_> = img.px.iter().filter(|p| *p >> 24 > 200).collect();
        assert!(!shown.is_empty());
        assert!(shown.iter().any(|p| { let (r, g, b) = ((*p >> 16) & 0xff, (*p >> 8) & 0xff, *p & 0xff); r.max(g).max(b) - r.min(g).min(b) > 40 }));
        assert!(Rc::ptr_eq(&img, &render("🌧", 40.0, 48, 48).unwrap()), "cached");
    }
}
