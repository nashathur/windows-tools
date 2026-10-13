use crate::{config::Config, format, history::History, sampler::Snapshot};
use std::ffi::c_void;
use taskbar_dock::{
    block::{Canvas, Frame, PanelFrame},
    draw,
    style::Look,
    theme::{self, Rgb, Theme},
};
use windows::Win32::{
    Foundation::{COLORREF, RECT, SIZE},
    Graphics::Gdi::*,
};
fn rgb(c: Rgb) -> u32 {
    c.colorref()
}
fn font(style: theme::TextStyle, face: &str, scale: f32) -> HFONT {
    let mut lf = LOGFONTW {
        lfHeight: -style.px_at(scale).max(1),
        lfWeight: style.weight,
        lfQuality: CLEARTYPE_QUALITY,
        ..Default::default()
    };
    for (d, s) in lf.lfFaceName.iter_mut().zip(face.encode_utf16().take(31)) {
        *d = s
    }
    unsafe { CreateFontIndirectW(&lf) }
}
struct Fonts(Vec<HFONT>);
impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            for f in self.0.drain(..) {
                let _ = DeleteObject(HGDIOBJ(f.0));
            }
        }
    }
}
fn draw_text(
    dc: HDC,
    f: HFONT,
    text: &str,
    color: u32,
    r: (i32, i32, i32, i32),
    flags: DRAW_TEXT_FORMAT,
) {
    if text.is_empty() || r.2 <= 0 || r.3 <= 0 {
        return;
    }
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rc = RECT {
        left: r.0,
        top: r.1,
        right: r.0 + r.2,
        bottom: r.1 + r.3,
    };
    unsafe {
        let old = SelectObject(dc, HGDIOBJ(f.0));
        let _ = SetBkMode(dc, TRANSPARENT);
        let _ = SetTextColor(dc, COLORREF(color));
        let _ = DrawTextW(dc, &mut wide, &mut rc, flags | DT_NOPREFIX | DT_SINGLELINE);
        SelectObject(dc, old);
    }
}
fn text_extent(dc: HDC, f: HFONT, text: &str) -> (i32, i32) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE::default();
    unsafe {
        let old = SelectObject(dc, HGDIOBJ(f.0));
        let _ = GetTextExtentPoint32W(dc, &wide, &mut size);
        SelectObject(dc, old);
    }
    (size.cx, size.cy)
}
fn ascent(dc: HDC, f: HFONT) -> i32 {
    let mut metric = TEXTMETRICW::default();
    unsafe {
        let old = SelectObject(dc, HGDIOBJ(f.0));
        let _ = GetTextMetricsW(dc, &mut metric);
        SelectObject(dc, old);
    }
    metric.tmAscent
}
fn text_px(
    cv: &mut Canvas,
    font: HFONT,
    text: &str,
    color: u32,
    r: (i32, i32, i32, i32),
    flags: DRAW_TEXT_FORMAT,
) {
    draw_text(HDC(cv.dc as *mut c_void), font, text, 0x00FF_FFFF, r, flags);
    let (w, h) = (cv.w, cv.h);
    let cr = (color & 0xff) as u32;
    let cg = ((color >> 8) & 0xff) as u32;
    let cb = ((color >> 16) & 0xff) as u32;
    let pixels = cv.pixels();
    for y in r.1.max(0)..(r.1 + r.3).min(h) {
        for x in r.0.max(0)..(r.0 + r.2).min(w) {
            let p = &mut pixels[(y * w + x) as usize];
            let a = ((*p >> 16) & 255).max((*p >> 8) & 255).max(*p & 255);
            if a > 0 {
                *p = (a << 24) | (cr * a / 255 << 16) | (cg * a / 255 << 8) | (cb * a / 255);
            }
        }
    }
}
fn metric_color(theme: &Theme, key: &str, level: u8) -> Rgb {
    if level == 2 {
        theme.status.danger
    } else if level == 1 {
        theme.status.warn
    } else {
        theme.brand(key)
    }
}
#[cfg(test)]
mod color_tests {
    use super::*;
    #[test]
    fn cpu_graph_colour_matches_theme_channels() {
        let look = Look::current();
        let theme = Theme::new(&look);
        let graph = metric_color(&theme, "sysmon.cpu", 0);
        let brand = theme.brand("sysmon.cpu");
        assert_eq!(graph.tuple(), brand.tuple());
    }
}
fn pixel_color(c: Rgb) -> u32 {
    let (r, g, b) = c.tuple();
    0xff00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}
fn graph(
    buf: &mut [u32],
    stride: i32,
    rect: (i32, i32, i32, i32),
    series: &[Option<f64>],
    max: f64,
    line: Rgb,
    background: Rgb,
    fill: bool,
) {
    let (l, t, r, b) = rect;
    if r <= l + 1 || b <= t + 1 {
        return;
    }
    let span = (r - l - 2).max(1) as usize;
    let h = (b - t - 2).max(1);
    let mut prev: Option<(i32, i32)> = None;
    for (i, v) in series.iter().enumerate() {
        let x = l + 1 + (i * span / series.len().saturating_sub(1).max(1)) as i32;
        let Some(v) = v else {
            prev = None;
            continue;
        };
        let y = b - 2 - (v.clamp(0., max) / max * h as f64).round() as i32;
        if fill {
            for fy in y..b - 1 {
                if fy >= t && fy < b {
                    let idx = (fy * stride + x) as usize;
                    if idx < buf.len() {
                        let (r, g, bl) = line.tuple();
                        let base = background.tuple();
                        let mix =
                            |bg: u32, fg: u32| (bg as f32 * 0.72 + fg as f32 * 0.28).round() as u32;
                        buf[idx] = 0xff00_0000
                            | (mix(base.0, r) << 16)
                            | (mix(base.1, g) << 8)
                            | mix(base.2, bl);
                    }
                }
            }
        }
        if let Some((px, py)) = prev {
            let steps = (x - px).abs().max((y - py).abs()).max(1);
            for s in 0..=steps {
                let xx = px + (x - px) * s / steps;
                let yy = py + (y - py) * s / steps;
                if xx >= l && xx < r && yy >= t && yy < b {
                    buf[(yy * stride + xx) as usize] = pixel_color(line);
                }
            }
        } else if x >= l && x < r && y >= t && y < b {
            buf[(y * stride + x) as usize] = pixel_color(line)
        }
        prev = Some((x, y));
    }
}

fn overlay_line(
    buf: &mut [u32],
    stride: i32,
    rect: (i32, i32, i32, i32),
    values: &[Option<f64>],
    max: f64,
    color: Rgb,
) {
    let (l, t, r, b) = rect;
    if values.is_empty() || r <= l + 1 || b <= t + 1 {
        return;
    }
    let span = (r - l - 2).max(1) as usize;
    let h = (b - t - 2).max(1);
    let mut prev: Option<(i32, i32)> = None;
    for (i, v) in values.iter().enumerate() {
        let x = l + 1 + (i * span / values.len().saturating_sub(1).max(1)) as i32;
        let Some(v) = v else {
            prev = None;
            continue;
        };
        let y = b - 2 - (v.clamp(0., max) / max * h as f64).round() as i32;
        if let Some((px, py)) = prev {
            let steps = (x - px).abs().max((y - py).abs()).max(1);
            for s in 0..=steps {
                let xx = px + (x - px) * s / steps;
                let yy = py + (y - py) * s / steps;
                if xx >= l && xx < r && yy >= t && yy < b {
                    buf[(yy * stride + xx) as usize] = pixel_color(color);
                }
            }
        } else if x >= l && x < r && y >= t && y < b {
            buf[(y * stride + x) as usize] = pixel_color(color)
        }
        prev = Some((x, y));
    }
}
fn graph_box(
    buf: &mut [u32],
    w: i32,
    rect: (i32, i32, i32, i32),
    look: &Look,
    scale: f32,
    fill: Rgb,
    border: Rgb,
) -> (i32, i32, i32, i32) {
    let (l, t, r, b) = rect;
    let radius = draw::card_radius(look, scale, (r - l).min(b - t));
    draw::fill_round(
        buf,
        w,
        (l as f32, t as f32, r as f32, b as f32),
        radius,
        fill.tuple(),
        1.0,
    );
    let stroke = (scale.round() as i32).max(1);
    let inner = (l + stroke, t + stroke, r - stroke, b - stroke);
    let inner_radius = draw::concentric(radius, stroke as f32, 0.0);
    draw::fill_round(
        buf,
        w,
        (l as f32, t as f32, r as f32, b as f32),
        radius,
        border.tuple(),
        1.0,
    );
    draw::fill_round(
        buf,
        w,
        (
            inner.0 as f32,
            inner.1 as f32,
            inner.2 as f32,
            inner.3 as f32,
        ),
        inner_radius,
        fill.tuple(),
        1.0,
    );
    inner
}
fn get_hist(h: &History, range: u64, n: usize, ch: usize) -> Vec<Option<f64>> {
    h.view(range, n, ch)
}
fn spark_panel(
    cv: &mut Canvas,
    rect: (i32, i32, i32, i32),
    h: &History,
    range: u64,
    ch: usize,
    theme: &Theme,
    look: &Look,
    scale: f32,
    key: &str,
    level: u8,
    max: f64,
) -> (i32, i32, i32, i32) {
    let color = metric_color(theme, key, level);
    let bg = theme.color.input;
    let w = cv.w;
    let inner = graph_box(cv.pixels(), w, rect, look, scale, bg, theme.color.line);
    let vals = get_hist(h, range, (inner.2 - inner.0 - 2) as usize, ch);
    graph(cv.pixels(), w, inner, &vals, max, color, bg, true);
    inner
}

/// The strip laid out for its longest values ("CPU 100%", "↓ 999.9 MB/s"): one fixed width
/// whatever it shows (a width following the values twitched every second).
struct StripLayout {
    font: HFONT,
    ascent: i32,
    line_h: i32,
    pad: i32,
    col_gap: i32,
    /// label → value
    label_gap: i32,
    /// arrow → rate
    arrow_gap: i32,
    /// Full: CPU, GPU, RAM, network column widths. Compact: the two columns, then 0, 0.
    cols: [i32; 4],
    width: i32,
}

/// The texts on the strip for `s`: (label, value) per metric column, then the two rates.
fn strip_texts(s: &Snapshot) -> ([(&'static str, String); 3], String, String) {
    (
        [("CPU", format::pct(s.cpu)), ("GPU", format::pct(s.gpu)), ("RAM", format::pct(s.ram))],
        format::rate(s.down),
        format::rate(s.up),
    )
}

/// The full strip's width from its parts: `pad` at both ends, `gap` between the four columns.
fn full_width(cols: &[i32; 4], pad: i32, gap: i32) -> i32 {
    2 * pad + cols.iter().sum::<i32>() + 3 * gap
}

/// Spacing that keeps a strip of `natural` px at most `max` px: the column gap and the end padding
/// give up width first (down to `floor`); past that the strip is cut off by Block as usual.
fn fit_spacing(cols: &[i32; 4], pad: i32, gap: i32, floor: i32, max: i32) -> (i32, i32) {
    let over = full_width(cols, pad, gap) - max;
    if over <= 0 {
        return (pad, gap);
    }
    // 5 spaces share the excess: two ends, three gaps
    let cut = ((over + 4) / 5).min((pad - floor).max(0)).min((gap - floor).max(0));
    (pad - cut, gap - cut)
}

impl StripLayout {
    fn new(look: &Look, scale: f32, compact: bool, max_w: i32) -> Self {
        let widest = Snapshot {
            cpu: Some(100.0),
            gpu: Some(100.0),
            ram: Some(100.0),
            down: Some(999.9e6),
            up: Some(999.9e6),
            ..Default::default()
        };
        let s = &widest;
        let font = font(theme::TextStyle { px: look.font_size, weight: 400 }, &look.font, scale);
        let dc = unsafe { CreateCompatibleDC(None) };
        let m = |t: &str| text_extent(dc, font, t).0;
        let px = |v: f32| (v * scale).round() as i32;
        let (pad, col_gap) = (px(theme::space::TIGHT), px(theme::space::TIGHT));
        let (label_gap, arrow_gap) = (px(theme::space::XS), px(theme::space::XS / 2.0));
        let (metrics, down, up) = strip_texts(s);
        let mut cols = [0; 4];
        let width;
        let (mut pad_used, mut gap_used) = (pad, col_gap);
        if compact {
            let c1 = m(&format!("C {}", metrics[0].1)).max(m(&format!("G {}", metrics[1].1)));
            let c2 = m(&format!("R {}", metrics[2].1)).max(m(&format!("↓{}", format::compact_rate(s.down))));
            cols = [c1, c2, 0, 0];
            width = 2 * pad + c1 + col_gap + c2;
        } else {
            for (i, (label, value)) in metrics.iter().enumerate() {
                cols[i] = m(label) + label_gap + m(value);
            }
            cols[3] = m("↓") + arrow_gap + m(&down).max(m(&up));
            (pad_used, gap_used) = fit_spacing(&cols, pad, col_gap, px(theme::space::XS / 2.0), max_w);
            // never past the setting: below the spacing floor Block cuts off the right end
            width = full_width(&cols, pad_used, gap_used).min(max_w);
        }
        let ascent = ascent(dc, font);
        let line_h = text_extent(dc, font, "CPU 100%").1;
        unsafe {
            let _ = DeleteDC(dc);
        }
        Self { font, ascent, line_h, pad: pad_used, col_gap: gap_used, label_gap, arrow_gap, cols, width }
    }
}

pub fn measure(frame: &Frame, compact: bool, max_width: f64) -> i32 {
    let max_w = (max_width as f32 * frame.scale).round() as i32;
    let l = StripLayout::new(frame.look, frame.scale, compact, max_w);
    unsafe {
        let _ = DeleteObject(HGDIOBJ(l.font.0));
    }
    l.width
}

pub fn strip(cv: &mut Canvas, f: &Frame, s: &Snapshot, h: &History, c: &Config, paused: bool) {
    let theme = Theme::new(f.look);
    let scale = f.scale;
    let max_w = (c.max_width as f32 * scale).round() as i32;
    let l = StripLayout::new(f.look, scale, f.compact, max_w);
    let _fonts = Fonts(vec![l.font]);
    let tf = l.font;
    let dim = rgb(theme.strip.dim);
    let fg = rgb(theme.strip.fg);
    let (top1, top2) = draw::two_line_tops(cv.h, scale, l.ascent, l.ascent);
    let dc = unsafe { CreateCompatibleDC(None) };
    let extent = |text: &str| text_extent(dc, tf, text).0;
    let level = |v: Option<f64>, gpu: bool| {
        format::level(v, c.warn_pct, c.crit_pct)
            .max(u8::from(gpu && s.gpu_temp.is_some_and(|t| t > c.gpu_temp_warn)))
    };
    let ink = |key: &str, lv: u8| if paused { dim } else { rgb(metric_color(&theme, key, lv)) };
    if f.compact {
        let x1 = l.pad;
        let x2 = x1 + l.cols[0] + l.col_gap;
        for (text, val, key, top, gpu) in [
            ("C", s.cpu, "sysmon.cpu", top1, false),
            ("G", s.gpu, "sysmon.gpu", top2, true),
        ] {
            let t = format!("{text} {}", format::pct(val));
            text_px(cv, tf, &t, ink(key, level(val, gpu)), (x1, top, l.cols[0], l.line_h), DT_LEFT);
        }
        let t = format!("R {}", format::pct(s.ram));
        text_px(cv, tf, &t, ink("sysmon.ram", level(s.ram, false)), (x2, top1, l.cols[1], l.line_h), DT_LEFT);
        let t = format!("↓{}", format::compact_rate(s.down));
        text_px(cv, tf, &t, if paused { dim } else { fg }, (x2, top2, l.cols[1], l.line_h), DT_LEFT);
    } else {
        let (metrics, down, up) = strip_texts(s);
        let mut x = l.pad;
        for (i, ((label, value), key)) in metrics.iter().zip(["sysmon.cpu", "sysmon.gpu", "sysmon.ram"]).enumerate() {
            let v = [s.cpu, s.gpu, s.ram][i];
            let lv = level(v, i == 1);
            let col = l.cols[i];
            let label_w = extent(label);
            text_px(cv, tf, label, dim, (x, top1, label_w, l.line_h), DT_LEFT);
            let vx = x + label_w + l.label_gap;
            text_px(cv, tf, value, ink(key, lv), (vx, top1, x + col - vx, l.line_h), DT_LEFT);
            // the sparkline box fills line 2's text cell under the column
            let color = if paused { theme.strip.dim } else { metric_color(&theme, key, lv) };
            let rect = (x, top2, x + col, top2 + l.line_h);
            let (cvw, cvh) = (cv.w, cv.h);
            let inner = graph_box(cv.pixels(), cvw, rect, f.look, scale, theme.color.input, theme.color.line);
            let vals = get_hist(h, 60, (inner.2 - inner.0).max(1) as usize, i);
            graph(cv.pixels(), cvw, inner, &vals, 100.0, color, theme.color.input, true);
            let p = cv.pixels();
            let mid = (rect.1 + rect.3) / 2;
            let dash = (theme::space::XS * scale).round().max(1.0) as i32;
            for xx in inner.0..inner.2 {
                if (xx - inner.0) % (dash * 2) < dash && mid >= 0 && mid < cvh {
                    p[(mid * cvw + xx) as usize] = pixel_color(theme.color.faint);
                }
            }
            x += col + l.col_gap;
        }
        let arrow_w = extent("↓");
        for (arrow, rate, color, top) in [
            ("↓", &down, theme.brand("sysmon.down"), top1),
            ("↑", &up, theme.brand("sysmon.up"), top2),
        ] {
            text_px(cv, tf, arrow, if paused { dim } else { rgb(color) }, (x, top, arrow_w, l.line_h), DT_LEFT);
            let rx = x + arrow_w + l.arrow_gap;
            text_px(cv, tf, rate, if paused { dim } else { fg }, (rx, top, l.cols[3] - arrow_w - l.arrow_gap, l.line_h), DT_LEFT);
        }
    }
    unsafe {
        let _ = GdiFlush();
        let _ = DeleteDC(dc);
    }
}
pub fn panel_size(scale: f32) -> (i32, i32) {
    panel_layout(&Look::current(), scale).size
}
fn txt(
    dc: HDC,
    f: HFONT,
    s: &str,
    c: Rgb,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    flags: DRAW_TEXT_FORMAT,
) {
    draw_text(dc, f, s, c.colorref(), (x, y, w, h), flags)
}
fn draw_card(bg: &mut [u32], w: i32, r: (i32, i32, i32, i32), theme: &Theme, look: &Look, scale: f32) {
    draw::fill_round(
        bg,
        w,
        (r.0 as f32, r.1 as f32, r.2 as f32, r.3 as f32),
        draw::card_radius(look, scale, (r.2 - r.0).min(r.3 - r.1)),
        theme.color.input.tuple(),
        1.0,
    )
}
#[derive(Clone, Copy, Debug)]
struct Rect(i32, i32, i32, i32);
impl Rect {
    fn width(self) -> i32 { self.2 - self.0 }
    fn height(self) -> i32 { self.3 - self.1 }
    fn contains(self, x: i32, y: i32) -> bool { x >= self.0 && x < self.2 && y >= self.1 && y < self.3 }
}
#[derive(Clone, Debug)]
struct PanelControl { id: usize, label: &'static str, rect: Rect }
#[derive(Clone, Debug)]
struct CardLayout { rect: Rect, title: Rect, value: Rect, graph: Rect, rows: Vec<(Rect, Rect)>, bars: Rect }
#[derive(Clone, Debug)]
struct PanelLayout { size: (i32, i32), title: Rect, controls: Vec<PanelControl>, cards: [CardLayout; 4] }

fn panel_layout(look: &Look, scale: f32) -> PanelLayout {
    let scale_px = |value: f32| (value * scale).round() as i32;
    let title_font = font(theme::TITLE, &look.font, scale);
    let small_font = font(theme::CAPTION, &look.font, scale);
    let dc = unsafe { CreateCompatibleDC(None) };
    let measure = |f, text: &str| text_extent(dc, f, text).0;
    let pad = scale_px(theme::space::PANEL_PADDING);
    let inset = scale_px(theme::space::S);
    let mid = scale_px(theme::space::M);
    let tiny = scale_px(theme::space::XS);
    let row = scale_px(theme::space::ROW_HEIGHT);
    let graph_h = scale_px(theme::space::ROW_HEIGHT * 1.25);
    let task_w = measure(small_font, "Task Manager") + mid * 2;
    let reset_w = measure(small_font, "Reset graphs") + mid * 2;
    let mut controls = Vec::new();
    let mut x = pad;
    let header2_y = pad + row + mid;
    for (label, id) in [("1m", 10), ("5m", 11), ("1h", 12)] {
        let width = measure(small_font, label) + inset * 2;
        controls.push(PanelControl { id, label, rect: Rect(x, header2_y, x + width, header2_y + row) });
        x += width + tiny;
    }
    x += mid - tiny;
    for (label, id) in [("High", 20), ("Normal", 21), ("Low", 22), ("Paused", 23)] {
        let width = measure(small_font, label) + inset * 2;
        controls.push(PanelControl { id, label, rect: Rect(x, header2_y, x + width, header2_y + row) });
        x += width + tiny;
    }
    let reset_left = x + mid;
    controls.push(PanelControl { id: 30, label: "Reset graphs", rect: Rect(reset_left, header2_y, reset_left + reset_w, header2_y + row) });
    let row2_w = reset_left + reset_w - pad;
    let card_samples = [
        ("Committed", "99.9 / 99.9 GB"),
        ("Session ↓ / ↑", "999.9 GB / 999.9 GB"),
        ("Temp · power", "100 °C · 999 W"),
        ("Clock · fan", "9.99 GHz · 100%"),
    ];
    let content_w = card_samples.iter().map(|(label, value)| measure(small_font, label) + mid + measure(small_font, value)).max().unwrap_or(0);
    let card_w = content_w + inset * 2;
    let cards_gap = mid;
    let panel_w = (row2_w + pad * 2).max(card_w * 2 + cards_gap + pad * 2);
    let task_left = panel_w - pad - task_w;
    controls[7].rect = Rect(panel_w - pad - reset_w, header2_y, panel_w - pad, header2_y + row);
    controls.push(PanelControl { id: 31, label: "Task Manager", rect: Rect(task_left, pad, task_left + task_w, pad + row) });
    let card_h = inset * 2 + row + graph_h + row * 4 + tiny * 5;
    let cards_top = header2_y + row + mid;
    let card_rects = [
        Rect(pad, cards_top, pad + card_w, cards_top + card_h),
        Rect(pad + card_w + cards_gap, cards_top, pad + card_w * 2 + cards_gap, cards_top + card_h),
        Rect(pad, cards_top + card_h + cards_gap, pad + card_w, cards_top + card_h * 2 + cards_gap),
        Rect(pad + card_w + cards_gap, cards_top + card_h + cards_gap, pad + card_w * 2 + cards_gap, cards_top + card_h * 2 + cards_gap),
    ];
    let cards = card_rects.map(|r| {
        let left = r.0 + inset;
        let right = r.2 - inset;
        let title_y = r.1 + tiny;
        let graph_y = title_y + row + tiny;
        let row_y = graph_y + graph_h + tiny;
        let rows = (0..4).map(|i| {
            let y = row_y + i * (row + tiny);
            let label_w = (right - left) / 2;
            (Rect(left, y, left + label_w, y + row), Rect(left + label_w, y, right, y + row))
        }).collect();
        let value_w = measure(small_font, "999.9 MB/s").max(measure(small_font, "100%")) + mid;
        CardLayout {
            rect: r,
            title: Rect(left, title_y, right - value_w, title_y + row),
            value: Rect(right - value_w, title_y, right, title_y + row),
            graph: Rect(left, graph_y, right, graph_y + graph_h),
            rows,
            bars: Rect(left + (right - left) / 3, row_y, right, row_y + row),
        }
    });
    let panel_h = cards_top + card_h * 2 + cards_gap + pad;
    let title = Rect(pad, pad, pad + measure(title_font, "System"), pad + row);
    unsafe {
        let _ = DeleteDC(dc);
        let _ = DeleteObject(HGDIOBJ(title_font.0));
        let _ = DeleteObject(HGDIOBJ(small_font.0));
    }
    PanelLayout { size: (panel_w, panel_h), title, controls, cards }
}

fn panel_hit(layout: &PanelLayout, x: i32, y: i32) -> Option<usize> {
    layout.controls.iter().find(|c| c.rect.contains(x, y)).map(|c| c.id)
}

pub fn panel_control_hit(look: &Look, scale: f32, x: i32, y: i32) -> Option<usize> {
    panel_hit(&panel_layout(look, scale), x, y)
}

pub fn panel(
    cv: &mut Canvas, f: &PanelFrame, s: &Snapshot, h: &History, c: &Config,
    range: &str, paused: bool, hover: Option<usize>, pressed: Option<usize>, look: &Look,
) {
    let theme = f.theme();
    let scale = f.scale;
    let layout = panel_layout(look, scale);
    let dc = HDC(cv.dc as *mut c_void);
    let title_font = font(theme::TITLE, &look.font, scale);
    let small_font = font(theme::CAPTION, &look.font, scale);
    let _fonts = Fonts(vec![title_font, small_font]);
    let (tf, sf) = (_fonts.0[0], _fonts.0[1]);
    let (radius, _) = draw::highlight_shape(look, scale);
    let (w, height) = (cv.w, cv.h);
    txt(dc, tf, "System", theme.color.fg, layout.title.0, layout.title.1, layout.title.width(), layout.title.height(), DT_LEFT | DT_VCENTER);
    let ranges = ["1m", "5m", "1h"];
    let selected_range = ranges.iter().position(|v| *v == range).unwrap_or(0);
    let speed_index = if paused { 3 } else { match c.speed.as_str() { "high" => 0, "low" => 2, _ => 1 } };
    for group in [&layout.controls[0..3], &layout.controls[3..7]] {
        if let (Some(first), Some(last)) = (group.first(), group.last()) {
            let track = Rect(first.rect.0 - (theme::space::XS * scale).round() as i32, first.rect.1, last.rect.2 + (theme::space::XS * scale).round() as i32, last.rect.3);
            draw::fill_round(cv.pixels(), w, (track.0 as f32, track.1 as f32, track.2 as f32, track.3 as f32), radius, theme.color.button.tuple(), 1.0);
        }
    }
    // the segmented range / speed controls (the two action buttons are drawn below)
    for control in layout.controls.iter().filter(|c| c.id != 30 && c.id != 31) {
        let selected = match control.id { 10..=12 => control.id - 10 == selected_range, 20..=23 => control.id - 20 == speed_index, _ => false };
        let is_hover = hover == Some(control.id) || pressed == Some(control.id);
        if selected || is_hover {
            let fill = if selected { theme.color.button_hover } else { theme.color.hover };
            let rect = control.rect;
            draw::fill_round(cv.pixels(), w, (rect.0 as f32, rect.1 as f32, rect.2 as f32, rect.3 as f32), draw::concentric(radius, (theme::space::XS * scale).round(), 0.0), fill.tuple(), 1.0);
        }
        let color = if selected || is_hover { theme.color.fg } else { theme.color.dim };
        let rect = control.rect;
        txt(dc, sf, control.label, color, rect.0, rect.1, rect.width(), rect.height(), DT_CENTER | DT_VCENTER);
    }
    for control in layout.controls.iter().filter(|c| c.id == 30 || c.id == 31) {
        let rect = control.rect;
        let active = hover == Some(control.id) || pressed == Some(control.id);
        // the highlight is the hit box (UI-RULES.md "Button highlights")
        let highlight = active.then_some((rect.0, rect.1, rect.2, rect.3));
        draw::background_rect(cv.pixels(), w, height, highlight, theme.light, radius);
        txt(dc, sf, control.label, theme.color.fg, rect.0, rect.1, rect.width(), rect.height(), DT_CENTER | DT_VCENTER);
    }
    let seconds = match range { "5m" => 300, "1h" => 3600, _ => 60 };
    let levels = [
        format::level(s.cpu, c.warn_pct, c.crit_pct),
        format::level(s.gpu, c.warn_pct, c.crit_pct).max(if s.gpu_temp.is_some_and(|v| v > c.gpu_temp_warn) { 1 } else { 0 }),
        format::level(s.ram, c.warn_pct, c.crit_pct),
        0,
    ];
    let keys = ["sysmon.cpu", "sysmon.gpu", "sysmon.ram", "sysmon.down"];
    let titles = [
        format!("CPU · {}", s.cpu_name.as_deref().unwrap_or("CPU")),
        format!("GPU · {}", s.gpu_name.as_deref().unwrap_or("—")),
        "Memory".to_owned(),
        format!("Network · {}", s.network.alias.as_deref().unwrap_or("—")),
    ];
    let values = [format::pct(s.cpu), format::pct(s.gpu), format::pct(s.ram), format::rate(s.down)];
    for (i, card) in layout.cards.iter().enumerate() {
        let r = card.rect;
        draw_card(cv.pixels(), w, (r.0, r.1, r.2, r.3), &theme, look, scale);
        let col = metric_color(&theme, keys[i], levels[i]);
        txt(dc, sf, &titles[i], theme.color.fg, card.title.0, card.title.1, card.title.width(), card.title.height(), DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
        txt(dc, sf, &values[i], col, card.value.0, card.value.1, card.value.width(), card.value.height(), DT_RIGHT | DT_VCENTER);
        let graph_rect = (card.graph.0, card.graph.1, card.graph.2, card.graph.3);
        if i == 3 {
            // ↓ and ↑ in one graph, scaled to the busiest moment of either in the range
            let samples = (graph_rect.2 - graph_rect.0).max(1) as usize;
            let up = h.view(seconds, samples, 4);
            let top = h.view(seconds, samples, 3).iter().chain(up.iter()).flatten().copied().fold(1.0_f64, f64::max);
            let plot = spark_panel(cv, graph_rect, h, seconds, 3, &theme, look, scale, keys[i], 0, top);
            let cvw = cv.w;
            overlay_line(cv.pixels(), cvw, plot, &up, top, theme.brand("sysmon.up"));
        } else {
            spark_panel(cv, graph_rect, h, seconds, i, &theme, look, scale, keys[i], levels[i], 100.0);
        }
        let labels: [&str; 4];
        let vals: [String; 4];
        match i {
            0 => {
                labels = ["Threads", "Clock", "Temp", "",];
                vals = ["".to_owned(), s.cpu_clock.map(|v| format!("{v:.2} GHz")).unwrap_or_else(|| "—".into()), s.cpu_temp.map(|v| format!("{v:.0} °C")).unwrap_or_else(|| "—".into()), "".to_owned()];
                let n = s.cpu_threads.iter().flatten().count();
                if n > 0 {
                    let values: Vec<f64> = s.cpu_threads.iter().flatten().copied().collect();
                    let slot = card.bars.width() / n as i32;
                    // a thin gap between bars; corners a quarter of the bar, never deeper than the fill
                    let bar = (slot - (scale.round() as i32).max(1)).max(1);
                    let top = card.bars.1 + (theme::space::XS * scale).round() as i32;
                    let bottom = card.bars.3 - (theme::space::XS * scale).round() as i32;
                    for (idx, value) in values.iter().enumerate() {
                        let x = card.bars.0 + idx as i32 * slot;
                        let fill_top = bottom - ((bottom - top) as f64 * value.clamp(0.0, 100.0) / 100.0).round() as i32;
                        let r = bar as f32 / 4.0;
                        draw::fill_round(cv.pixels(), w, (x as f32, top as f32, (x + bar) as f32, bottom as f32), r, theme.color.track.tuple(), 1.0);
                        if bottom > fill_top {
                            let rf = r.min((bottom - fill_top) as f32 / 2.0);
                            draw::fill_round(cv.pixels(), w, (x as f32, fill_top as f32, (x + bar) as f32, bottom as f32), rf, theme.brand("sysmon.cpu").tuple(), 1.0);
                        }
                    }
                }
            }
            1 => { labels = ["VRAM", "Temp · power", "Clock · fan", ""]; vals = [format!("{} / {}", format::gb(s.gpu_mem_used), format::gb(s.gpu_mem_total)), format!("{} · {}", s.gpu_temp.map(|v| format!("{v:.0} °C")).unwrap_or_else(|| "—".into()), s.gpu_power.map(|v| format!("{v:.0} W")).unwrap_or_else(|| "—".into())), format!("{} · {}%", s.gpu_clock.map(|v| format!("{v:.2} GHz")).unwrap_or_else(|| "—".into()), s.gpu_fan.map(|v| format!("{v:.0}")).unwrap_or_else(|| "—".into())), String::new()]; }
            2 => { labels = ["In use", "Committed", "Cached", ""]; vals = [format!("{} / {}", format::gb(s.memory.used), format::gb(s.memory.total)), format!("{} / {}", format::gb(s.memory.committed), format::gb(s.memory.commit_limit)), format!("{}", format::gb(s.memory.cached)), String::new()]; }
            _ => { labels = ["Down", "Up", "Link", "Session ↓ / ↑"]; vals = [format::rate(s.down), format::rate(s.up), s.network.link.map(|v| format!("{} Mb/s", v / 1_000_000)).unwrap_or_else(|| "—".into()), format!("{} / {}", format::bytes(s.network.total_down), format::bytes(s.network.total_up))]; }
        }
        for ri in 0..4 {
            let (label_rect, value_rect) = card.rows[ri];
            let label = if i == 0 && ri == 0 { "Threads" } else { labels[ri] };
            if !label.is_empty() {
                txt(dc, sf, label, theme.color.dim, label_rect.0, label_rect.1, label_rect.width(), label_rect.height(), DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
                if !(i == 0 && ri == 0) {
                    // temperatures take the warning / critical colour past their limits
                    let lv = match (i, ri) {
                        (0, 2) => format::level(s.cpu_temp, c.cpu_temp_warn, c.cpu_temp_crit),
                        (1, 1) => u8::from(s.gpu_temp.is_some_and(|t| t > c.gpu_temp_warn)),
                        _ => 0,
                    };
                    let ink = match lv { 2 => theme.status.danger, 1 => theme.status.warn, _ => theme.color.fg };
                    txt(dc, sf, &vals[ri], ink, value_rect.0, value_rect.1, value_rect.width(), value_rect.height(), DT_RIGHT | DT_VCENTER | DT_END_ELLIPSIS);
                }
            }
        }
    }
    unsafe { let _ = GdiFlush(); }
}

#[cfg(test)]
mod panel_layout_tests {
    use super::*;
    #[test]
    fn controls_and_cards_fit_without_header_overlap_at_supported_scales() {
        let look = Look::current();
        for scale in [1.0, 1.5] {
            let layout = panel_layout(&look, scale);
            println!("panel_layout scale={scale:.1}: {}x{}", layout.size.0, layout.size.1);
            for (i, a) in layout.controls.iter().enumerate() {
                assert!(a.rect.0 >= 0 && a.rect.1 >= 0 && a.rect.2 <= layout.size.0 && a.rect.3 <= layout.size.1);
                for b in layout.controls.iter().skip(i + 1) {
                    if a.rect.1 == b.rect.1 {
                        assert!(a.rect.2 <= b.rect.0 || b.rect.2 <= a.rect.0, "header controls overlap: {:?} {:?}", a, b);
                    }
                }
                let center = (a.rect.0 + a.rect.2) / 2;
                let y = (a.rect.1 + a.rect.3) / 2;
                assert_eq!(panel_hit(&layout, center, y), Some(a.id));
            }
            assert!(layout.title.0 >= 0 && layout.title.1 >= 0 && layout.title.2 <= layout.size.0 && layout.title.3 <= layout.size.1);
            for card in &layout.cards {
                assert!(card.rect.0 >= 0 && card.rect.1 >= 0 && card.rect.2 <= layout.size.0 && card.rect.3 <= layout.size.1);
            }
        }
    }
}

pub fn tray(cv: &mut Canvas, look: &Look) -> bool {
    let t = Theme::new(look);
    let f = font(theme::ICON, t.font.icon, 1.);
    let dc = HDC(cv.dc as *mut c_void);
    txt(
        dc,
        f,
        "\u{E9D9}",
        t.color.fg,
        0,
        0,
        cv.w,
        cv.h,
        DT_CENTER | DT_VCENTER,
    );
    unsafe {
        let _ = DeleteObject(HGDIOBJ(f.0));
        let _ = GdiFlush();
    }
    let (w, h) = (cv.w, cv.h);
    draw::text_to_color(cv.pixels(), w, h, |_| look.text_rgb().0);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measured_strip_widths_with_the_taskbar_font() {
        // the user's taskbar look (font from Windows), at 12 px
        let mut look = taskbar_dock::style::Style::default().resolve(&taskbar_dock::system::read());
        look.font_size = 12.0;
        let max = 254;
        let full = StripLayout::new(&look, 1.0, false, max);
        let compact = StripLayout::new(&look, 1.0, true, max);
        unsafe {
            let _ = DeleteObject(HGDIOBJ(full.font.0));
            let _ = DeleteObject(HGDIOBJ(compact.font.0));
        }
        println!("12 px strip widths at scale 1.0: full={}, compact={}", full.width, compact.width);
        assert!(full.width <= max, "the longest values fit the maximum");
        assert!(compact.width < full.width);
    }

    #[test]
    fn spacing_gives_way_before_the_maximum_is_passed() {
        let cols = [56, 56, 56, 68];
        assert_eq!(fit_spacing(&cols, 6, 6, 2, 300), (6, 6), "room to spare: unchanged");
        let natural = full_width(&cols, 6, 6);
        let (pad, gap) = fit_spacing(&cols, 6, 6, 2, natural - 10);
        assert!(full_width(&cols, pad, gap) <= natural - 10);
        assert_eq!(fit_spacing(&cols, 6, 6, 2, 0), (2, 2), "never below the floor");
    }
}
