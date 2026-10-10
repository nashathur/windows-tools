//! Avatars: downloaded once from Discord's CDN (PNG), kept in %APPDATA%\TaskbarDock\discord-avatars,
//! decoded on a worker thread to 64×64 straight-alpha BGRA, and drawn as circles.

use crate::voice::Member;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub const SIZE: usize = 64;

/// 64×64 pixels, 0xAARRGGBB, straight alpha.
pub struct Image(pub Vec<u32>);

/// key → image (None: being fetched, or failed this run).
fn cache() -> &'static Mutex<HashMap<String, Option<Arc<Image>>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<Arc<Image>>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn dir() -> std::path::PathBuf {
    taskbar_dock::settings::dir().join("discord-avatars")
}

/// The member's avatar if it's ready; otherwise starts fetching it (from disk or the CDN) and
/// calls `ready` when it is.
pub fn get(m: &Member, ready: impl Fn() + Send + 'static) -> Option<Arc<Image>> {
    get_url(m.avatar_key(), m.avatar_url(), ready)
}

/// The image at `url` (a PNG), cached under `key` like the avatars: the server's logo.
pub fn get_url(key: String, url: String, ready: impl Fn() + Send + 'static) -> Option<Arc<Image>> {
    let mut c = cache().lock().ok()?;
    if let Some(v) = c.get(&key) {
        return v.clone();
    }
    c.insert(key.clone(), None);
    std::thread::spawn(move || {
        let file = dir().join(format!("{key}.png"));
        let bytes = std::fs::read(&file).ok().or_else(|| {
            let b = download(&url)?;
            let _ = std::fs::create_dir_all(dir());
            let _ = std::fs::write(&file, &b);
            Some(b)
        });
        if let Some(img) = bytes.as_deref().and_then(decode) {
            if let Ok(mut c) = cache().lock() {
                c.insert(key, Some(Arc::new(img)));
            }
            ready();
        }
    });
    None
}

fn download(url: &str) -> Option<Vec<u8>> {
    let tls = native_tls::TlsConnector::new().ok()?;
    let agent = ureq::AgentBuilder::new().tls_connector(Arc::new(tls)).timeout(Duration::from_secs(15)).build();
    let resp = agent.get(url).call().ok()?;
    let mut b = Vec::new();
    std::io::Read::read_to_end(&mut resp.into_reader().take(2 << 20), &mut b).ok()?;
    Some(b)
}

use std::io::Read as _;

/// PNG → 64×64 (box-filtered from whatever size Discord sent).
pub fn decode(bytes: &[u8]) -> Option<Image> {
    let mut dec = png::Decoder::new(bytes);
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let px = |x: usize, y: usize| -> [u32; 4] {
        let i = y * info.line_size;
        match info.color_type {
            png::ColorType::Rgba => { let p = &buf[i + x * 4..]; [p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32] }
            png::ColorType::Rgb => { let p = &buf[i + x * 3..]; [p[0] as u32, p[1] as u32, p[2] as u32, 255] }
            png::ColorType::GrayscaleAlpha => { let p = &buf[i + x * 2..]; [p[0] as u32, p[0] as u32, p[0] as u32, p[1] as u32] }
            _ => { let g = buf[i + x] as u32; [g, g, g, 255] }
        }
    };
    if w == 0 || h == 0 {
        return None;
    }
    let mut out = vec![0u32; SIZE * SIZE];
    for oy in 0..SIZE {
        for ox in 0..SIZE {
            let (x0, x1) = (ox * w / SIZE, ((ox + 1) * w / SIZE).max(ox * w / SIZE + 1));
            let (y0, y1) = (oy * h / SIZE, ((oy + 1) * h / SIZE).max(oy * h / SIZE + 1));
            let mut s = [0u32; 4];
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    let p = px(x, y);
                    // weight colour by alpha so transparent edges don't darken
                    s[0] += p[0] * p[3];
                    s[1] += p[1] * p[3];
                    s[2] += p[2] * p[3];
                    s[3] += p[3];
                }
            }
            let n = ((y1.min(h) - y0) * (x1.min(w) - x0)).max(1) as u32;
            let a = s[3] / n;
            let c = |v: u32| if s[3] == 0 { 0 } else { v / s[3] };
            out[oy * SIZE + ox] = a << 24 | c(s[0]) << 16 | c(s[1]) << 8 | c(s[2]);
        }
    }
    Some(Image(out))
}

/// Clears a disc of diameter `d` at (`x`, `y`) to transparent: the gap an avatar leaves around
/// itself where it overlaps the one under it.
pub fn cut_circle(buf: &mut [u32], width: i32, x: f32, y: f32, d: f32) {
    let height = buf.len() as i32 / width.max(1);
    let r = d / 2.0;
    let (cx, cy) = (x + r, y + r);
    for py in (y.floor() as i32).max(0)..((y + d).ceil() as i32).min(height) {
        for px in (x.floor() as i32).max(0)..((x + d).ceil() as i32).min(width) {
            let dist = ((px as f32 + 0.5 - cx).powi(2) + (py as f32 + 0.5 - cy).powi(2)).sqrt();
            let keep = 1.0 - (r - dist + 0.5).clamp(0.0, 1.0);
            let p = &mut buf[(py * width + px) as usize];
            let ch = |s: u32| ((((*p >> s) & 0xff) as f32 * keep).round() as u32) << s;
            *p = ch(24) | ch(16) | ch(8) | ch(0);
        }
    }
}

/// Draws a circle of diameter `d` at (`x`, `y`) into a premultiplied BGRA buffer, over what's
/// there: the image (bilinear) or, without one, a flat disc of `fallback` (0xRRGGBB).
/// `alpha` 0..=255 dims the whole circle.
pub fn draw_circle(buf: &mut [u32], width: i32, x: f32, y: f32, d: f32, img: Option<&Image>, fallback: u32, alpha: u32) {
    let height = buf.len() as i32 / width.max(1);
    let r = d / 2.0;
    let (cx, cy) = (x + r, y + r);
    for py in (y.floor() as i32).max(0)..((y + d).ceil() as i32).min(height) {
        for px in (x.floor() as i32).max(0)..((x + d).ceil() as i32).min(width) {
            let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
            let dist = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let cover = (r - dist + 0.5).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            let src = match img {
                Some(im) => sample(im, (fx - x) / d * SIZE as f32 - 0.5, (fy - y) / d * SIZE as f32 - 0.5),
                None => 0xFF00_0000 | fallback,
            };
            over(&mut buf[(py * width + px) as usize], src, cover * alpha as f32 / 255.0);
        }
    }
}

/// A ring (outer diameter `d`, `thick` px wide) of colour `rgb`, e.g. the green "speaking" ring.
pub fn draw_ring(buf: &mut [u32], width: i32, x: f32, y: f32, d: f32, thick: f32, rgb: u32) {
    draw_ring_a(buf, width, x, y, d, thick, rgb, 255);
}

/// [`draw_ring`] at `alpha` (0..=255): the pulsing ring.
#[allow(clippy::too_many_arguments)]
pub fn draw_ring_a(buf: &mut [u32], width: i32, x: f32, y: f32, d: f32, thick: f32, rgb: u32, alpha: u32) {
    let height = buf.len() as i32 / width.max(1);
    let r = d / 2.0;
    let (cx, cy) = (x + r, y + r);
    for py in (y.floor() as i32).max(0)..((y + d).ceil() as i32).min(height) {
        for px in (x.floor() as i32).max(0)..((x + d).ceil() as i32).min(width) {
            let dist = ((px as f32 + 0.5 - cx).powi(2) + (py as f32 + 0.5 - cy).powi(2)).sqrt();
            let cover = (r - dist + 0.5).clamp(0.0, 1.0).min((dist - (r - thick) + 0.5).clamp(0.0, 1.0));
            if cover > 0.0 {
                over(&mut buf[(py * width + px) as usize], 0xFF00_0000 | rgb, cover * alpha as f32 / 255.0);
            }
        }
    }
}

fn sample(im: &Image, x: f32, y: f32) -> u32 {
    let n = SIZE as i32;
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let at = |x: i32, y: i32| im.0[(y.clamp(0, n - 1) * n + x.clamp(0, n - 1)) as usize];
    let ch = |p: u32, s: u32| ((p >> s) & 0xff) as f32;
    let mut out = 0u32;
    for s in [24u32, 16, 8, 0] {
        let top = ch(at(x0, y0), s) * (1.0 - tx) + ch(at(x0 + 1, y0), s) * tx;
        let bot = ch(at(x0, y0 + 1), s) * (1.0 - tx) + ch(at(x0 + 1, y0 + 1), s) * tx;
        out |= ((top * (1.0 - ty) + bot * ty).round() as u32).min(255) << s;
    }
    out
}

/// Straight-alpha `src` with extra coverage `k` over a premultiplied pixel.
pub fn over(dst: &mut u32, src: u32, k: f32) {
    let a = ((src >> 24) as f32 * k).round() as u32;
    let inv = 255 - a;
    let ch = |s: u32| ((src >> s) & 0xff) * a / 255 + ((*dst >> s) & 0xff) * inv / 255;
    *dst = (a + (*dst >> 24) * inv / 255) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_covers_its_middle_not_its_corners() {
        let mut buf = vec![0u32; 20 * 20];
        draw_circle(&mut buf, 20, 0.0, 0.0, 20.0, None, taskbar_dock::theme::brand("discord.blurple", true).rgb24(), 255);
        assert_eq!(buf[10 * 20 + 10], 0xFF58_65F2);
        assert_eq!(buf[0], 0);
    }

    #[test]
    fn cut_makes_a_gap() {
        let mut buf = vec![0xFF11_2233u32; 20 * 20];
        cut_circle(&mut buf, 20, 5.0, 5.0, 10.0);
        assert_eq!(buf[10 * 20 + 10], 0);
        assert_eq!(buf[0], 0xFF11_2233);
    }

    #[test]
    fn ring_is_hollow() {
        let mut buf = vec![0u32; 20 * 20];
        draw_ring(&mut buf, 20, 0.0, 0.0, 20.0, 2.0, taskbar_dock::theme::brand("discord.green", true).rgb24());
        assert_eq!(buf[10 * 20 + 10], 0);
        assert!(buf[10 * 20 + 1] >> 24 > 240); // on the ring (anti-aliased edge)
    }

    #[test]
    fn decode_scales_to_64() {
        // a 2×2 RGBA PNG, encoded here
        let mut bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut bytes, 2, 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 0]).unwrap();
        }
        let img = decode(&bytes).unwrap();
        assert_eq!(img.0.len(), SIZE * SIZE);
        assert_eq!(img.0[0], 0xFFFF_0000); // top-left red
        assert_eq!(img.0[SIZE * SIZE - 1] >> 24, 0); // bottom-right transparent
    }
}
