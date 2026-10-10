//! The departures popup: content, drawing, open/pin/hover/fade, and its window procedure.

use super::*;
use taskbar_dock::theme::{size, space};

/// One departure: minutes to go, destination, clock time, and its status in its colour.
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
pub(crate) struct PopContent {
    title: String,
    subtitle: String,
    rows: Vec<PopRow>,
    foot: String,
    empty: &'static str,
}

pub(crate) fn popup_content() -> PopContent {
    let (cfg, sched, live, ok_at, err) = current();
    let t = Theme::new(&look());
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
        rows.push(PopRow { mins: m, dest: dest.to_string(), clock: clock_text(u.t, false), status, color: color.colorref(), cancelled: u.cancel, scheduled: !u.cancel && !u.live });
    }
    let no_live = shared().lock().unwrap().no_live;
    let mut foot = match ok_at {
        _ if no_live => "no live data for this stop, timetable times only".to_string(),
        Some(t) => format!("live data {}", clock_text(t, true)),
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

/// Lay out (and with `draw`, paint) the popup; returns its size. Same look as the other
/// widgets' panels: a header (line badge, stop, subtitle), a separator, one row per departure
/// (minutes to go | destination over clock time | status, with a dot when live or cancelled),
/// a separator, the footer.
pub(crate) unsafe fn popup_draw(hdc: HDC, draw: bool) -> (i32, i32) {
    let (fonts, s) = ui(|u| (u.fonts.get(), u.scale.get()));
    let px = |v: f32| (v * s).round() as i32;
    let c = popup_content();
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
    if !draw {
        return (w, h);
    }

    let lk = look();
    let t = Theme::new(&lk);
    let (fg, dim, line_c) = (t.color.fg.colorref(), t.color.dim.colorref(), t.color.line.colorref());
    fill(hdc, &RECT { left: 0, top: 0, right: w, bottom: h }, t.color.bg.colorref());
    SetBkMode(hdc, TRANSPARENT as _);

    // the line badge in the line's colours (corners as the strip's badge)
    let (badge_bg, badge_fg) = badge_colors();
    let by = pad + (head_h - badge_h) / 2;
    let radius = draw::highlight_shape(&lk, s).0 as i32 * 2;
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

pub(crate) unsafe fn position_popup(show: bool) {
    let (p, bar) = ui(|u| (u.popup.get(), u.bar.get()));
    let screen = GetDC(0);
    let (w, h) = popup_draw(screen, false);
    ReleaseDC(0, screen);
    // centred on the strip, kept on its monitor (shared rule)
    let (x, y) = taskbar_dock::panel::place(bar, w, h, &look(), ui(|u| u.scale.get()));
    SetWindowPos(p, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | if show { SWP_SHOWWINDOW } else { 0 });
    InvalidateRect(p, null(), 0);
}

/// Left click on the strip (not a drag): open, pin or close the popup.
pub(crate) unsafe fn toggle_popup() {
    panel_do(ui(|u| u.panel.borrow_mut().click(Instant::now())));
}
/// A time in the Windows time format (or the shared style's), e.g. "7:45 PM" / "19:45".
pub(crate) fn clock_text(t: i64, seconds: bool) -> String {
    let p: Vec<u32> = data::clock(t, "%H %M %S").split(' ').filter_map(|x| x.parse().ok()).collect();
    let (h, m, s) = (p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0), p.get(2).copied().unwrap_or(0));
    let lk = look();
    if seconds { lk.time_with_seconds(h, m, s) } else { lk.time(h, m) }
}

/// Does what the shared panel behaviour (taskbar_dock::panel) decided.
pub(crate) unsafe fn panel_do(a: Action) {
    match a {
        Action::OpenByHover => show_popup(false),
        Action::OpenByClick => show_popup(true),
        Action::Pin => {
            SetForegroundWindow(ui(|u| u.popup.get())); // a click elsewhere now closes it
        }
        Action::Close => hide_popup(),
        Action::None => {}
    }
    let (p, ticks) = ui(|u| (u.popup.get(), u.panel.borrow().wants_ticks()));
    if !ticks {
        KillTimer(p, TIMER_HOVER);
    }
}

/// Shows the popup, fading in. `activate`: opened by a click (takes focus; a click elsewhere
/// closes it). Opened by hovering it doesn't take focus, so typing elsewhere isn't interrupted.
pub(crate) unsafe fn show_popup(activate: bool) {
    let (p, tray, bar) = ui(|u| (u.popup.get(), u.tray.get(), u.bar.get()));
    // only one panel at a time: clicking a widget doesn't activate anything, so the other
    // widgets' panels wouldn't notice the click by themselves
    dock::close_other_panels(tray, bar);
    SetLayeredWindowAttributes(p, 0, 0, LWA_ALPHA); // start transparent; fade_step brings it in
    ui(|u| u.fade.borrow_mut().start(Instant::now()));
    position_popup(true);
    if activate {
        SetForegroundWindow(p);
    }
    SetTimer(p, TIMER_FADE, 10, None);
}

/// Hides the popup. Callers tell the panel behaviour why (or it decided it).
pub(crate) unsafe fn hide_popup() {
    let p = ui(|u| u.popup.get());
    ui(|u| u.fade.borrow_mut().stop());
    ShowWindow(p, SW_HIDE);
}

/// The popup closed for a reason of its own (lost focus, clicked, another widget's panel).
pub(crate) unsafe fn popup_closed(lost_focus: bool) {
    ui(|u| u.panel.borrow_mut().closed(Instant::now(), lost_focus));
    hide_popup();
    panel_do(Action::None);
}

/// One frame of the popup's fade-in.
pub(crate) unsafe fn fade_step() {
    let p = ui(|u| u.popup.get());
    let lk = look();
    match ui(|u| u.fade.borrow_mut().alpha(Instant::now(), &lk)) {
        Some(a) => {
            SetLayeredWindowAttributes(p, 0, a, LWA_ALPHA);
        }
        None => {
            KillTimer(p, TIMER_FADE);
        }
    }
}

/// The mouse came onto the strip: the popup opens if it stays (see hover_step).
pub(crate) unsafe fn hover_started() {
    let watch = ui(|u| {
        let mut pb = u.panel.borrow_mut();
        pb.mouse_entered(Instant::now());
        pb.wants_ticks()
    });
    if watch {
        SetTimer(ui(|u| u.popup.get()), TIMER_HOVER, 50, None);
    }
}

/// Every 50 ms while hovering or while the popup is hover-opened.
pub(crate) unsafe fn hover_step() {
    let (p, bar) = ui(|u| (u.popup.get(), u.bar.get()));
    let pt = dock::cursor();
    let inside = |h: HWND| {
        let mut rc: RECT = zeroed();
        GetWindowRect(h, &mut rc);
        pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
    };
    let (on_bar, on_panel) = (inside(bar), IsWindowVisible(p) != 0 && inside(p));
    let lk = look();
    panel_do(ui(|u| u.panel.borrow_mut().tick(Instant::now(), on_bar, on_panel, &lk)));
}
pub(crate) unsafe extern "system" fn popup_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let hdc = BeginPaint(h, &mut ps);
            let mut rc: RECT = zeroed();
            GetClientRect(h, &mut rc);
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp);
            fill(mem, &rc, Theme::new(&look()).color.bg.colorref());
            popup_draw(mem, true);
            BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            EndPaint(h, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETTINGCHANGE | WM_DISPLAYCHANGE | WM_DPICHANGED => {
            // theme, scaling or another Windows setting changed: redraw right away
            if refresh_look() {
                ensure_popup_fonts();
                ui(|u| u.shown.borrow_mut().clear());
                render_bar(&bar_text());
                if IsWindowVisible(h) != 0 {
                    position_popup(false);
                }
            }
            DefWindowProcW(h, m, w, l)
        }
        WM_TIMER => {
            match w {
                TIMER_GLIDE => glide_step(),
                TIMER_ZOOM => zoom_step(),
                TIMER_HOVER => hover_step(),
                TIMER_FADE => fade_step(),
                TIMER_RESIZE => resize_step(),
                _ => on_tick(),
            }
            0
        }
        WM_ACTIVATE => {
            if (w & 0xffff) as u32 == WA_INACTIVE && IsWindowVisible(h) != 0 {
                popup_closed(true);
            }
            0
        }
        WM_LBUTTONUP => {
            popup_closed(false);
            0
        }
        // right click on the departures: switch to the right-click panel
        WM_RBUTTONUP => {
            context_menu();
            0
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}
