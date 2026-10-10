//! The panel window: the click panel (hovering or a left click on the strip: full control) or
//! the menu panel (right click: what the app's menu would hold, as a panel). Hovering opens the
//! click panel without taking focus; it closes when the mouse leaves, and a click on the strip
//! or on the panel pins it (then a click elsewhere closes it).
//!
//! Opening and closing follow the shared taskbar_dock::panel::PanelBehaviour, so they work like
//! the bus and weather widgets' panels: a click on the strip opens one with focus, the same
//! click again closes it, a click elsewhere or Esc closes it, and opening it closes the other
//! widgets' panels. A right click while the click panel is open switches to the menu, and a
//! left click while the menu is open switches to the click panel.

use crate::panel_view::{self, Bar, Content, Layout, Mode, Part};
use crate::render::Canvas;
use crate::{button_cmd, buttons, current, is_live, look, raw, realtime, ring_level, with, Cmd, Msg, Progress};
use std::ffi::c_void;
use std::time::Instant;
use taskbar_dock::panel::Action;
use taskbar_dock::{dock, system};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const CLASS: windows::core::PCWSTR = w!("TaskbarMediaPanel");
pub const TIMER_HOVER: usize = 6;
pub const TIMER_FADE: usize = 7;
pub const TIMER_FRAME: usize = 8;
const WM_MOUSELEAVE: u32 = 0x02A3;

/// The panel's state.
pub struct Panel {
    pub hwnd: Option<HWND>,
    pub mode: Mode,
    /// When it opened (scrolling text starts from here).
    pub opened: Instant,
    pub hover: Option<Part>,
    pub pressed: Option<Part>,
    /// The progress bar being dragged, at this fraction.
    pub seek: Option<f32>,
    /// Something in it moves (scrolling text, pulsing bar): redraw every frame.
    pub moving: bool,
    pub painted: Instant,
    pub tracking: bool,
    /// The shown app's Volume Mixer entries (and whose, and when they were looked up), its
    /// volume as last read, and the slider being dragged, at this level.
    pub vol_entries: Vec<windows::Win32::Media::Audio::ISimpleAudioVolume>,
    pub vol_id: String,
    pub vol_at: Option<Instant>,
    pub volume: Option<crate::audio::Volume>,
    pub vol_drag: Option<f32>,
}

impl Default for Panel {
    fn default() -> Self {
        Panel { hwnd: None, mode: Mode::Click, opened: Instant::now(), hover: None, pressed: None, seek: None, moving: false,
                painted: Instant::now(), tracking: false, vol_entries: Vec::new(), vol_id: String::new(), vol_at: None,
                volume: None, vol_drag: None }
    }
}

fn scale() -> f32 {
    with(|a| a.strip.map(|s| system::scale_for(raw(s)))).flatten().unwrap_or(1.0)
}

fn layout_for(mode: Mode) -> Layout {
    let width = with(|a| match mode {
        Mode::Click => a.cfg.click_panel_width,
        Mode::Menu => a.cfg.menu_panel_width,
    })
    .unwrap_or(300.0);
    panel_view::layout(mode, width, scale())
}

/// Where a panel of this mode goes on screen (centred on the strip, above the taskbar).
fn place(mode: Mode) -> RECT {
    let l = layout_for(mode);
    let Some(Some(strip)) = with(|a| a.strip) else { return RECT::default() };
    let (x, y) = taskbar_dock::panel::place(raw(strip), l.w, l.h, &look(), scale());
    RECT { left: x, top: y, right: x + l.w, bottom: y + l.h }
}

/// What the panel shows, from the session on the strip.
pub fn content(a: &crate::App) -> Option<Content> {
    let s = current(a)?;
    let level = ring_level(a, s);
    let bar = match s.progress() {
        _ if is_live(a, s) => Bar::Live(level),
        Progress::At { pos, len } if realtime(a, s) => Bar::At { pos, len, seek: s.can_seek },
        _ => Bar::Pulse(level),
    };
    let app = crate::apps::display_name(&s.id);
    // the brand: "YouTube · Firefox", the logo or badge without a cover, the colour
    let id = crate::identity(a, s);
    Some(Content {
        title: if s.title.is_empty() { app.clone() } else { s.title.clone() },
        artist: s.artist.clone(),
        album: s.album.clone(),
        source: id.source,
        cover: id.picture,
        badge: id.badge,
        accent: id.color,
        wash: 0.10 * a.cfg.tint_strength.clamp(0.0, 3.0),
        bar,
        buttons: buttons(a, s),
        volume: a.popup.volume,
        app,
        compact: a.cfg.compact,
        startup: crate::startup_enabled(),
    })
}

// ---------------------------------------------------------------- opening and closing

/// Does what the shared panel behaviour decided (hovering, a left click): the click panel.
pub fn act(action: Action) {
    match action {
        Action::OpenByHover => open(Mode::Click, false),
        Action::OpenByClick => open(Mode::Click, true),
        Action::Pin => {
            if let Some(Some(h)) = with(|a| a.popup.hwnd) {
                unsafe { let _ = SetForegroundWindow(h); }
            }
        }
        Action::Close => close(),
        Action::None => {}
    }
    if let Some((main, false)) = with(|a| (a.main, a.panel.wants_ticks())) {
        unsafe { let _ = KillTimer(main, TIMER_HOVER); }
    }
}

fn open_mode() -> Option<Mode> {
    with(|a| a.popup.hwnd.map(|_| a.popup.mode)).flatten()
}

/// A left click on the strip (cover or text): the click panel opens or closes; if the menu
/// panel is showing, it switches to the click panel.
pub fn left_click() {
    if open_mode() == Some(Mode::Menu) {
        closed(false);
    }
    if let Some(a) = with(|a| a.panel.click(Instant::now())) {
        act(a);
    }
}

/// A right click on the strip or a panel: the menu panel opens, or closes if it's showing;
/// if the click panel is showing, it switches to the menu.
pub fn right_click() {
    match open_mode() {
        Some(Mode::Menu) => closed(false),
        other => {
            if other.is_some() {
                closed(false);
            }
            // the same rules as a click (focus, closing), with the menu panel
            if let Some(Action::OpenByClick) = with(|a| a.panel.click(Instant::now())) {
                open(Mode::Menu, true);
            }
        }
    }
}

/// Opens a panel; `activate`: with focus (a click), else without (hovering), so typing
/// elsewhere isn't interrupted.
fn open(mode: Mode, activate: bool) {
    close();
    let Some(Some(strip)) = with(|a| a.strip) else { return };
    if with(|a| current(a).is_some()) != Some(true) {
        with(|a| a.panel.closed(Instant::now(), false));
        return;
    }
    if let Some(tb) = crate::taskbar() {
        // one panel at a time across the widgets
        unsafe { dock::close_other_panels(raw(tb), raw(strip)); }
    }
    let rc = place(mode);
    let light = look().light;
    unsafe {
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED, CLASS, w!("Now playing"), WS_POPUP,
            rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top, None, None, crate::hinstance(), None) else { return };
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const c_void, 4);
        let dark: BOOL = (!light).into();
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const c_void, 4);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA); // fades in
        let main = with(|a| {
            a.popup = Panel { hwnd: Some(hwnd), mode, opened: Instant::now(), ..Default::default() };
            a.pfade.start(Instant::now());
            a.main
        });
        if activate {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        if let Some(main) = main {
            SetTimer(main, TIMER_FADE, 10, None);
            SetTimer(main, TIMER_FRAME, 33, None);
        }
    }
}

pub fn close() {
    let Some((main, hwnd)) = with(|a| {
        a.pfade.stop();
        let h = a.popup.hwnd.take();
        a.popup = Panel::default();
        (a.main, h)
    }) else { return };
    unsafe {
        let _ = KillTimer(main, TIMER_FADE);
        let _ = KillTimer(main, TIMER_FRAME);
        if let Some(h) = hwnd {
            let _ = DestroyWindow(h);
        }
    }
}

/// The panel closed for a reason of its own (lost focus, Esc, another widget's panel).
pub fn closed(lost_focus: bool) {
    with(|a| a.panel.closed(Instant::now(), lost_focus));
    close();
}

pub fn fade_step() {
    let lk = look();
    let Some((main, hwnd, alpha)) = with(|a| (a.main, a.popup.hwnd, a.pfade.alpha(Instant::now(), &lk))) else { return };
    unsafe {
        match (hwnd, alpha) {
            (Some(h), Some(alpha)) => { let _ = SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA); }
            _ => { let _ = KillTimer(main, TIMER_FADE); }
        }
    }
}

/// ~30 times a second while open: redraw if something moves (scrolling text, pulsing bar, a
/// drag), else twice a second (the times). Closes it if the session went away.
pub fn frame() {
    let Some((hwnd, moving, painted, gone)) = with(|a| (a.popup.hwnd, a.popup.moving || a.popup.seek.is_some(), a.popup.painted, current(a).is_none())) else { return };
    let Some(hwnd) = hwnd else { return };
    if gone {
        closed(false);
        return;
    }
    let volume_changed = refresh_volume();
    if moving || volume_changed || painted.elapsed().as_millis() >= 500 {
        unsafe { let _ = InvalidateRect(hwnd, None, false); }
    }
}

// ---------------------------------------------------------------- hovering

/// The mouse came onto the strip: the click panel opens if it stays (see hover_step).
pub fn hover_started() {
    let Some((main, true)) = with(|a| {
        a.panel.mouse_entered(Instant::now());
        (a.main, a.panel.wants_ticks())
    }) else { return };
    unsafe { SetTimer(main, TIMER_HOVER, 50, None); }
}

fn contains(rc: &RECT, pt: POINT) -> bool {
    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
}

fn pointer_on(h: Option<HWND>) -> bool {
    let Some(h) = h else { return false };
    let (mut rc, mut pt) = (RECT::default(), POINT::default());
    unsafe {
        let _ = GetWindowRect(h, &mut rc);
        let _ = GetCursorPos(&mut pt);
    }
    contains(&rc, pt)
}

/// Every 50 ms while hovering or while the panel is hover-opened: opens it after the hover
/// delay, closes a hover-opened one once the mouse has left both the strip and the panel.
pub fn hover_step() {
    let Some((strip, popup)) = with(|a| (a.strip, a.popup.hwnd)) else { return };
    let (on_strip, on_panel) = (pointer_on(strip), pointer_on(popup));
    let lk = look();
    if let Some(a) = with(|a| a.panel.tick(Instant::now(), on_strip, on_panel, &lk)) {
        act(a);
    }
}

// ---------------------------------------------------------------- volume

/// The click panel's volume, kept current (changes made in the Volume Mixer show up): the
/// app's entries are looked up again every second, the volume read twice a second. Not while
/// the slider is being dragged. True if it changed.
fn refresh_volume() -> bool {
    let Some(Some((id, mode, dragging, at, same_app))) = with(|a| {
        current(a).map(|s| (s.id.clone(), a.popup.mode, a.popup.vol_drag.is_some(), a.popup.vol_at, a.popup.vol_id == s.id))
    }) else { return false };
    if mode != Mode::Click || dragging || at.is_some_and(|t| t.elapsed().as_millis() < 500 && same_app) {
        return false;
    }
    let stale = !same_app || at.is_none_or(|t| t.elapsed().as_millis() >= 1000);
    let fresh = stale.then(|| crate::audio::entries(&id));
    with(|a| {
        if let Some(e) = fresh {
            a.popup.vol_entries = e;
            a.popup.vol_id = id;
        }
        a.popup.vol_at = Some(Instant::now());
        let v = crate::audio::get(&a.popup.vol_entries);
        std::mem::replace(&mut a.popup.volume, v) != v
    })
    .unwrap_or(false)
}

/// Sets the shown app's volume from the slider (unmuting it, like the Volume Mixer).
fn set_volume(level: f32) {
    with(|a| {
        crate::audio::set_level(&a.popup.vol_entries, level);
        if let Some(v) = a.popup.volume.as_mut() {
            v.level = level;
            if level > 0.0 {
                v.muted = false;
            }
        }
    });
}

fn toggle_mute() {
    with(|a| {
        if let Some(v) = a.popup.volume.as_mut() {
            v.muted = !v.muted;
            crate::audio::set_muted(&a.popup.vol_entries, v.muted);
        }
    });
}

// ---------------------------------------------------------------- the menu panel's rows

/// Puts text on the clipboard.
fn copy_text(text: &str) {
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let Some(main) = with(|a| a.main) else { return };
    unsafe {
        if OpenClipboard(main).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let p = GlobalLock(mem) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                let _ = GlobalUnlock(mem);
                // the clipboard owns the memory from here
                let _ = SetClipboardData(CF_UNICODETEXT, HANDLE(mem.0));
            }
        }
        let _ = CloseClipboard();
    }
}

/// Brings the shown session's app window to the front (the strip's cover, like the panel's Open).
pub fn open_app() {
    if let Some(id) = with(|a| current(a).map(|s| s.id.clone())).flatten() {
        crate::apps::activate(&id);
    }
}

fn row_clicked(hwnd: HWND, row: usize) {
    use panel_view::*;
    match row {
        ROW_OPEN => {
            let id = with(|a| current(a).map(|s| s.id.clone())).flatten();
            closed(false);
            if let Some(id) = id {
                crate::apps::activate(&id);
            }
        }
        ROW_COPY => {
            let text = with(|a| current(a).map(|s| if s.artist.is_empty() { s.title.clone() } else { format!("{} – {}", s.artist, s.title) })).flatten();
            if let Some(t) = text {
                copy_text(&t);
            }
            closed(false);
        }
        // the switches flip and the panel stays open
        ROW_COMPACT => crate::set_config(|c| c.compact = !c.compact),
        ROW_STARTUP => crate::set_config(|c| c.start_with_windows = !crate::startup_enabled()),
        ROW_SETTINGS => {
            closed(false);
            crate::open_settings();
        }
        ROW_QUIT => {
            closed(false);
            crate::quit();
        }
        _ => {}
    }
    unsafe { let _ = InvalidateRect(hwnd, None, false); }
}

// ---------------------------------------------------------------- the panel window

fn client_pt(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

fn paint(hwnd: HWND) {
    let Some(Some((mode, content, opened, hover, seek, vol_drag, live))) = with(|a| {
        content(a).map(|c| (a.popup.mode, c, a.popup.opened, a.popup.hover, a.popup.seek, a.popup.vol_drag, a.cfg.live_rgb()))
    }) else { return };
    let lk = look();
    let scale = scale();
    let l = layout_for(mode);
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        if let Some(mut cv) = Canvas::new(l.w, l.h) {
            let moving = panel_view::paint(&mut cv, &l, &content, &lk, scale, opened.elapsed().as_secs_f32(), hover, seek, vol_drag, live);
            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, cv.dc, 0, 0, SRCCOPY);
            with(|a| {
                a.popup.moving = moving;
                a.popup.painted = Instant::now();
            });
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

/// A click panel button or the seek bar was used.
fn act_on(part: Part, at_fraction: Option<f32>) {
    let Some(Some(cmd)) = with(|a| {
        let s = current(a)?;
        let id = s.id.clone();
        match part {
            // the same buttons as the strip's; a dimmed one does nothing
            Part::Button(i) => button_cmd(a, i),
            Part::Bar => match (s.progress(), at_fraction) {
                (Progress::At { len, .. }, Some(f)) => Some(Cmd::Seek(id, f as f64 * len)),
                _ => None,
            },
            // the volume row and the menu rows work directly, not through the media worker
            Part::Mute | Part::Volume | Part::Row(_) => None,
        }
    }) else { return };
    with(|a| {
        let _ = a.media.send(Msg::Cmd(cmd));
    });
}

/// Can the bar be dragged now?
fn seekable() -> bool {
    with(|a| content(a).is_some_and(|c| matches!(c.bar, Bar::At { seek: true, .. }))).unwrap_or(false)
}

fn mode() -> Mode {
    with(|a| a.popup.mode).unwrap_or(Mode::Click)
}

pub extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                paint(hwnd);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEMOVE => {
                let (x, y) = client_pt(lp);
                let l = layout_for(mode());
                let part = l.hit(x, y);
                let (changed, track) = with(|a| {
                    let changed = a.popup.hover != part;
                    a.popup.hover = part;
                    if a.popup.seek.is_some() {
                        a.popup.seek = Some(l.bar_fraction(x));
                    }
                    (changed || a.popup.seek.is_some() || a.popup.vol_drag.is_some(), !std::mem::replace(&mut a.popup.tracking, true))
                })
                .unwrap_or((false, false));
                // dragging the volume slider: the volume follows the pointer
                if let Some(Some(_)) = with(|a| a.popup.vol_drag) {
                    let f = l.volume_fraction(x);
                    with(|a| a.popup.vol_drag = Some(f));
                    set_volume(f);
                }
                if track {
                    let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut tme);
                }
                if changed {
                    let _ = InvalidateRect(hwnd, None, false);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                with(|a| {
                    a.popup.hover = None;
                    a.popup.tracking = false;
                });
                let _ = InvalidateRect(hwnd, None, false);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                // a click in a hover-opened panel pins it (it stays until a click elsewhere),
                // and the click still does what it's on
                if with(|a| a.panel.by_hover()) == Some(true) {
                    if let Some(a) = with(|a| a.panel.click(Instant::now())) {
                        act(a);
                    }
                }
                let (x, y) = client_pt(lp);
                let l = layout_for(mode());
                let part = l.hit(x, y);
                with(|a| a.popup.pressed = part);
                if part == Some(Part::Bar) && seekable() {
                    with(|a| a.popup.seek = Some(l.bar_fraction(x)));
                    SetCapture(hwnd);
                    let _ = InvalidateRect(hwnd, None, false);
                }
                if part == Some(Part::Volume) && with(|a| a.popup.volume.is_some()) == Some(true) {
                    let f = l.volume_fraction(x);
                    with(|a| a.popup.vol_drag = Some(f));
                    set_volume(f);
                    SetCapture(hwnd);
                    let _ = InvalidateRect(hwnd, None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = client_pt(lp);
                let l = layout_for(mode());
                let here = l.hit(x, y);
                let (pressed, seek, vol) = with(|a| (a.popup.pressed.take(), a.popup.seek.take(), a.popup.vol_drag.take())).unwrap_or((None, None, None));
                if let Some(f) = seek {
                    let _ = ReleaseCapture();
                    act_on(Part::Bar, Some(f));
                } else if let Some(f) = vol {
                    let _ = ReleaseCapture();
                    set_volume(f);
                } else if pressed.is_some() && pressed == here {
                    // a button, row or the speaker acts when released where it was pressed
                    match pressed {
                        Some(Part::Button(i)) => act_on(Part::Button(i), None),
                        Some(Part::Mute) => toggle_mute(),
                        Some(Part::Row(r)) => {
                            row_clicked(hwnd, r);
                            return LRESULT(0); // the panel may be gone
                        }
                        _ => {}
                    }
                }
                let _ = InvalidateRect(hwnd, None, false);
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                with(|a| {
                    a.popup.seek = None;
                    a.popup.vol_drag = None;
                });
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                // on the click panel: switch to the menu; on the menu: nothing
                if mode() == Mode::Click {
                    right_click();
                }
                LRESULT(0)
            }
            WM_ACTIVATE => {
                // only if still ours: closing it ourselves also deactivates it
                if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE && with(|a| a.popup.hwnd == Some(hwnd)) == Some(true) {
                    closed(true);
                }
                LRESULT(0)
            }
            WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
                closed(false);
                LRESULT(0)
            }
            WM_DESTROY => {
                with(|a| {
                    if a.popup.hwnd == Some(hwnd) {
                        a.popup = Panel::default();
                    }
                });
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
