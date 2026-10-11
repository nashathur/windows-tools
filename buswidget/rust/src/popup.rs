//! The departures panel: its content and drawing (Block owns the window, its opening and fade).

use super::*;
use taskbar_dock::theme::{size, space};

/// One departure: minutes to go, destination, clock time, and its status in its colour.
#[derive(PartialEq, Debug)]
pub(crate) struct PopRow {
    mins: String,
    dest: String,
    clock: String,
    status: String,
    color: u32,
    cancelled: bool,
    /// Timetable only (no live data): no status dot.
    scheduled: bool,
}

/// Header (title, subtitle), departure rows, footer, and the line shown when there are no rows.
#[derive(PartialEq, Debug)]
pub(crate) struct PopContent {
    title: String,
    subtitle: String,
    rows: Vec<PopRow>,
    foot: String,
    empty: &'static str,
}

pub(crate) fn popup_content(look: &Look) -> PopContent {
    let (cfg, sched, live, ok_at, err) = current();
    let t = Theme::new(look);
    let empty = if sched.is_none() && live.is_empty() {
        "Waiting for the timetable download"
    } else {
        "No departures in the next 6 hours"
    };
    let now = data::now();
    let mut rows = Vec::new();
    for u in data::upcoming(sched.as_deref(), &live, now).into_iter().take(SHOW_IN_POPUP) {
        let delay = ((u.t - u.plan) as f64 / 60.0).round() as i64;
        // live on time / early: ok, late: warn, timetable only: dim
        let (status, color) = if u.cancel {
            ("Cancelled".to_string(), t.status.danger)
        } else if u.live {
            match delay {
                0 => ("On time".to_string(), t.status.ok),
                d if d > 0 => (format!("+{d} min late"), t.status.warn),
                d => (format!("{} min early", -d), t.status.ok),
            }
        } else {
            ("Scheduled".to_string(), t.color.dim)
        };
        let mins = (u.t - now).div_euclid(60);
        let m = if mins < 1 { "now".to_string() } else { format!("{mins} min") };
        let dest = if cfg.dest.is_empty() { u.head.as_str() } else { cfg.dest.as_str() };
        rows.push(PopRow { mins: m, dest: dest.to_string(), clock: clock_text(u.t, false, look), status, color: color.colorref(), cancelled: u.cancel, scheduled: !u.cancel && !u.live });
    }
    let no_live = shared().lock().unwrap().no_live;
    let mut foot = match ok_at {
        _ if no_live => "no live data for this stop, timetable times only".to_string(),
        Some(t) => format!("live data {}", clock_text(t, true, look)),
        None => "no live data yet".to_string(),
    };
    if let Some(e) = err {
        let e: String = e.chars().take(60).collect();
        foot += &format!(" · last error: {e}");
    }
    foot += " · ~ = scheduled only";
    let title = if cfg.label.is_empty() { format!("Line {}", cfg.line) } else { cfg.label.clone() };
    PopContent { title, subtitle: format!("Line {} · next departures", cfg.line), rows, foot, empty }
}

/// A time in the Windows time format (or the shared style's), e.g. "7:45 PM" / "19:45".
pub(crate) fn clock_text(t: i64, seconds: bool, look: &Look) -> String {
    let p: Vec<u32> = data::clock(t, "%H %M %S").split(' ').filter_map(|x| x.parse().ok()).collect();
    let (h, m, s) = (p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0), p.get(2).copied().unwrap_or(0));
    if seconds { look.time_with_seconds(h, m, s) } else { look.time(h, m) }
}

// panel fonts
const F_POP: usize = 0; // departure rows: destination
const F_POPT: usize = 1; // title
const F_FOOT: usize = 2; // footer
const F_CAP: usize = 3; // clock time, status, subtitle
const F_MIN: usize = 4; // minutes to go
const F_BADGE: usize = 5; // line number in the header badge

/// The panel's fonts at a display scale (opaque panel, so ClearType); deleted on drop.
struct Fonts([HFONT; 6]);

impl Fonts {
    fn new(look: &Look, scale: f32) -> Fonts {
        let face = wide(&look.font);
        let font = |t: theme::TextStyle| unsafe { CreateFontW(-t.px_at(scale), 0, 0, 0, t.weight, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr()) };
        Fonts([font(theme::BODY), font(theme::TITLE), font(theme::FOOTNOTE), font(theme::CAPTION), font(theme::TITLE_SMALL), font(theme::BADGE)])
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for f in self.0 {
            unsafe { DeleteObject(f) };
        }
    }
}

/// The panel's size at `scale`, px.
pub(crate) fn panel_size(look: &Look, scale: f32) -> (i32, i32) {
    unsafe {
        let dc = CreateCompatibleDC(0);
        let wh = popup_draw(dc, look, scale, None);
        DeleteDC(dc);
        wh
    }
}

/// Draws the panel into `hdc` (its background already filled), `w` wide.
pub(crate) fn panel_draw(hdc: HDC, look: &Look, scale: f32, w: i32) {
    unsafe { popup_draw(hdc, look, scale, Some(w)) };
}

/// Lay out (and with `draw_w`, paint at that width) the panel; returns its size. Same look as the
/// other widgets' panels: a header (line badge, stop, subtitle), a separator, one row per
/// departure (minutes to go | destination over clock time | status, with a dot when live or
/// cancelled), a separator, the footer.
unsafe fn popup_draw(hdc: HDC, lk: &Look, s: f32, draw_w: Option<i32>) -> (i32, i32) {
    let fonts = Fonts::new(lk, s);
    let fonts = &fonts.0;
    let px = |v: f32| (v * s).round() as i32;
    let c = popup_content(lk);
    let pad = px(space::PANEL_PADDING);
    let line_px = px(1.0).max(1);

    // header: the line badge, the stop beside it over the subtitle
    let line = current().0.line;
    let (lw, _) = text_size(hdc, fonts[F_BADGE], &line);
    let badge_h = px(32.0);
    let badge_w = badge_h.max(lw + 2 * px(space::S));
    let (title_w, title_h) = text_size(hdc, fonts[F_POPT], &c.title);
    let (sub_w, sub_h) = text_size(hdc, fonts[F_CAP], &c.subtitle);
    let head_h = badge_h.max(title_h + sub_h);
    let head_tx = pad + badge_w + px(space::M);

    // rows
    let row_h = px(space::ROW_HEIGHT_LARGE);
    let dot = "●";
    let (dot_w, _) = text_size(hdc, fonts[F_CAP], dot);
    let (mut min_w, mut mid_w, mut st_w) = (0, 0, 0);
    for r in &c.rows {
        min_w = min_w.max(text_size(hdc, fonts[F_MIN], &r.mins).0);
        mid_w = mid_w.max(text_size(hdc, fonts[F_POP], &r.dest).0).max(text_size(hdc, fonts[F_CAP], &r.clock).0);
        st_w = st_w.max(text_size(hdc, fonts[F_CAP], &r.status).0);
    }
    let (dest_h, cap_h) = (text_size(hdc, fonts[F_POP], "Ag").1, text_size(hdc, fonts[F_CAP], "Ag").1);
    let mid_x = pad + min_w + px(space::M);
    let st_full = dot_w + px(space::XS) + st_w;
    let rows_w = mid_x + mid_w + px(space::L) + st_full + pad;

    let sep1 = pad + head_h + px(space::S);
    let y0 = sep1 + line_px + px(space::XS);
    let n = c.rows.len().max(1) as i32;
    let sep2 = y0 + row_h * n + px(space::XS);
    let foot_y = sep2 + line_px + px(space::S);
    let (foot_w, foot_h) = text_size(hdc, fonts[F_FOOT], &c.foot);
    let empty_w = if c.rows.is_empty() { text_size(hdc, fonts[F_POP], c.empty).0 } else { 0 };
    let w = px(size::PANEL_NARROW)
        .max(rows_w)
        .max(head_tx + title_w.max(sub_w) + pad)
        .max(2 * pad + foot_w)
        .max(2 * pad + empty_w);
    let h = foot_y + foot_h + pad;
    let Some(w) = draw_w else { return (w, h) };

    let t = Theme::new(lk);
    let (fg, dim, line_c) = (t.color.fg.colorref(), t.color.dim.colorref(), t.color.line.colorref());
    SetBkMode(hdc, TRANSPARENT as _);

    // the line badge in the line's colours (corners as the strip's badge)
    let (badge_bg, badge_fg) = badge_colors();
    let by = pad + (head_h - badge_h) / 2;
    let radius = draw::highlight_shape(lk, s).0 as i32 * 2;
    let brush = CreateSolidBrush(badge_bg);
    let pen = CreatePen(PS_SOLID, 1, badge_bg);
    let (ob, op) = (SelectObject(hdc, brush), SelectObject(hdc, pen));
    RoundRect(hdc, pad, by, pad + badge_w, by + badge_h, radius, radius);
    SelectObject(hdc, ob);
    SelectObject(hdc, op);
    DeleteObject(brush);
    DeleteObject(pen);
    let lh = text_size(hdc, fonts[F_BADGE], &line).1;
    SetTextColor(hdc, badge_fg);
    draw_text(hdc, fonts[F_BADGE], pad + (badge_w - lw) / 2, by + (badge_h - lh) / 2, &line);

    // stop over subtitle, centred on the badge
    let ty = pad + (head_h - title_h - sub_h) / 2;
    SetTextColor(hdc, fg);
    draw_text(hdc, fonts[F_POPT], head_tx, ty, &c.title);
    SetTextColor(hdc, dim);
    draw_text(hdc, fonts[F_CAP], head_tx, ty + title_h, &c.subtitle);
    fill(hdc, &RECT { left: pad, top: sep1, right: w - pad, bottom: sep1 + line_px }, line_c);

    if c.rows.is_empty() {
        SetTextColor(hdc, dim);
        draw_text(hdc, fonts[F_POP], pad, y0 + (row_h - dest_h) / 2, c.empty);
    }
    for (i, r) in c.rows.iter().enumerate() {
        let y = y0 + i as i32 * row_h;
        let text = if r.cancelled { dim } else { fg };
        // minutes to go, centred in the row
        let (_, mh) = text_size(hdc, fonts[F_MIN], &r.mins);
        SetTextColor(hdc, text);
        draw_text(hdc, fonts[F_MIN], pad, y + (row_h - mh) / 2, &r.mins);
        // destination over the clock time
        let ty = y + (row_h - dest_h - cap_h) / 2;
        draw_text(hdc, fonts[F_POP], mid_x, ty, &r.dest);
        SetTextColor(hdc, dim);
        draw_text(hdc, fonts[F_CAP], mid_x, ty + dest_h, &r.clock);
        // status in its colour, right-aligned; live and cancelled with a dot before it
        let sw = text_size(hdc, fonts[F_CAP], &r.status).0;
        let sx = w - pad - sw;
        let sy = y + (row_h - cap_h) / 2;
        SetTextColor(hdc, r.color);
        if !r.scheduled {
            draw_text(hdc, fonts[F_CAP], sx - px(space::XS) - dot_w, sy, dot);
        }
        draw_text(hdc, fonts[F_CAP], sx, sy, &r.status);
    }

    fill(hdc, &RECT { left: pad, top: sep2, right: w - pad, bottom: sep2 + line_px }, line_c);
    SetTextColor(hdc, dim);
    draw_text(hdc, fonts[F_FOOT], pad, foot_y, &c.foot);
    (w, h)
}
