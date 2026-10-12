//! Shared drawing for a widget's taskbar strip: a 32-bit premultiplied-alpha buffer (as used
//! by UpdateLayeredWindow), so it blends with the taskbar's Mica and stays clickable everywhere.
//!
//! Typical frame: draw the text white-on-black with GDI into the buffer → [`text_to_color`] →
//! draw opaque things (badges) → [`round_rect`] them → [`background_rect`] (hover highlight).

use crate::style::{Look, CLOCK_BASELINES, TRAY_ICON};

/// Text drawn white on black (alpha 0) becomes premultiplied colour, its brightness the
/// coverage. `color(y)` picks the colour per row (e.g. a dimmer first line).
pub fn text_to_color(
    buf: &mut [u32],
    width: i32,
    height: i32,
    color: impl Fn(i32) -> (u32, u32, u32),
) {
    for y in 0..height {
        let c = color(y);
        for x in 0..width {
            let p = &mut buf[(y * width + x) as usize];
            let a = ((*p >> 16) & 0xff).max((*p >> 8) & 0xff).max(*p & 0xff);
            *p = a << 24 | (c.0 * a / 255) << 16 | (c.1 * a / 255) << 8 | (c.2 * a / 255);
        }
    }
}

// ---- corners (see CLAUDE.md "UI: rounded corners") ----

/// The corner Windows 11 gives a top-level panel (`DWMWCP_ROUND`), px at 100%.
pub const PANEL_RADIUS: f32 = 8.0;
/// Corners with a radius above this (px) get a continuous-curvature ("squircle") shape instead
/// of a circular arc, as long as they fit (see [`shape_cover`]); at 4 px it can't be seen.
pub const SMOOTH_ABOVE: f32 = 8.0;
/// How far a continuous corner reaches along each edge, × the radius (iOS's is about 1.53).
const SMOOTH_EXTENT: f32 = 1.5;
/// Exponent of the superellipse |u|^n + |v|^n = 1 that makes the corner. For `SMOOTH_EXTENT`
/// 1.5 it cuts the corner as deep on the diagonal as a circle of the same radius (n = 3.2).
const SMOOTH_N: f32 = 3.2;

/// A nested shape's radius: the outer radius less the gap between them, but never below `floor`
/// (use the control radius for anything the user can click).
pub fn concentric(outer: f32, gap: f32, floor: f32) -> f32 {
    (outer - gap).max(floor)
}

/// The radius for a card or cover art of `side` px: 12.5% of the side (roundness 25%, between
/// an iPad's and an iPhone's), at least the control radius, at most 12 px at 100%.
pub fn card_radius(look: &Look, scale: f32, side: i32) -> f32 {
    let lo = (look.control_radius * scale).round();
    let hi = (12.0 * scale).round().max(lo);
    (side as f32 * 0.125).round().clamp(lo, hi)
}

/// How much of the pixel centred at (`fx`, `fy`) lies inside the rectangle (`l`, `t`, `r`, `b`,
/// fractional px) with corners of `radius` px, 0..1; `radius` is clamped to half the shorter
/// side. A radius above [`SMOOTH_ABOVE`] gets the continuous-curvature corner, if it fits
/// (reach ≤ half the shorter side); otherwise (small radius, capsule, circle) a circular arc.
pub fn shape_cover(fx: f32, fy: f32, (l, t, r, b): (f32, f32, f32, f32), radius: f32) -> f32 {
    let radius = radius.min((r - l) / 2.0).min((b - t) / 2.0).max(0.0);
    let ext = radius * SMOOTH_EXTENT;
    if radius > SMOOTH_ABOVE && ext <= (r - l) / 2.0 && ext <= (b - t) / 2.0 {
        // u, v: how far into the corner square (0 = straight part, 1 = its outer edge)
        let u = (l + ext - fx).max(fx - (r - ext)).max(0.0) / ext;
        let v = (t + ext - fy).max(fy - (b - ext)).max(0.0) / ext;
        let (un, vn) = (u.powf(SMOOTH_N), v.powf(SMOOTH_N));
        // distance to the curve ≈ f / |grad f| with f = u^n + v^n - 1
        let g = SMOOTH_N / ext * (un / u.max(1e-6)).hypot(vn / v.max(1e-6));
        if g < 1e-6 {
            return 1.0;
        }
        (0.5 - (un + vn - 1.0) / g).clamp(0.0, 1.0)
    } else {
        let (cx, cy) = (
            fx.clamp(l + radius, r - radius),
            fy.clamp(t + radius, b - radius),
        );
        (radius - ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0)
    }
}

/// Fills a rounded rectangle (fractional px) with `c` at `alpha` (0..1), anti-aliased, over the
/// opaque pixels already there (buffer rows of `width` px).
pub fn fill_round(
    buf: &mut [u32],
    width: i32,
    (l, t, r, b): (f32, f32, f32, f32),
    radius: f32,
    c: (u32, u32, u32),
    alpha: f32,
) {
    let height = buf.len() as i32 / width.max(1);
    for y in (t.floor() as i32).max(0)..(b.ceil() as i32).min(height) {
        for x in (l.floor() as i32).max(0)..(r.ceil() as i32).min(width) {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // coverage: inside the rounded part, and within the straight edges
            let edge = (fx - l + 0.5)
                .min(r - fx + 0.5)
                .min(fy - t + 0.5)
                .min(b - fy + 0.5)
                .clamp(0.0, 1.0);
            let a = (shape_cover(fx, fy, (l, t, r, b), radius) * edge * alpha).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let p = &mut buf[(y * width + x) as usize];
            let ch = |s: u32, v: u32| {
                ((((*p >> s) & 0xff) as f32 * (1.0 - a) + v as f32 * a).round() as u32).min(255)
                    << s
            };
            *p = 0xff00_0000 | ch(16, c.0) | ch(8, c.1) | ch(0, c.2);
        }
    }
}

/// Makes the opaque pixels of `rect` (left, top, right, bottom) a rounded rectangle with
/// anti-aliased corners of `radius` px: outside the shape the pixels fade to transparent.
pub fn round_rect(buf: &mut [u32], width: i32, rect: (i32, i32, i32, i32), radius: f32) {
    let (l, t, r, b) = rect;
    let shape = (l as f32, t as f32, r as f32, b as f32);
    for y in t.max(0)..b {
        for x in l.max(0)..r.min(width) {
            let cover = shape_cover(x as f32 + 0.5, y as f32 + 0.5, shape, radius);
            let a = (255.0 * cover).round() as u32;
            let p = &mut buf[(y * width + x) as usize];
            let ch = |shift: u32| ((*p >> shift) & 0xff) * a / 255;
            *p = a << 24 | ch(16) << 16 | ch(8) << 8 | ch(0);
        }
    }
}

/// Padding (px at 100%) around a button's glyph in its hover highlight box (rule: "Buttons" in
/// CLAUDE.md): the box is the tray-icon box plus this on every side, never the full-height bar.
pub const BUTTON_MARGIN: f32 = 4.0;

/// The highlight box (left, top, right, bottom) of a button whose glyph is centred in the
/// column `col` = (left, right): the tray-icon box ([`icon_box`]) plus [`BUTTON_MARGIN`] on
/// every side, centred on the taskbar's middle, and never wider than its column (so neighbouring
/// buttons' boxes don't overlap). Use it for every button's hover / pressed / state highlight.
pub fn button_box(col: (i32, i32), height: i32, scale: f32) -> (i32, i32, i32, i32) {
    let (icon_top, icon) = icon_box(height, scale);
    let m = (BUTTON_MARGIN * scale).round() as i32;
    let side = icon + 2 * m;
    let cx = (col.0 + col.1) / 2;
    let w = side.min(col.1 - col.0);
    let (l, t) = (cx - w / 2, icon_top - m);
    (l, t, l + w, t + side)
}

/// Draws the shared Windows 11 style text field into an opaque panel buffer.
pub fn text_box(
    buf: &mut [u32],
    width: i32,
    rect: (i32, i32, i32, i32),
    look: &Look,
    scale: f32,
    focused: bool,
) {
    let t = crate::theme::Theme::new(look);
    let radius = (look.control_radius * scale).round();
    fill_round(
        buf,
        width,
        (rect.0 as f32, rect.1 as f32, rect.2 as f32, rect.3 as f32),
        radius,
        t.color.input.tuple(),
        1.0,
    );
    let line_h = (if focused { 2.0 } else { 1.0 } * scale).round().max(1.0);
    let c = if focused {
        t.status.accent
    } else {
        t.color.input_border
    };
    fill_round(
        buf,
        width,
        (
            rect.0 as f32,
            rect.3 as f32 - line_h,
            rect.2 as f32,
            rect.3 as f32,
        ),
        radius.min(line_h),
        c.tuple(),
        1.0,
    );
}

/// Draws a highlighted scrolling-list row using the shared control radius.
pub fn list_row(
    buf: &mut [u32],
    width: i32,
    rect: (i32, i32, i32, i32),
    look: &Look,
    scale: f32,
    highlighted: bool,
) {
    if highlighted {
        let t = crate::theme::Theme::new(look);
        fill_round(
            buf,
            width,
            (rect.0 as f32, rect.1 as f32, rect.2 as f32, rect.3 as f32),
            (look.control_radius * scale).round(),
            t.color.hover.tuple(),
            1.0,
        );
    }
}

/// Draws a shared plain or accent button surface.
pub fn button(
    buf: &mut [u32],
    width: i32,
    rect: (i32, i32, i32, i32),
    look: &Look,
    scale: f32,
    accent: bool,
    hovered: bool,
) {
    let t = crate::theme::Theme::new(look);
    let c = if accent {
        if hovered {
            t.status.accent_hover
        } else {
            t.status.accent
        }
    } else if hovered {
        t.color.button_hover
    } else {
        t.color.button
    };
    fill_round(
        buf,
        width,
        (rect.0 as f32, rect.1 as f32, rect.2 as f32, rect.3 as f32),
        (look.control_radius * scale).round(),
        c.tuple(),
        1.0,
    );
}

/// The box (left, top, right, bottom) of a `size` px square at x `left`, centred vertically on
/// a strip `height` px tall: the cover or logo with its ring. Draw it and hit-test it with this.
pub fn centered_box(left: i32, size: i32, height: i32) -> (i32, i32, i32, i32) {
    let top = (height - size) / 2;
    (left, top, left + size, top + size)
}

/// `rect` grown by factor `k` about its centre (whole px, the centre stays put): the hovered or
/// pressed cover and its ring ([`crate::anim::Feel::value`]). 1.0 gives `rect` back.
pub fn grow_box((l, t, r, b): (i32, i32, i32, i32), k: f32) -> (i32, i32, i32, i32) {
    let (cx, cy) = ((l + r) as f32 / 2.0, (t + b) as f32 / 2.0);
    let (w, h) = (((r - l) as f32 * k).round(), ((b - t) as f32 * k).round());
    let (nl, nt) = ((cx - w / 2.0).round() as i32, (cy - h / 2.0).round() as i32);
    (nl, nt, nl + w as i32, nt + h as i32)
}

/// Redraws what is already in `rect` (cover or logo with its ring) grown by factor `k` about its
/// centre ([`grow_box`]), anti-aliased (bilinear, premultiplied): the hover / press feel
/// ([`crate::anim::Feel`]). Call it after drawing the cover and ring, before the highlight.
/// The pixels of `rect` move; anything around it stays under the grown copy's transparent parts.
/// Hit-test with the resting `rect`, not the grown one, so the edge doesn't flicker.
pub fn grow_pixels(buf: &mut [u32], width: i32, rect: (i32, i32, i32, i32), k: f32) {
    if (k - 1.0).abs() < 0.002 || width <= 0 {
        return;
    }
    let height = buf.len() as i32 / width;
    let (l, t, r, b) = (
        rect.0.max(0),
        rect.1.max(0),
        rect.2.min(width),
        rect.3.min(height),
    );
    let (w, h) = (r - l, b - t);
    if w <= 0 || h <= 0 {
        return;
    }
    // lift the source out, leaving its place empty
    let mut src = vec![0u32; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let p = &mut buf[((t + y) * width + l + x) as usize];
            src[(y * w + x) as usize] = std::mem::take(p);
        }
    }
    let at = |x: i32, y: i32| {
        if x < 0 || y < 0 || x >= w || y >= h {
            0
        } else {
            src[(y * w + x) as usize]
        }
    };
    let (cx, cy) = (
        (rect.0 + rect.2) as f32 / 2.0,
        (rect.1 + rect.3) as f32 / 2.0,
    );
    // (1 px wider than the rounded grown box: the scaled edge is fractional)
    let (gl, gt, gr, gb) = grow_box(rect, k);
    for y in (gt - 1).max(0)..(gb + 1).min(height) {
        for x in (gl - 1).max(0)..(gr + 1).min(width) {
            // the source point (in `src`, pixel centres at +0.5) this pixel shows
            let sx = (x as f32 + 0.5 - cx) / k + cx - l as f32 - 0.5;
            let sy = (y as f32 + 0.5 - cy) / k + cy - t as f32 - 0.5;
            let (x0, y0) = (sx.floor() as i32, sy.floor() as i32);
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let ws = [
                (1.0 - fx) * (1.0 - fy),
                fx * (1.0 - fy),
                (1.0 - fx) * fy,
                fx * fy,
            ];
            let ps = [
                at(x0, y0),
                at(x0 + 1, y0),
                at(x0, y0 + 1),
                at(x0 + 1, y0 + 1),
            ];
            let ch = |s: u32| {
                ps.iter()
                    .zip(ws)
                    .map(|(p, w)| ((p >> s) & 0xff) as f32 * w)
                    .sum::<f32>()
            };
            let sa = ch(24);
            if sa <= 0.0 {
                continue;
            }
            // over what is there (premultiplied "source over")
            let d = &mut buf[(y * width + x) as usize];
            let inv = 1.0 - sa / 255.0;
            let mix =
                |s: u32| ((ch(s) + ((*d >> s) & 0xff) as f32 * inv).round() as u32).min(255) << s;
            *d = mix(24) | mix(16) | mix(8) | mix(0);
        }
    }
}

/// Is point (`x`, `y`) inside `rect` = (left, top, right, bottom; right/bottom exclusive)?
pub fn contains((l, t, r, b): (i32, i32, i32, i32), x: i32, y: i32) -> bool {
    x >= l && x < r && y >= t && y < b
}

/// Paints colour `c` at `alpha` (0..1) *under* what is already in the buffer (so text and icons
/// stay on top), inside `rect` = (left, top, right, bottom) with `radius` corners, anti-aliased
/// like the hover highlight. For brand tints, backdrops and highlights in a widget's own colour
/// (the highlight itself is [`background_rect`]).
pub fn tint_box(
    buf: &mut [u32],
    width: i32,
    rect: (i32, i32, i32, i32),
    (r, g, b): (u32, u32, u32),
    alpha: f32,
    radius: f32,
) {
    let height = buf.len() as i32 / width.max(1);
    let shape = (rect.0 as f32, rect.1 as f32, rect.2 as f32, rect.3 as f32);
    for y in rect.1.max(0)..rect.3.min(height) {
        for x in rect.0.max(0)..rect.2.min(width) {
            let cover = shape_cover(x as f32 + 0.5, y as f32 + 0.5, shape, radius);
            let ta = alpha * cover;
            if ta <= 0.0 {
                continue;
            }
            let p = &mut buf[(y * width + x) as usize];
            let a = (*p >> 24) as f32 / 255.0;
            let under = ta * (1.0 - a); // only where the strip's own pixels don't cover
            let ch = |s: u32, c: u32| {
                ((((*p >> s) & 0xff) as f32 + c as f32 * under).round() as u32).min(255) << s
            };
            let na = ((a + under) * 255.0).round().min(255.0) as u32;
            *p = na << 24 | ch(16, r) | ch(8, g) | ch(0, b);
        }
    }
}

/// The background under everything: almost transparent (alpha 1 keeps the whole strip
/// clickable), with a rounded highlight like the taskbar's buttons (white ~10% on a dark taskbar,
/// black ~6% on a light one) over the rectangle `hover` = (left, top, right, bottom); None = no
/// highlight. The whole strip uses (0, inset, width, height - inset); a button uses [`button_box`].
pub fn background_rect(
    buf: &mut [u32],
    width: i32,
    height: i32,
    hover: Option<(i32, i32, i32, i32)>,
    light: bool,
    radius: f32,
) {
    let (bg_c, bg_a): (u32, u32) = match (hover.is_some(), light) {
        (true, false) => (255, 0x1a),
        (true, true) => (0, 0x10),
        _ => (0, 1),
    };
    let (hl, ht, hr, hb) = hover.unwrap_or((0, 0, width, height));
    let (x0, y0, x1, y1) = (hl as f32, ht as f32, hr as f32, hb as f32);
    let radius = radius.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let (cx, cy) = (
                fx.clamp(x0 + radius, x1 - radius),
                fy.clamp(y0 + radius, y1 - radius),
            );
            let inside =
                fy >= y0 && fy <= y1 && (fx - cx).powi(2) + (fy - cy).powi(2) <= radius * radius;
            let ba = if inside { bg_a } else { 1 };
            let bc = bg_c * ba / 255;
            let p = &mut buf[(y * width + x) as usize];
            let a = *p >> 24;
            let inv = 255 - a;
            let ch = |shift: u32| ((*p >> shift) & 0xff) + bc * inv / 255;
            *p = (a + ba * inv / 255) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0);
        }
    }
}

/// Tops (y) for two lines of text so they sit on the taskbar clock's baselines, given each
/// line's font ascent. `scale` = display scale × drag zoom (scaled around the middle, so the
/// zoom grows evenly).
pub fn two_line_tops(height: i32, scale: f32, ascent1: i32, ascent2: i32) -> (i32, i32) {
    let mid = height as f32 / 2.0;
    (
        (mid + CLOCK_BASELINES.0 * scale).round() as i32 - ascent1,
        (mid + CLOCK_BASELINES.1 * scale).round() as i32 - ascent2,
    )
}

/// The hover highlight's corner radius and inset (px) for a look at `scale`.
pub fn highlight_shape(look: &Look, scale: f32) -> (f32, i32) {
    (
        (look.control_radius * scale).round(),
        (4.0 * scale).round() as i32,
    )
}

/// The box (top y, size) for a widget's icon or badge, so it lines up with the tray icons:
/// [`TRAY_ICON`] px at `scale` (display scale × drag zoom), centred on the taskbar's middle.
pub fn icon_box(height: i32, scale: f32) -> (i32, i32) {
    let size = (TRAY_ICON * scale).round() as i32;
    ((height - size + 1) / 2, size)
}

/// The bounds (left, top, right, bottom; right/bottom exclusive) of the visible pixels of a
/// white-on-black coverage buffer, or None if it's empty.
pub fn ink_bounds(buf: &[u32], width: i32, height: i32) -> Option<(i32, i32, i32, i32)> {
    let (mut l, mut t, mut r, mut b) = (width, height, -1, -1);
    for y in 0..height {
        for x in 0..width {
            if cover(buf[(y * width + x) as usize]) > 8 {
                l = l.min(x);
                t = t.min(y);
                r = r.max(x);
                b = b.max(y);
            }
        }
    }
    (r >= 0).then_some((l, t, r + 1, b + 1))
}

fn cover(p: u32) -> u32 {
    ((p >> 16) & 0xff).max((p >> 8) & 0xff).max(p & 0xff)
}

/// Puts an icon into a strip so it lines up with the tray icons: `src` is the icon drawn
/// large, white on black (e.g. an emoji or symbol-font glyph at ~64 px). Its visible part is
/// scaled to fit the `size` px square at (`x`, `top`) (from [`icon_box`]), centred, and smoothly
/// downsampled into `dst`, also white on black, before [`text_to_color`].
pub fn fit_icon(
    dst: &mut [u32],
    dst_w: i32,
    x: i32,
    top: i32,
    size: i32,
    src: &[u32],
    src_w: i32,
    src_h: i32,
) {
    let Some((l, t, r, b)) = ink_bounds(src, src_w, src_h) else {
        return;
    };
    let (iw, ih) = ((r - l) as f32, (b - t) as f32);
    let k = iw.max(ih) / size as f32; // source px per destination px
                                      // centred in the box
    let (ox, oy) = (
        x as f32 + (size as f32 - iw / k) / 2.0,
        top as f32 + (size as f32 - ih / k) / 2.0,
    );
    let dst_h = dst.len() as i32 / dst_w.max(1);
    for dy in top.max(0)..(top + size).min(dst_h) {
        for dx in x.max(0)..(x + size).min(dst_w) {
            // the destination pixel's area in source coordinates; average the coverage there
            let (sx0, sy0) = (
                l as f32 + (dx as f32 - ox) * k,
                t as f32 + (dy as f32 - oy) * k,
            );
            let (sx1, sy1) = (sx0 + k, sy0 + k);
            let (mut sum, mut area) = (0.0f32, 0.0f32);
            let mut sy = sy0.floor() as i32;
            while (sy as f32) < sy1 {
                let hy = (sy1.min(sy as f32 + 1.0) - sy0.max(sy as f32)).max(0.0);
                let mut sx = sx0.floor() as i32;
                while (sx as f32) < sx1 {
                    let hx = (sx1.min(sx as f32 + 1.0) - sx0.max(sx as f32)).max(0.0);
                    if sx >= 0 && sy >= 0 && sx < src_w && sy < src_h {
                        sum += cover(src[(sy * src_w + sx) as usize]) as f32 * hx * hy;
                    }
                    area += hx * hy;
                    sx += 1;
                }
                sy += 1;
            }
            let a = (sum / area.max(1e-6)).round().min(255.0) as u32;
            let p = &mut dst[(dy * dst_w + dx) as usize];
            let v = cover(*p).max(a);
            *p = v << 16 | v << 8 | v;
        }
    }
}

/// Signed distance from `p` (relative to the centre) to a rounded rectangle with half sizes
/// `(hw, hh)` and corner radius `r`: negative inside.
fn rounded_rect_distance(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let (qx, qy) = (px.abs() - hw + r, py.abs() - hh + r);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

/// How far along a rounded rectangle's outline (half sizes `hw`, `hh`, corner radius `r`) the
/// point nearest to `p` is, from the top centre clockwise (y points down), and the outline's
/// whole length.
fn outline_position(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> (f32, f32) {
    use std::f32::consts::FRAC_PI_2;
    let (ex, ey) = ((hw - r).max(0.0), (hh - r).max(0.0)); // half lengths of the straight parts
    let arc = FRAC_PI_2 * r;
    let total = 4.0 * (ex + ey + arc);
    // starts of the segments, clockwise: top right half, corner, right, corner, bottom,
    // corner, left, corner, top left half
    let (tr, right, br, bottom, bl, left, tl) = (
        ex,
        ex + arc,
        ex + arc + 2.0 * ey,
        ex + 2.0 * arc + 2.0 * ey,
        3.0 * ex + 2.0 * arc + 2.0 * ey,
        3.0 * ex + 3.0 * arc + 2.0 * ey,
        3.0 * ex + 3.0 * arc + 4.0 * ey,
    );
    let s = match (px > ex, px < -ex, py > ey, py < -ey) {
        (true, _, _, true) => tr + (px - ex).atan2(-(py + ey)) * r,
        (true, _, true, _) => br + (py - ey).atan2(px - ex) * r,
        (_, true, true, _) => bl + (-(px + ex)).atan2(py - ey) * r,
        (_, true, _, true) => tl + (-(py + ey)).atan2(-(px + ex)) * r,
        _ => {
            // on a straight part: the nearest side, by how close the point is to each
            let (dx, dy) = (hw - px.abs(), hh - py.abs());
            if dy <= dx {
                if py < 0.0 {
                    if px >= 0.0 {
                        px
                    } else {
                        total + px
                    }
                } else {
                    bottom + (ex - px)
                }
            } else if px > 0.0 {
                right + (py + ey)
            } else {
                left + (ey - py)
            }
        }
    };
    (s.rem_euclid(total.max(1e-6)), total)
}

/// A progress ring along a rounded square, e.g. around a cover picture: it lies inside `rect`
/// (left, top, right, bottom; fractional px allowed, for smooth movement), `thickness` px wide, its outer corners `radius` px. From the top
/// centre clockwise, the first `fraction` (0..1) of it is `fill`, the rest `track`, each as
/// (r, g, b, alpha) with straight colour. Anti-aliased and painted over the premultiplied buffer
/// (after [`text_to_color`]).
pub fn progress_ring(
    buf: &mut [u32],
    width: i32,
    rect: (f32, f32, f32, f32),
    thickness: f32,
    radius: f32,
    fraction: f32,
    fill: (u32, u32, u32, u32),
    track: (u32, u32, u32, u32),
) {
    let (l, t, r, b) = rect;
    let height = buf.len() as i32 / width.max(1);
    let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
    let (hw, hh) = ((r - l) / 2.0, (b - t) / 2.0);
    let radius = radius.min(hw.min(hh));
    let inner_r = (radius - thickness).max(0.0);
    // the outline along the middle of the ring decides how far round each pixel is
    let (mw, mh, mr) = (
        hw - thickness / 2.0,
        hh - thickness / 2.0,
        (radius - thickness / 2.0).max(0.0),
    );
    let fraction = fraction.clamp(0.0, 1.0);
    for y in (t.floor() as i32).max(0)..(b.ceil() as i32).min(height) {
        for x in (l.floor() as i32).max(0)..(r.ceil() as i32).min(width) {
            let (px, py) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let outer = (0.5 - rounded_rect_distance(px, py, hw, hh, radius)).clamp(0.0, 1.0);
            let inner = (0.5
                + rounded_rect_distance(px, py, hw - thickness, hh - thickness, inner_r))
            .clamp(0.0, 1.0);
            let cover = outer * inner;
            if cover <= 0.0 {
                continue;
            }
            let (s, total) = outline_position(px, py, mw, mh, mr);
            // soft edge (about 1 px) where the filled part ends
            let f = if fraction >= 1.0 {
                1.0
            } else {
                (fraction * total - s + 0.5).clamp(0.0, 1.0)
            };
            let mix = |a: u32, b: u32| a as f32 * f + b as f32 * (1.0 - f);
            let alpha = mix(fill.3, track.3) / 255.0 * cover;
            let (cr, cg, cb) = (
                mix(fill.0, track.0),
                mix(fill.1, track.1),
                mix(fill.2, track.2),
            );
            let p = &mut buf[(y * width + x) as usize];
            let keep = 1.0 - alpha;
            let ch = |c: f32, shift: u32| {
                ((c * alpha + ((*p >> shift) & 0xff) as f32 * keep).round() as u32).min(255)
            };
            *p = ch(255.0, 24) << 24 | ch(cr, 16) << 16 | ch(cg, 8) << 8 | ch(cb, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_ring_fills_clockwise_from_the_top() {
        let (w, h) = (24, 24);
        let white = (255, 255, 255, 255);
        let red = (255, 0, 0, 255);
        let ring = |fraction: f32| {
            let mut buf = vec![0u32; (w * h) as usize];
            progress_ring(
                &mut buf,
                w,
                (0.0, 0.0, w as f32, h as f32),
                2.0,
                6.0,
                fraction,
                white,
                red,
            );
            buf
        };
        let px = |buf: &[u32], x: i32, y: i32| buf[(y * w + x) as usize];
        let half = ring(0.5);
        assert_eq!(px(&half, 23, 12), 0xffff_ffff); // right side: filled
        assert_eq!(px(&half, 0, 12), 0xffff_0000); // left side: track
        assert_eq!(px(&half, 14, 0), 0xffff_ffff); // just right of the top centre: filled
        assert_eq!(px(&half, 9, 0), 0xffff_0000); // just left of it: the end, track
        assert_eq!(px(&half, 12, 12), 0); // the middle stays untouched
        assert_eq!(px(&half, 12, 3), 0); // inside the ring too
        assert_eq!(px(&ring(0.0), 23, 12), 0xffff_0000);
        assert_eq!(px(&ring(1.0), 0, 12), 0xffff_ffff);
        // a quarter reaches the right side's middle, not beyond
        let q = ring(0.24);
        assert_eq!(px(&q, 23, 9), 0xffff_ffff);
        assert_eq!(px(&q, 23, 15), 0xffff_0000);
    }

    #[test]
    fn outline_position_runs_clockwise() {
        // a 20×20 square with 4 px corners: 4 × 12 straight + 2π·4 round
        let (s_top, total) = outline_position(0.0, -10.0, 10.0, 10.0, 4.0);
        assert!((total - (48.0 + 8.0 * std::f32::consts::PI)).abs() < 0.01);
        assert!(s_top.abs() < 0.01 || (s_top - total).abs() < 0.01);
        let s_right = outline_position(10.0, 0.0, 10.0, 10.0, 4.0).0;
        let s_bottom = outline_position(0.0, 10.0, 10.0, 10.0, 4.0).0;
        let s_left = outline_position(-10.0, 0.0, 10.0, 10.0, 4.0).0;
        assert!((s_right - total / 4.0).abs() < 0.01);
        assert!((s_bottom - total / 2.0).abs() < 0.01);
        assert!((s_left - total * 0.75).abs() < 0.01);
    }

    #[test]
    fn button_box_is_icon_plus_margin() {
        // 48 px taskbar at 100%: 16 px glyph + 4 px each side = 24 px square, centred
        let (l, t, r, b) = button_box((100, 124), 48, 1.0);
        assert_eq!((r - l, b - t), (24, 24));
        assert_eq!(((l + r) / 2, (t + b) / 2), (112, 24));
        // never wider than a narrower column
        let (l, _, r, _) = button_box((100, 110), 48, 1.0);
        assert_eq!((l, r), (100, 110));
        // scales with DPI / zoom
        let (l, t, r, b) = button_box((0, 60), 96, 2.0);
        assert_eq!((r - l, b - t), (48, 48));
    }

    #[test]
    fn grow_box_keeps_the_centre() {
        let ring = (4, 13, 26, 35); // 22 px
        assert_eq!(grow_box(ring, 1.0), ring);
        let g = grow_box(ring, 1.14); // 25 px
        assert_eq!((g.2 - g.0, g.3 - g.1), (25, 25));
        assert!(
            (g.0 + g.2 - (ring.0 + ring.2)).abs() <= 1
                && (g.1 + g.3 - (ring.1 + ring.3)).abs() <= 1
        );
        let s = grow_box(ring, 0.92); // 20 px
        assert_eq!((s.2 - s.0, s.3 - s.1), (20, 20));
    }

    #[test]
    fn grow_pixels_scales_about_the_centre() {
        let (w, h) = (40, 40);
        let mut buf = vec![0u32; (w * h) as usize];
        let rect = (10, 10, 30, 30);
        for y in 10..30 {
            for x in 10..30 {
                buf[(y * w + x) as usize] = 0xff_ff_ff_ff;
            }
        }
        buf[(20 * w + 35) as usize] = 0xff_00_00_ff; // a neighbour outside the grown box
        let before = buf.clone();
        grow_pixels(&mut buf, w, rect, 1.0);
        assert_eq!(buf, before); // 1.0: untouched
        grow_pixels(&mut buf, w, rect, 1.2); // 20 → 24 px: 8..32
        assert_eq!(buf[(20 * w + 9) as usize] >> 24, 255);
        assert_eq!(buf[(9 * w + 20) as usize] >> 24, 255);
        assert_eq!(buf[(20 * w + 6) as usize], 0);
        assert_eq!(buf[(20 * w + 35) as usize], 0xff_00_00_ff);
        // shrunk: the old edge is cleared
        let mut buf = before.clone();
        grow_pixels(&mut buf, w, rect, 0.9); // 18 px: 11..29
        assert_eq!(buf[(20 * w + 10) as usize], 0);
        assert_eq!(buf[(20 * w + 20) as usize], 0xff_ff_ff_ff);
    }

    #[test]
    fn centered_box_and_contains() {
        let b = centered_box(4, 22, 48);
        assert_eq!(b, (4, 13, 26, 35));
        assert!(contains(b, 4, 13) && contains(b, 25, 34));
        assert!(
            !contains(b, 3, 20)
                && !contains(b, 26, 20)
                && !contains(b, 10, 12)
                && !contains(b, 10, 35)
        );
    }

    #[test]
    fn tint_box_goes_under_and_keeps_outside() {
        let (w, h) = (40, 40);
        let mut buf = vec![0u32; (w * h) as usize];
        buf[(20 * w + 20) as usize] = 0xff_ff_ff_ff; // an opaque white pixel stays white
        tint_box(&mut buf, w, (10, 10, 30, 30), (0x58, 0x65, 0xF2), 0.5, 4.0);
        assert_eq!(buf[(20 * w + 20) as usize], 0xff_ff_ff_ff);
        let p = buf[(15 * w + 15) as usize];
        assert_eq!(p >> 24, 128); // half-transparent colour under nothing
        assert_eq!(buf[(5 * w + 5) as usize], 0); // outside: untouched
    }

    #[test]
    fn background_rect_highlights_only_its_box() {
        let (w, h) = (60, 48);
        let mut buf = vec![0u32; (w * h) as usize];
        background_rect(&mut buf, w, h, Some((20, 12, 44, 36)), false, 4.0);
        assert_eq!(buf[(24 * w + 30) as usize] >> 24, 0x1a);
        assert_eq!(buf[(6 * w + 30) as usize] >> 24, 1); // above the box
        assert_eq!(buf[(24 * w + 50) as usize] >> 24, 1);
    }

    #[test]
    fn concentric_and_card_radii() {
        assert_eq!(concentric(8.0, 4.0, 4.0), 4.0);
        assert_eq!(concentric(8.0, 12.0, 4.0), 4.0); // floor for controls
        assert_eq!(concentric(20.0, 4.0, 0.0), 16.0);
        let look = crate::style::Style::default().resolve(&crate::system::read());
        assert_eq!(card_radius(&look, 1.0, 20), 4.0); // small: the control radius
        assert_eq!(card_radius(&look, 1.0, 44), 6.0);
        assert_eq!(card_radius(&look, 1.0, 96), 12.0);
        assert_eq!(card_radius(&look, 1.0, 400), 12.0); // capped
        assert_eq!(card_radius(&look, 2.0, 192), 24.0);
    }

    #[test]
    fn small_corners_stay_circular_and_radius_is_clamped() {
        let r = (0.0, 0.0, 40.0, 40.0);
        // 4 px corner: same as the old circular formula
        let (fx, fy) = (1.5, 1.5);
        let (cx, cy): (f32, f32) = (4.0, 4.0);
        let old = (4.0 - ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0);
        assert!((shape_cover(fx, fy, r, 4.0) - old).abs() < 1e-6);
        // radius larger than half the side = capsule/circle, centre filled, corner empty
        assert_eq!(shape_cover(20.5, 20.5, r, 100.0), 1.0);
        assert_eq!(shape_cover(0.5, 0.5, r, 100.0), 0.0);
    }

    #[test]
    fn smooth_corner_is_seamless_and_as_deep_as_a_circle() {
        let r = (0.0, 0.0, 96.0, 96.0);
        let radius = 12.0;
        assert_eq!(shape_cover(48.5, 48.5, r, radius), 1.0); // middle
        assert_eq!(shape_cover(48.5, 0.5, r, radius), 1.0); // straight edge, as far in as a pixel centre
        assert_eq!(shape_cover(0.5, 0.5, r, radius), 0.0); // the very corner is cut
                                                           // along the top row the coverage rises monotonically and is full where the curve ends
        let row: Vec<f32> = (0..40)
            .map(|x| shape_cover(x as f32 + 0.5, 0.5, r, radius))
            .collect();
        assert!(row.windows(2).all(|w| w[1] >= w[0] - 1e-4));
        assert!(row[18] > 0.99); // reach = 1.5 × radius = 18 px
                                 // on the diagonal it cuts about as deep as a circular corner of the same radius
        let diag = |f: &dyn Fn(f32, f32) -> f32| {
            (0..30)
                .map(|k| k as f32 + 0.5)
                .find(|&k| f(k, k) >= 0.5)
                .unwrap()
        };
        let smooth = diag(&|x, y| shape_cover(x, y, r, radius));
        let circle = diag(&|x, y| shape_cover(x, y, r, 8.0)); // radius ≤ SMOOTH_ABOVE: circular arc
        let circle12 = circle * radius / 8.0;
        assert!(
            (smooth - circle12).abs() <= 1.0,
            "smooth {smooth} vs circle {circle12}"
        );
        // symmetric on all four corners
        let a = shape_cover(3.5, 5.5, r, radius);
        assert!((shape_cover(96.0 - 3.5, 5.5, r, radius) - a).abs() < 1e-5);
        assert!((shape_cover(3.5, 96.0 - 5.5, r, radius) - a).abs() < 1e-5);
        assert!((shape_cover(5.5, 3.5, r, radius) - shape_cover(3.5, 5.5, r, radius)).abs() < 1e-5);
    }

    #[test]
    fn fill_round_blends_and_stays_inside() {
        let (w, h) = (20, 20);
        let mut buf = vec![0xff00_0000u32; (w * h) as usize];
        fill_round(
            &mut buf,
            w,
            (2.0, 2.0, 18.0, 18.0),
            4.0,
            (255, 255, 255),
            1.0,
        );
        assert_eq!(buf[(10 * w + 10) as usize], 0xffff_ffff);
        assert_eq!(buf[(0 * w + 0) as usize], 0xff00_0000); // outside
        assert_eq!(buf[(2 * w + 2) as usize], 0xff00_0000); // cut corner
        let mut half = vec![0xff00_0000u32; (w * h) as usize];
        fill_round(
            &mut half,
            w,
            (2.0, 2.0, 18.0, 18.0),
            4.0,
            (200, 200, 200),
            0.5,
        );
        assert_eq!(half[(10 * w + 10) as usize] & 0xff, 100);
    }

    #[test]
    fn icon_box_matches_the_tray() {
        assert_eq!(icon_box(48, 1.0), (16, 16)); // rows 16..31, like the tray icons
        assert_eq!(icon_box(72, 1.5), (24, 24));
    }

    #[test]
    fn fit_icon_centres_the_ink_in_the_box() {
        // a 40×20 white bar off-centre in a 100×100 source
        let (sw, sh) = (100, 100);
        let mut src = vec![0u32; (sw * sh) as usize];
        for y in 10..30 {
            for x in 5..45 {
                src[(y * sw + x) as usize] = 0xffffff;
            }
        }
        let (dw, dh) = (40, 48);
        let mut dst = vec![0u32; (dw * dh) as usize];
        let (top, size) = icon_box(dh, 1.0);
        fit_icon(&mut dst, dw, 8, top, size, &src, sw, sh);
        // 16 wide, 8 tall, centred in the 16 px box at x 8..24, y 16..32 → rows 20..28
        assert_eq!(ink_bounds(&dst, dw, dh), Some((8, 20, 24, 28)));
    }
}
