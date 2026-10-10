//! Cover pictures: a square, premultiplied BGRA (as u32 0xAARRGGBB, the DIB layout) picture
//! made from whatever the app gives (album art, or a video thumbnail with black bars).

/// A square picture.
#[derive(Clone, PartialEq, Debug)]
pub struct Cover {
    pub size: i32,
    pub pixels: Vec<u32>,
}

/// Size covers are kept at; drawing scales them down (the click panel's 96 px at 150% is 144).
pub const KEEP: i32 = 160;

fn luma(p: u32) -> u32 {
    (((p >> 16) & 0xff) * 299 + ((p >> 8) & 0xff) * 587 + (p & 0xff) * 114) / 1000
}

/// Trims black bars (a 4:3 video thumbnail holding a 16:9 video), at most a quarter from each
/// side, then takes the centred square and scales it to `n` × `n`.
pub fn square(src: &[u32], w: i32, h: i32, n: i32) -> Cover {
    let dark_row = |y: i32| (0..w).all(|x| luma(src[(y * w + x) as usize]) <= 32);
    let dark_col = |x: i32| (0..h).all(|y| luma(src[(y * w + x) as usize]) <= 32);
    let (mut t, mut b, mut l, mut r) = (0, h, 0, w);
    while t < h / 4 && dark_row(t) {
        t += 1;
    }
    while b > h - h / 4 && dark_row(b - 1) {
        b -= 1;
    }
    while l < w / 4 && dark_col(l) {
        l += 1;
    }
    while r > w - w / 4 && dark_col(r - 1) {
        r -= 1;
    }
    let side = (r - l).min(b - t).max(1);
    let (x0, y0) = (l + (r - l - side) / 2, t + (b - t - side) / 2);
    Cover { size: n, pixels: resample(src, w, (x0, y0, side), n) }
}

/// The centred square of a picture, scaled to `n` × `n`, without trimming anything (for logos:
/// their transparent or dark edges belong to them).
pub fn fit(src: &[u32], w: i32, h: i32, n: i32) -> Cover {
    let side = w.min(h).max(1);
    Cover { size: n, pixels: resample(src, w, ((w - side) / 2, (h - side) / 2, side), n) }
}

/// Makes 0xAARRGGBB pixels premultiplied (colour scaled by alpha), as drawing expects: a picture
/// with any colour brighter than its alpha has straight alpha and is converted; one with no
/// alpha at all (every pixel 0) is opaque.
pub fn premultiply(mut px: Vec<u32>) -> Vec<u32> {
    if px.iter().all(|p| p >> 24 == 0) {
        for p in px.iter_mut() {
            *p |= 0xff00_0000;
        }
        return px;
    }
    let ch = |p: u32, s: u32| (p >> s) & 0xff;
    let straight = px.iter().any(|&p| ch(p, 16).max(ch(p, 8)).max(ch(p, 0)) > p >> 24);
    if straight {
        for p in px.iter_mut() {
            let a = *p >> 24;
            *p = a << 24 | (ch(*p, 16) * a / 255) << 16 | (ch(*p, 8) * a / 255) << 8 | (ch(*p, 0) * a / 255);
        }
    }
    px
}

/// The main colour of a picture, for a tint: the most common hue among its coloured pixels
/// (grey, very dark and transparent ones don't count), evened out so every tint looks about
/// as strong (saturation at least 55 %, brightness 75–95 %). None for a grey picture.
pub fn dominant(c: &Cover) -> Option<(u32, u32, u32)> {
    let mut bins = [(0f32, 0f32, 0f32, 0f32); 36];
    for &p in &c.pixels {
        let a = (p >> 24) as f32 / 255.0;
        if a < 0.8 {
            continue;
        }
        let (r, g, b) = (((p >> 16) & 0xff) as f32 / a, ((p >> 8) & 0xff) as f32 / a, (p & 0xff) as f32 / a);
        let (h, s, v) = hsv(r, g, b);
        if s < 0.3 || v < 0.25 {
            continue;
        }
        let bin = &mut bins[((h / 10.0) as usize).min(35)];
        let w = s * v;
        *bin = (bin.0 + w, bin.1 + r * w, bin.2 + g * w, bin.3 + b * w);
    }
    let best = bins.iter().max_by(|a, b| a.0.total_cmp(&b.0))?;
    if best.0 <= 0.0 {
        return None;
    }
    let (h, s, v) = hsv(best.1 / best.0, best.2 / best.0, best.3 / best.0);
    Some(rgb_of(h, s.max(0.55), v.clamp(0.75, 0.95)))
}

/// (r, g, b) 0..255 → (hue 0..360, saturation 0..1, value 0..1).
fn hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let d = mx - mn;
    let h = if d <= 0.0 {
        0.0
    } else if mx == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, if mx > 0.0 { d / mx } else { 0.0 }, mx / 255.0)
}

fn rgb_of(h: f32, s: f32, v: f32) -> (u32, u32, u32) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    (to(r), to(g), to(b))
}

/// Area-averages the `side`-px square at (x, y) of `src` (`w` wide) down or up to `n` × `n`.
pub fn resample(src: &[u32], w: i32, (x, y, side): (i32, i32, i32), n: i32) -> Vec<u32> {
    let h = src.len() as i32 / w.max(1);
    let k = side as f32 / n as f32; // source px per output px
    let mut out = vec![0u32; (n * n) as usize];
    for oy in 0..n {
        for ox in 0..n {
            let (sx0, sy0) = (x as f32 + ox as f32 * k, y as f32 + oy as f32 * k);
            let (sx1, sy1) = (sx0 + k, sy0 + k);
            let mut acc = [0f32; 4];
            let mut area = 0f32;
            let mut sy = sy0.floor() as i32;
            while (sy as f32) < sy1 {
                let hy = (sy1.min(sy as f32 + 1.0) - sy0.max(sy as f32)).max(0.0);
                let mut sx = sx0.floor() as i32;
                while (sx as f32) < sx1 {
                    let hx = (sx1.min(sx as f32 + 1.0) - sx0.max(sx as f32)).max(0.0);
                    let p = src[(sy.clamp(0, h - 1) * w + sx.clamp(0, w - 1)) as usize];
                    for (i, a) in acc.iter_mut().enumerate() {
                        *a += ((p >> (24 - 8 * i)) & 0xff) as f32 * hx * hy;
                    }
                    area += hx * hy;
                    sx += 1;
                }
                sy += 1;
            }
            let c = |i: usize| ((acc[i] / area.max(1e-6)).round() as u32).min(255);
            out[(oy * n + ox) as usize] = c(0) << 24 | c(1) << 16 | c(2) << 8 | c(3);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_bars_are_trimmed_and_the_middle_kept() {
        // 8×6: black bars on the top and bottom rows, red in between with a blue middle
        let (w, h) = (8, 6);
        let mut px = vec![0xff00_0000u32; (w * h) as usize];
        for y in 1..5 {
            for x in 0..8 {
                px[(y * w + x) as usize] = if (2..6).contains(&x) { 0xff00_00ff } else { 0xffff_0000 };
            }
        }
        // content is 8×4 (rows 1..5); the centred 4×4 square is the blue middle
        let c = square(&px, w, h, 2);
        assert_eq!(c.pixels, vec![0xff00_00ff; 4]);
    }

    #[test]
    fn a_dark_picture_is_not_cropped_away() {
        let px = vec![0xff10_1010u32; 16 * 16];
        let c = square(&px, 16, 16, 4);
        assert_eq!(c.pixels, vec![0xff10_1010; 16]);
    }

    #[test]
    fn fit_keeps_the_edges() {
        // 4×2 with black sides: the centred 2×2 square, nothing trimmed
        let px = vec![0xff00_0000, 0xffff_0000, 0xffff_0000, 0xff00_0000, 0xff00_0000, 0xffff_0000, 0xffff_0000, 0xff00_0000];
        assert_eq!(fit(&px, 4, 2, 2).pixels, vec![0xffff_0000; 4]);
    }

    #[test]
    fn premultiply_only_when_needed() {
        // straight alpha: half-transparent white → half-bright
        assert_eq!(premultiply(vec![0x80ff_ffff, 0xff00_0000]), vec![0x8080_8080, 0xff00_0000]);
        // already premultiplied: unchanged
        assert_eq!(premultiply(vec![0x8080_8080]), vec![0x8080_8080]);
        // no alpha at all: opaque
        assert_eq!(premultiply(vec![0x0012_3456]), vec![0xff12_3456]);
    }

    #[test]
    fn dominant_colour() {
        // mostly Spotify green with some white and black: green, evened out
        let mut px = vec![0xff1e_d760u32; 60];
        px.extend(vec![0xffff_ffffu32; 30]);
        px.extend(vec![0xff00_0000u32; 30]);
        let (r, g, b) = dominant(&Cover { size: 11, pixels: px }).unwrap();
        assert!(g > r && g > b && g >= 190);
        // a dull dark red comes out as a clearer red
        let (r, g, b) = dominant(&Cover { size: 2, pixels: vec![0xff60_2020; 4] }).unwrap();
        assert!(r >= 190 && g < 100 && b < 100);
        // grey has no colour
        assert_eq!(dominant(&Cover { size: 2, pixels: vec![0xff80_8080; 4] }), None);
    }

    #[test]
    fn resample_averages() {
        // 2×2 black and white checker → one grey pixel
        let px = vec![0xff00_0000, 0xffff_ffff, 0xffff_ffff, 0xff00_0000];
        assert_eq!(resample(&px, 2, (0, 0, 2), 1), vec![0xff80_8080]);
    }
}
