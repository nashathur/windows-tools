//! The strip's text and drawing (into Block's canvas), and the tray logo.

use super::*;

pub(crate) fn fmt_min(u: &data::Bus, now: i64) -> String {
    let m = (u.t - now).div_euclid(60);
    let s = if m < 1 { "now".to_string() } else { format!("{m}′") };
    if u.live { s } else { format!("~{s}") }
}

/// The strip's text in full or compact mode.
pub(crate) fn bar_text_for(compact: bool) -> String {
    let (_, sched, live, ok_at, err) = current();
    let err = err.is_some();
    if sched.is_none() && live.is_empty() {
        let no_live = shared().lock().unwrap().no_live;
        let msg = if compact { "…" } else if no_live { "waiting for timetable" } else { "loading…" };
        return if err { format!("{msg}  ⚠") } else { msg.into() };
    }
    let now = data::now();
    let mut t = data::upcoming(sched.as_deref(), &live, now)
        .iter()
        .filter(|u| !u.cancel)
        .take(if compact { 2 } else { SHOW_IN_BAR })
        .map(|u| fmt_min(u, now))
        .collect::<Vec<_>>()
        .join(if compact { "\n" } else { "  " });
    if t.is_empty() {
        t = if compact { "–" } else { "no buses" }.into();
    }
    if err && ok_at.map_or(true, |a| now - a > 3 * POLL_S) {
        t.push_str("  ⚠");
    }
    t
}

// ---- GDI helpers ------------------------------------------------------------

pub(crate) unsafe fn text_size(hdc: HDC, font: HFONT, s: &str) -> (i32, i32) {
    SelectObject(hdc, font);
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut sz = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(hdc, w.as_ptr(), w.len() as i32, &mut sz);
    (sz.cx, sz.cy)
}

pub(crate) unsafe fn draw_text(hdc: HDC, font: HFONT, x: i32, y: i32, s: &str) {
    SelectObject(hdc, font);
    let w: Vec<u16> = s.encode_utf16().collect();
    TextOutW(hdc, x, y, w.as_ptr(), w.len() as i32);
}

pub(crate) unsafe fn fill(hdc: HDC, rc: &RECT, color: u32) {
    let br = CreateSolidBrush(color);
    FillRect(hdc, rc, br);
    DeleteObject(br);
}

// ---- the strip: badge, then the label over the departures ------------------------------

/// The strip laid out for a frame: its fonts (deleted on drop), the two lines and where they go.
struct Layout {
    fonts: [HFONT; 3], // badge, label, departures
    label: String,
    text: String,
    badge: badge::Badge,
    pad: i32,
    tx: i32,
    tops: (i32, i32),
    width: i32,
}

impl Drop for Layout {
    fn drop(&mut self) {
        for f in self.fonts {
            unsafe { DeleteObject(f) };
        }
    }
}

/// Lays out full or `compact` mode at the frame's scale (display scale × drag zoom); the strip
/// is as tall as the taskbar. `dc`: any memory DC.
unsafe fn layout(dc: HDC, f: &Frame, compact: bool) -> Layout {
    let lk = f.look;
    let s = f.scale;
    // the shared style: the clock's font and size, regular weight; the line number in the shared badge
    let font = |size: f32, weight: i32| {
        let face = wide(&lk.font);
        CreateFontW(-f.px(size), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 4, 0, face.as_ptr())
    };
    let fonts = [badge::font(lk, s), font(lk.font_size, 400), font(lk.font_size, 400)];
    let cfg = current().0;
    // compact: the next two departures on two lines beside the badge, no label
    let all = bar_text_for(compact);
    let (label, text) = match all.split_once('\n') {
        Some((a, b)) if compact => (a.to_string(), b.to_string()),
        _ => (if compact { String::new() } else { cfg.label.clone() }, all.clone()),
    };
    let (sw, _) = if label.is_empty() { (0, 0) } else { text_size(dc, fonts[1], &label) };
    let (tw, th) = text_size(dc, fonts[2], &text);
    let (pad, gap) = (f.px(8.0), f.px(6.0));
    // the two lines sit on the taskbar clock's baselines
    let ascent = |font: HFONT| {
        SelectObject(dc, font);
        let mut tm: TEXTMETRICW = zeroed();
        GetTextMetricsW(dc, &mut tm);
        tm.tmAscent
    };
    let tops = if label.is_empty() {
        (0, (f.height - th + 1) / 2)
    } else {
        draw::two_line_tops(f.height, s, ascent(fonts[1]), ascent(fonts[2]))
    };
    // the line number: the shared badge (lines up with the tray icons, as wide as the number
    // needs), 6 px from the text
    let badge = badge::Badge::measure(dc, fonts[0], &cfg.line, f.height, s);
    let tx = pad + badge.width + gap;
    let width = tx + sw.max(tw) + pad;
    Layout { fonts, label, text, badge, pad, tx, tops, width }
}

pub(crate) fn measure(f: &Frame, compact: bool) -> i32 {
    unsafe {
        let dc = CreateCompatibleDC(0);
        let w = layout(dc, f, compact).width;
        DeleteDC(dc);
        w
    }
}

/// White text gives the coverage, turned into the text colours (label dimmer); then the badge.
pub(crate) fn draw(cv: &mut Canvas, f: &Frame) {
    unsafe {
        let dc = cv.dc;
        let l = layout(dc, f, f.compact);
        SetTextColor(dc, WHITE);
        if !l.label.is_empty() {
            draw_text(dc, l.fonts[1], l.tx, l.tops.0, &l.label);
        }
        draw_text(dc, l.fonts[2], l.tx, l.tops.1, &l.text);
        let (w, h, top2, compact) = (cv.w, cv.h, l.tops.1, f.compact);
        let (fg, dim) = f.look.text_rgb();
        draw::text_to_color(cv.pixels(), w, h, |y| if y < top2 && !compact { dim } else { fg });
        let (badge_bg, badge_fg) = badge_colors();
        l.badge.paint(dc, cv.pixels(), w, l.pad, l.fonts[0], badge_bg, Some(badge_fg), f.look, f.scale);
    }
}

/// The tray logo: the line badge filling the icon (the number made smaller if it doesn't fit).
pub(crate) fn tray_logo(cv: &mut Canvas, look: &Look) -> bool {
    unsafe {
        let (dc, w, h) = (cv.dc, cv.w, cv.h);
        let (bg, fg) = badge_colors();
        let line = current().0.line;
        let scale = h as f32 / theme::size::ICON_BOX;
        let face = wide(&look.font);
        let mut px = badge::font_px(look, scale);
        let font = loop {
            let font = CreateFontW(-px, 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 4, 0, face.as_ptr());
            if px <= 6 || text_size(dc, font, &line).0 <= w - 2 {
                break font;
            }
            DeleteObject(font);
            px -= 1;
        };
        fill(dc, &RECT { left: 0, top: 0, right: w, bottom: h }, bg);
        let (tw, th) = text_size(dc, font, &line);
        SetTextColor(dc, fg);
        draw_text(dc, font, (w - tw) / 2, (h - th) / 2, &line);
        DeleteObject(font);
        draw::round_rect(cv.pixels(), w, (0, 0, w, h), (look.control_radius * scale).round());
        true
    }
}
