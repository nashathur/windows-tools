//! The departures popup: content, drawing, open/pin/hover/fade, and its window procedure.

use super::*;

pub(crate) struct PopRow {
    cells: [String; 4],
    color: u32,
}

/// Title, departure rows, footer, and the line shown when there are no rows.
pub(crate) fn popup_content() -> (String, Vec<PopRow>, String, &'static str) {
    let (cfg, sched, live, ok_at, err) = current();
    let t = Theme::new(&look());
    let empty = if sched.is_none() && live.is_empty() {
        "Waiting for the timetable download"
    } else {
        "No departures in the next 6 hours"
    };
    let now = data::now();
    let mut rows = Vec::new();
    {
        for u in data::upcoming(sched.as_deref(), &live, now).into_iter().take(SHOW_IN_POPUP) {
            let delay = ((u.t - u.plan) as f64 / 60.0).round() as i64;
            let (status, color) = if u.cancel {
                ("cancelled".to_string(), t.status.danger.colorref())
            } else if u.live {
                let s = match delay {
                    0 => "live on time".to_string(),
                    d if d > 0 => format!("live +{d}"),
                    d => format!("live {d}"),
                };
                (s, t.status.ok.colorref())
            } else {
                ("scheduled".to_string(), t.color.dim.colorref())
            };
            let mins = (u.t - now).div_euclid(60);
            let m = if mins < 1 { "now".to_string() } else { format!("{mins} min") };
            let dest = if cfg.dest.is_empty() { u.head.as_str() } else { cfg.dest.as_str() };
            rows.push(PopRow { cells: [clock_text(u.t, false), format!("→ {dest}"), m, status], color });
        }
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
    (format!("Line {} · {}", cfg.line, cfg.label), rows, foot, empty)
}

/// Lay out (and with `draw`, paint) the popup; returns its size.
pub(crate) unsafe fn popup_draw(hdc: HDC, draw: bool) -> (i32, i32) {
    let (fonts, s) = ui(|u| (u.fonts.get(), u.scale.get()));
    let px = |v: f32| (v * s).round() as i32;
    let (title, rows, foot, empty) = popup_content();
    let (pad, colgap) = (px(12.0), px(14.0));
    let (title_w, title_h) = text_size(hdc, fonts[F_POPT], &title);
    let row_h = text_size(hdc, fonts[F_POP], "Ag").1 + px(4.0);
    let mut colw = [0i32; 4];
    for r in &rows {
        for (k, c) in r.cells.iter().enumerate() {
            colw[k] = colw[k].max(text_size(hdc, fonts[F_POP], c).0);
        }
    }
    let mut colx = [pad; 4];
    for k in 1..4 {
        colx[k] = colx[k - 1] + colw[k - 1] + colgap;
    }
    let y0 = pad + title_h + px(6.0);
    let foot_y = y0 + row_h * rows.len().max(1) as i32 + px(6.0);
    let (foot_w, foot_h) = text_size(hdc, fonts[F_FOOT], &foot);
    let empty_w = if rows.is_empty() { text_size(hdc, fonts[F_POP], empty).0 } else { 0 };
    let w = (colx[3] + colw[3] + pad).max(2 * pad + foot_w).max(2 * pad + title_w).max(2 * pad + empty_w);
    let h = foot_y + foot_h + pad;
    if draw {
        let c = Theme::new(&look()).color;
        let (fg, dim) = (c.fg.colorref(), c.dim.colorref());
        fill(hdc, &RECT { left: 0, top: 0, right: w, bottom: h }, c.bg.colorref());
        SetBkMode(hdc, TRANSPARENT as _);
        SetTextColor(hdc, fg);
        draw_text(hdc, fonts[F_POPT], pad, pad, &title);
        if rows.is_empty() {
            draw_text(hdc, fonts[F_POP], pad, y0, empty);
        }
        for (i, r) in rows.iter().enumerate() {
            let y = y0 + i as i32 * row_h;
            for k in 0..4 {
                SetTextColor(hdc, if k == 3 { r.color } else { fg });
                draw_text(hdc, fonts[F_POP], colx[k], y, &r.cells[k]);
            }
        }
        SetTextColor(hdc, dim);
        draw_text(hdc, fonts[F_FOOT], pad, foot_y, &foot);
    }
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
