//! Taskbar bus-arrival widget for one stop and line (default: U-OV line 3, Rhônedreef → Utrecht CS).
//!
//! Hover or left-click: next departures · drag: reorder in the dock · right-click: menu.
//! Settings: the "bus" section of %APPDATA%\TaskbarDock\settings.json; look, docking and Windows
//! settings come from the shared taskbar-dock library (C:\Users\thurs\tools\taskbar-dock).
//! Data lives in .\data next to the exe (schedule cache, timetable, line colours, log).
#![windows_subsystem = "windows"]

mod data;
mod picker;
mod bar;
mod placement;
mod popup;
mod worker;

use bar::*;
use placement::*;
use popup::*;
use worker::*;

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::{fs, thread, time::Duration};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows_sys::Win32::Graphics::Gdi::*;
use taskbar_dock::panel::{Action, Fade, PanelBehaviour};
use taskbar_dock::style::Look;
use taskbar_dock::theme::{self, Theme};
use std::time::Instant;
use taskbar_dock::{badge, dock, draw, menu_panel, strip, system, wide};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_MOUSELEAVE: u32 = 0x02A3; // not exported where windows-sys 0.52 keeps the other WM_*
const MSG_CAPTURECHANGED: u32 = 0x0215; // WM_CAPTURECHANGED

const POLL_S: i64 = 60; // live update interval (one ~6 KB request)
const LIVE_WINDOW_MIN: i64 = 90; // only fetch live data when a bus is due within this many minutes
const LIVE_MAX_AGE_S: i64 = 600; // older live data is ignored (its expected times would be stale)
const SHOW_IN_BAR: usize = 3;
const SHOW_IN_POPUP: usize = 8;

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
/// Turquoise: a line with no colour in settings, table or timetable (COLORREF).
fn badge_fallback() -> u32 {
    theme::brand("bus.badge_fallback", true).colorref()
}
/// White coverage: strip text is drawn white, then coloured.
const WHITE: u32 = rgb(255, 255, 255);

fn hex_color(s: &str) -> Option<u32> {
    let s = s.trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).ok().filter(|_| s.len() == 6)?;
    Some(rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Line badge (background, text): badge_color/badge_text from the "bus" section of the shared
/// settings.json, else data\line_colors.txt, else the operator's line colour from GTFS, else
/// turquoise. Edits to line_colors.txt apply live (checked each time, reloaded when it changed).
fn badge_colors() -> (u32, u32) {
    thread_local!(static TABLE: RefCell<(Option<std::time::SystemTime>, Vec<data::LineColor>)> = RefCell::new((None, Vec::new())));
    let path = data_dir().join("line_colors.txt");
    let mtime = fs::metadata(&path).and_then(|m| m.modified()).ok();
    let table = TABLE.with(|t| {
        let mut t = t.borrow_mut();
        if t.0 != mtime || (mtime.is_some() && t.1.is_empty()) {
            *t = (mtime, data::load_line_colors(data_dir()));
        }
        t.1.clone()
    });
    let table = &table;
    let (cfg, sched, ..) = current();
    let (bg, fg) = if let Some(bg) = hex_color(&cfg.badge_color) {
        (Some(bg), hex_color(&cfg.badge_text))
    } else if let Some(c) = data::find_line_color(table, &cfg.agency, &cfg.line).filter(|c| hex_color(&c.bg).is_some()) {
        (hex_color(&c.bg), hex_color(&c.fg))
    } else {
        sched.map_or((None, None), |s| (hex_color(&s.color), hex_color(&s.text_color)))
    };
    let bg = bg.unwrap_or_else(badge_fallback);
    (bg, fg.unwrap_or_else(|| badge::contrast_text(bg)))
}

/// The strip's window class: its name in the dock order.
const MY_CLASS: &str = "BusWidgetBar";

// ---- UI state (UI thread only; Cells so re-entrant window messages are safe) ----

// popup fonts (the strip makes its own, at the current zoom)
const F_POP: usize = 0;
const F_POPT: usize = 1;
const F_FOOT: usize = 2;

struct Ui {
    hinst: Cell<HMODULE>,
    bar: Cell<HWND>,
    embedded: Cell<bool>, // the strip is a child of the taskbar (else a floating topmost window)
    tray: Cell<HWND>,     // the taskbar it is attached to
    hover: Cell<bool>,    // mouse over the strip: draw the taskbar-button highlight
    hooks: RefCell<dock::Hooks>,            // WinEvent hooks on processes owning taskbar windows
    motion: RefCell<strip::Motion>,         // glide, drag and zoom of the strip (shared behaviour)
    settings_mtime: Cell<Option<std::time::SystemTime>>, // to notice edits to settings.json
    look: RefCell<Look>,                    // shared style + Windows settings, kept current
    panel: RefCell<PanelBehaviour>,         // when the popup opens and closes (shared behaviour)
    fade: RefCell<Fade>,                    // popup fade-in
    quitting: Cell<bool>,
    popup: Cell<HWND>,
    scale: Cell<f32>,                       // display scale of the taskbar's monitor, kept current
    fonts: Cell<[HFONT; 3]>,                // popup fonts, remade when scale or font changes
    fonts_for: RefCell<(f32, String, f32)>, // (scale, family, size) the popup fonts were made for
    ticks: Cell<u32>,
    hidden: Cell<bool>,
    shown: RefCell<String>,
    drawn_compact: Cell<Option<bool>>, // compact mode as last drawn: a switch animates the width
    fit: RefCell<dock::FitState>,      // room next to the taskbar's buttons: full, compact or hidden
}

thread_local!(static UI: Ui = Ui {
    hinst: Cell::new(0),
    bar: Cell::new(0),
    embedded: Cell::new(false),
    tray: Cell::new(0),
    hover: Cell::new(false),
    hooks: RefCell::new(dock::Hooks::default()),
    motion: RefCell::new(strip::Motion::default()),
    settings_mtime: Cell::new(None),
    look: RefCell::new(Look::current()),
    panel: RefCell::new(PanelBehaviour::default()),
    fade: RefCell::new(Fade::default()),
    quitting: Cell::new(false),
    popup: Cell::new(0),
    scale: Cell::new(1.0),
    fonts: Cell::new([0; 3]),
    fonts_for: RefCell::new((0.0, String::new(), 0.0)),
    ticks: Cell::new(0),
    hidden: Cell::new(false),
    shown: RefCell::new(String::new()),
    drawn_compact: Cell::new(None),
    fit: RefCell::new(dock::FitState::default()),
});

fn ui<R>(f: impl FnOnce(&Ui) -> R) -> R {
    UI.with(f)
}

// ---- strip window ---------------------------------------------------------

/// The right-click panel (taskbar_dock::menu_panel): actions, switches, settings and quit.
/// Also opened by a right click on the departures popup (it switches to this panel).
unsafe fn context_menu() {
    // a right-click isn't a hover: cancels a pending hover-open, closes a hover-opened popup
    panel_do(ui(|u| u.panel.borrow_mut().right_click(false)));
    if IsWindowVisible(ui(|u| u.popup.get())) != 0 {
        popup_closed(false);
    }
    let (bar, tray, scale) = ui(|u| (u.bar.get(), u.tray.get(), u.scale.get()));
    if !menu_panel::is_open() && tray != 0 {
        dock::close_other_panels(tray, bar); // one panel at a time across the widgets
    }
    let cfg = current().0;
    let menu = menu_panel::Menu {
        title: cfg.label.clone(),
        subtitle: format!("Line {} · {}", cfg.line, bar_text_for(false)),
        items: vec![
            menu_panel::Row::action(4, menu_panel::glyph::PIN, "Change stop…"),
            menu_panel::Row::action(1, menu_panel::glyph::REFRESH, "Refresh live times"),
            menu_panel::Row::action(2, menu_panel::glyph::CALENDAR, "Update timetable"),
            menu_panel::Item::Sep,
            menu_panel::Row::switch(7, "Compact mode", cfg.compact),
            menu_panel::Row::switch(8, "Start with Windows", taskbar_dock::startup::is_on("bus")),
            menu_panel::Item::Sep,
            menu_panel::settings_and_quit(6, 3),
        ],
    };
    // the switches flip and the panel stays open
    let mut flip = |id: usize| match id {
        7 => {
            let mut cfg = current().0;
            cfg.compact = !cfg.compact;
            let on = cfg.compact;
            set_config(cfg); // redraw now and glide back into the dock slot
            on_tick();
            on
        }
        _ => {
            let on = !taskbar_dock::startup::is_on("bus");
            if let Err(e) = taskbar_dock::startup::set("bus", on) {
                data::log(data_dir(), &format!("start with Windows: {e}"));
            }
            taskbar_dock::startup::is_on("bus")
        }
    };
    let cmd = menu_panel::run(bar_rect(), menu, &look(), scale, &mut flip);
    // the mouse is likely still on the strip: no hover-open until it leaves and comes back
    ui(|u| u.panel.borrow_mut().menu_closed());
    match cmd.unwrap_or(0) {
        1 => FORCE_RT.store(true, Relaxed),
        2 => FORCE_TT.store(true, Relaxed),
        4 => picker::open(ui(|u| u.scale.get())),
        6 => {
            // the settings file shared with the weather widget; changes apply when saved
            let _ = std::process::Command::new("notepad.exe").arg(taskbar_dock::settings::path()).spawn();
        }
        3 => {
            ui(|u| u.quitting.set(true));
            DestroyWindow(ui(|u| u.bar.get()));
            PostQuitMessage(0);
        }
        _ => {}
    }
}

unsafe fn on_tick() {
    let p = ui(|u| u.popup.get());
    let tick = ui(|u| {
        u.ticks.set(u.ticks.get() + 1);
        u.ticks.get()
    });
    watch_settings();

    // Explorer restarted (taskbar re-created): our strip went with the old one, so attach again
    let (bar, embedded, tray) = ui(|u| (u.bar.get(), u.embedded.get(), u.tray.get()));
    let now_tray = dock::taskbar();
    if IsWindow(bar) == 0 || (embedded && now_tray != tray) {
        if now_tray == 0 {
            return; // taskbar not back yet
        }
        if IsWindow(bar) != 0 {
            DestroyWindow(bar);
        }
        create_bar();
    }
    update_hooks();
    let (h, embedded) = ui(|u| (u.bar.get(), u.embedded.get()));

    // follow Windows (theme, animations, hover time, time format, text size, scaling) and the
    // shared style live: anything changed → redraw the strip and the popup
    let _ = tick;
    let force = refresh_look();
    if force {
        ensure_popup_fonts();
    }
    if !embedded {
        // floating: hide for full-screen apps (attached, the taskbar does this for us)
        let fs = fullscreen_app_active();
        if fs != ui(|u| u.hidden.get()) {
            ShowWindow(h, if fs { SW_HIDE } else { SW_SHOWNOACTIVATE });
            ui(|u| u.hidden.set(fs));
        }
        if fs {
            return;
        }
    }
    let text = bar_text();
    // redraw when the times, line colour, line or label change
    let cfg = current().0;
    let state = format!("{text}|{:?}|{}|{}|{}", badge_colors(), cfg.line, cfg.label, cfg.compact);
    let changed = ui(|u| {
        let mut shown = u.shown.borrow_mut();
        let c = force || *shown != state;
        if c {
            *shown = state;
        }
        c
    });
    if changed {
        render_bar(&text);
    }
    if !ui(|u| u.motion.borrow().busy()) {
        // backup for missed events, and keep the strip above the taskbar's own content
        // and follow the room next to the taskbar's buttons (compact or hidden while short)
        let rc = bar_rect();
        match dock_slot() {
            Some(s) if apply_fit(s.fit) => {}
            Some(s) if s.fit != dock::Fit::Hidden && s.x_for(rc.right - rc.left) != rc.left => glide_to(s.x_for(rc.right - rc.left), s.slide),
            _ => move_bar(rc.left, true),
        }
    }
    if IsWindowVisible(p) != 0 {
        position_popup(false);
    }
}

unsafe extern "system" fn bar_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_LBUTTONDOWN => {
            mouse_down();
            SetCapture(h);
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT, // clicking shouldn't activate the taskbar
        WM_MOUSELEAVE => {
            ui(|u| u.hover.set(false));
            // the right-click menu also "leaves" (it takes the mouse); only a real leave, with
            // the pointer outside the strip, ends the block on hover-opening
            let (rc, pt) = (bar_rect(), dock::cursor());
            let outside = !(pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom);
            ui(|u| u.panel.borrow_mut().mouse_left(outside));
            render_bar(&bar_text());
            0
        }
        WM_MOUSEMOVE => {
            if !ui(|u| u.hover.replace(true)) {
                // mouse just came in: highlight, and ask for WM_MOUSELEAVE when it goes
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: h,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut tme);
                render_bar(&bar_text());
                hover_started(); // the popup opens if the mouse stays a moment
            }
            mouse_drag();
            0
        }
        WM_LBUTTONUP => {
            mouse_up(true);
            0
        }
        MSG_CAPTURECHANGED => {
            // the drag was interrupted (Esc, a window took the mouse): end it like a drop
            mouse_up(false);
            0
        }
        WM_RBUTTONUP => {
            context_menu();
            0
        }
        WM_DESTROY => {
            // also happens when Explorer restarts and takes the taskbar (and us) down:
            // only a real Quit ends the program; otherwise on_tick re-creates the strip
            if ui(|u| u.quitting.get()) {
                PostQuitMessage(0);
            }
            0
        }
        // another dock widget opened its panel: close ours (one panel at a time)
        _ if m == dock::close_panel_message() => {
            if IsWindowVisible(ui(|u| u.popup.get())) != 0 {
                popup_closed(false);
            }
            menu_panel::close();
            0
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

/// (Re)makes the popup fonts when the display scale or the shared font settings changed.
unsafe fn ensure_popup_fonts() {
    let (scale, lk) = ui(|u| (u.scale.get(), u.look.borrow().clone()));
    let key = (scale, lk.font.clone(), lk.font_size);
    if ui(|u| *u.fonts_for.borrow() == key) {
        return;
    }
    let font = |t: theme::TextStyle| {
        let face = wide(&lk.font);
        CreateFontW(-t.px_at(scale), 0, 0, 0, t.weight, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr())
    };
    // popup (opaque, so ClearType)
    let fonts = [font(theme::ROW), font(theme::BODY), font(theme::FOOTNOTE)];
    let old = ui(|u| {
        *u.fonts_for.borrow_mut() = key;
        u.fonts.replace(fonts)
    });
    for f in old {
        if f != 0 {
            DeleteObject(f);
        }
    }
}

unsafe fn run_ui() {
    let hinst = GetModuleHandleW(null());

    let bar_cls = wide("BusWidgetBar");
    let pop_cls = wide("BusWidgetPopup");
    let mut wc: WNDCLASSW = zeroed();
    wc.hInstance = hinst;
    wc.hCursor = LoadCursorW(0, IDC_ARROW);
    wc.lpfnWndProc = Some(bar_proc);
    wc.lpszClassName = bar_cls.as_ptr();
    RegisterClassW(&wc);
    wc.lpfnWndProc = Some(popup_proc);
    wc.lpszClassName = pop_cls.as_ptr();
    RegisterClassW(&wc);

    let title = wide("Bus widget");
    let popup = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED, // layered: for the fade-in
        pop_cls.as_ptr(),
        title.as_ptr(),
        WS_POPUP,
        0, 0, 10, 10,
        0, 0, hinst, null(),
    );
    SetLayeredWindowAttributes(popup, 0, 255, LWA_ALPHA);
    // Windows 11's own rounded corners (8 px, the system flyout radius), like the weather flyout
    let round: i32 = DWMWCP_ROUND;
    DwmSetWindowAttribute(popup, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &round as *const i32 as *const c_void, 4);
    ui(|u| {
        u.hinst.set(hinst);
        u.popup.set(popup);
        u.tray.set(dock::taskbar());
    });
    refresh_look();
    ensure_popup_fonts();
    create_bar();
    // the popup window lives as long as the program, so it carries the timer
    SetTimer(popup, TIMER_TICK, 1000, None);

    let mut msg: MSG = zeroed();
    while GetMessageW(&mut msg, 0, 0, 0) > 0 {
        // Tab / Enter / Esc in the stop picker
        let p = picker::hwnd();
        if p != 0 && IsDialogMessageW(p, &msg) != 0 {
            continue;
        }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

fn main() {
    unsafe {
        // one instance only (autostart + manual start)
        let name = wide("Local\\BusWidgetRhonedreef");
        CreateMutexW(null(), 0, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        // a crash closes the widget at once (panic = abort): write why to the log first
        taskbar_dock::crash::log_panics(|m| data::log(data_dir(), m));
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    thread::spawn(worker);
    unsafe { run_ui() };
}
