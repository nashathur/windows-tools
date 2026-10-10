//! The strip's text and drawing, the shared look, and dock geometry helpers.

use super::*;

pub(crate) fn fmt_min(u: &data::Bus, now: i64) -> String {
    let m = (u.t - now).div_euclid(60);
    let s = if m < 1 { "now".to_string() } else { format!("{m}′") };
    if u.live { s } else { format!("~{s}") }
}

pub(crate) fn bar_text() -> String {
    bar_text_for(compact_now())
}

/// Drawn in compact mode: chosen in the settings, or no room for the full strip next to the
/// taskbar's buttons (dock::fit).
pub(crate) fn compact_now() -> bool {
    current().0.compact || ui(|u| u.fit.borrow().compact())
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

pub(crate) unsafe fn fullscreen_app_active() -> bool {
    let h = GetForegroundWindow();
    if h == 0 {
        return false;
    }
    let mut cls = [0u16; 64];
    let n = GetClassNameW(h, cls.as_mut_ptr(), cls.len() as i32);
    let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
    if ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"].contains(&cls.as_str()) {
        return false;
    }
    let mut r: RECT = zeroed();
    GetWindowRect(h, &mut r);
    r.left <= 0 && r.top <= 0 && r.right >= GetSystemMetrics(SM_CXSCREEN) && r.bottom >= GetSystemMetrics(SM_CYSCREEN)
}

pub(crate) fn look() -> Look {
    ui(|u| u.look.borrow().clone())
}

/// Re-reads the shared style and the Windows settings (theme, animation effects, hover time,
/// time format, text size) and the display scale. Returns true if anything changed, so the
/// strip and popup can be redrawn: everything follows Windows live, without a restart.
pub(crate) unsafe fn refresh_look() -> bool {
    let new = Look::current();
    let anchor = ui(|u| if u.tray.get() != 0 { u.tray.get() } else { u.bar.get() });
    let scale = system::scale_for(anchor);
    ui(|u| {
        let changed = *u.look.borrow() != new || u.scale.get() != scale;
        *u.look.borrow_mut() = new;
        u.scale.set(scale);
        changed
    })
}

// ---- taskbar strip: per-pixel alpha, so it blends with any taskbar and stays clickable ----

/// Draw the strip; false if Windows refused (e.g. a transparent child window isn't supported).
/// While compact mode is switching, the full strip is drawn narrowed to the animated width
/// (see `start_resize`). A new width keeps the right edge: the stack hangs from the tray.
pub(crate) unsafe fn render_bar(text: &str) -> bool {
    let compact = compact_now();
    let before = bar_rect();
    let ok = match start_resize(compact) {
        Some(w) => draw_bar(&bar_text_for(false), false, Some(w), false),
        None => draw_bar(text, compact, None, false),
    }
    .is_some();
    let (h, embedded, tray) = ui(|u| (u.bar.get(), u.embedded.get(), u.tray.get()));
    if ok && embedded && IsWindowVisible(h) != 0 && ui(|u| u.motion.borrow().settled()) {
        strip::keep_right_edge(h, tray, before);
    }
    if ok && embedded && ui(|u| u.motion.borrow().zoom()) == 1.0 {
        // the other members fit the stack with these (dock::fit)
        if let Some(size) = bar_size() {
            dock::publish_widths(h, size);
        }
    }
    ok
}

/// The strip's widths for the fit: in the chosen mode, and compact.
pub(crate) unsafe fn bar_size() -> Option<dock::Size> {
    Some(dock::Size { full: bar_width(current().0.compact)?, min: bar_width(true)?, stretch: false })
}

/// The strip's width in a mode (nothing drawn).
pub(crate) unsafe fn bar_width(compact: bool) -> Option<i32> {
    draw_bar(&bar_text_for(compact), compact, None, true)
}

/// Draws `text` in full or compact mode, cut off at `clip` px if narrower (the right end fades
/// out), or only measures when `measure`. Returns the width, None if Windows refused.
unsafe fn draw_bar(text: &str, compact: bool, clip: Option<i32>, measure: bool) -> Option<i32> {
    let (h, scale, zoom) = ui(|u| (u.bar.get(), u.scale.get(), u.motion.borrow().zoom()));
    let lk = look();
    // content scale: the zoom grows it while dragged; the strip stays as tall as the taskbar
    let s = scale * zoom;
    let px = |v: f32| (v * s).round() as i32;
    // the shared style: the clock's font and size, regular weight; the line number in the shared badge
    let font = |size: f32, weight: i32| {
        let face = wide(&lk.font);
        CreateFontW(-px(size), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 4, 0, face.as_ptr())
    };
    let (f_badge, f_small, f_big) = (badge::font(&lk, s), font(lk.font_size, 400), font(lk.font_size, 400));
    let screen = GetDC(0);
    let mem = CreateCompatibleDC(screen);
    let cfg = current().0;
    // compact: the next two departures on two lines beside the badge, no label
    let (label, text) = match text.split_once('\n') {
        Some((a, b)) if compact => (a, b),
        _ => (if compact { "" } else { cfg.label.as_str() }, text),
    };
    let line = cfg.line.as_str();
    let (sw, sh) = if label.is_empty() { (0, 0) } else { text_size(mem, f_small, label) };
    let (tw, th) = text_size(mem, f_big, text);
    let (pad, gap) = (px(8.0), px(6.0));
    let content_h = sh + th;
    // attached: as tall as the taskbar (like its buttons and the weather widget)
    let (embedded, tray) = ui(|u| (u.embedded.get(), u.tray.get()));
    let mut tb: RECT = zeroed();
    let height = if embedded && GetClientRect(tray, &mut tb) != 0 && tb.bottom > content_h {
        tb.bottom
    } else {
        content_h
    };
    // the two lines sit on the taskbar clock's baselines
    let ascent = |f: HFONT| {
        SelectObject(mem, f);
        let mut tm: TEXTMETRICW = zeroed();
        GetTextMetricsW(mem, &mut tm);
        tm.tmAscent
    };
    let (top1, top2) = if label.is_empty() {
        (0, (height - th + 1) / 2)
    } else {
        draw::two_line_tops(height, s, ascent(f_small), ascent(f_big))
    };
    // the line number: the shared badge (lines up with the tray icons, as wide as the number
    // needs), 6 px from the text
    let line_badge = badge::Badge::measure(mem, f_badge, line, height, s);
    let tx = pad + line_badge.width + gap;
    let natural = tx + sw.max(tw) + pad;
    if measure {
        DeleteDC(mem);
        ReleaseDC(0, screen);
        for f in [f_badge, f_small, f_big] {
            DeleteObject(f);
        }
        return Some(natural);
    }
    let width = clip.map_or(natural, |c| c.clamp(1, natural));

    let mut bmi: BITMAPINFO = zeroed();
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = width;
    bmi.bmiHeader.biHeight = -height; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB as _;
    let mut bits: *mut c_void = null_mut();
    let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, 0, 0);
    if dib == 0 || bits.is_null() {
        DeleteDC(mem);
        ReleaseDC(0, screen);
        for f in [f_badge, f_small, f_big] {
            DeleteObject(f);
        }
        return None;
    }
    let old = SelectObject(mem, dib);
    let buf = std::slice::from_raw_parts_mut(bits as *mut u32, (width * height) as usize);

    // 1) white text on black gives the coverage mask; 2) into colour (first line dimmer)
    buf.fill(0);
    SetBkMode(mem, TRANSPARENT as _);
    SetTextColor(mem, WHITE);
    if !label.is_empty() {
        draw_text(mem, f_small, tx, top1, label);
    }
    draw_text(mem, f_big, tx, top2, text);
    GdiFlush();
    if width < natural {
        // cut off while collapsing or expanding: the text fades out at the right end
        taskbar_dock::marquee::fade_edges(buf, width, (tx, 0, width, height), px(12.0), false, true, 0);
    }
    let (fg, dim) = lk.text_rgb();
    draw::text_to_color(buf, width, height, |y| if y < top2 && !compact { dim } else { fg });
    // 3) the line badge in the line's colours
    let (badge_bg, badge_fg) = badge_colors();
    line_badge.paint(mem, buf, width, pad, f_badge, badge_bg, Some(badge_fg), &lk, s);
    // 4) background under everything, with the hover highlight
    let (radius, inset) = draw::highlight_shape(&lk, s);
    draw::background(buf, width, height, ui(|u| u.hover.get()), lk.light, radius, inset);

    let size = SIZE { cx: width, cy: height };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let ok = UpdateLayeredWindow(h, screen, null(), &size, mem, &src, 0, &blend, ULW_ALPHA) != 0;
    SelectObject(mem, old);
    DeleteObject(dib);
    DeleteDC(mem);
    ReleaseDC(0, screen);
    for f in [f_badge, f_small, f_big] {
        DeleteObject(f);
    }
    ok.then_some(width)
}

pub(crate) fn saved_x() -> Option<i32> {
    fs::read_to_string(data_dir().join("pos.txt")).ok()?.trim().parse().ok()
}

pub(crate) unsafe fn bar_rect() -> RECT {
    dock::rect(ui(|u| u.bar.get()))
}

/// Gap between docked widgets and to the tray (shared style), in pixels.
pub(crate) fn gap() -> i32 {
    (look().gap * ui(|u| u.scale.get())).round() as i32
}

/// The tray boundary (screen x), when attached to the taskbar. See dock::tray_boundary.
pub(crate) unsafe fn boundary() -> Option<i32> {
    let (embedded, tray, scale) = ui(|u| (u.embedded.get(), u.tray.get(), u.scale.get()));
    embedded.then(|| dock::tray_boundary(tray, scale))
}

/// Where the strip belongs in the dock stack, whether to slide there, and how it fits next to
/// the taskbar's buttons (see dock::slot). None when floating.
pub(crate) unsafe fn dock_slot() -> Option<dock::Slot> {
    let (bar, tray) = ui(|u| (u.bar.get(), u.tray.get()));
    let b = boundary()?;
    dock::slot(tray, bar, bar_size()?, b, gap())
}
