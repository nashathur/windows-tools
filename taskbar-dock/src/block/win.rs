//! Block's windows: the hidden main window (timers, Explorer restarts, Windows settings, worker
//! posts), the strip (a taskbar child, or floating), and the main panel (`shell.rs`). Every
//! handler borrows the state briefly, never across a Win32 call that can send
//! messages, and carries out the Content's requests after the borrow ([`apply`]).

#[path = "remote.rs"]
mod remote;
#[path = "shell.rs"]
mod shell;

use super::core::{self, Core, Route, Up};
use super::place;
use super::*;
use crate::dock::{self, Fit, FitChange, FitState};
use crate::panel::{Action, Fade};
use crate::strip::{self, DragMove, Glide, Limits, Motion};
use crate::{draw, settings, startup, system, theme::space, wide};
use std::cell::RefCell;
use std::time::{Duration, Instant, SystemTime};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const T_GLIDE: usize = CONTENT_TIMERS + 1;
const T_ZOOM: usize = CONTENT_TIMERS + 2;
const T_RESIZE: usize = CONTENT_TIMERS + 3;
const T_HOVER: usize = CONTENT_TIMERS + 4;
const T_FADE: usize = CONTENT_TIMERS + 5;
const T_FEEL: usize = CONTENT_TIMERS + 6;
const T_WATCH: usize = CONTENT_TIMERS + 7;
/// Animation frames (glide, zoom, resize, fade).
const FRAME_MS: u32 = 10;
/// Hover-open / hover-close checks ([`crate::panel`]).
const HOVER_MS: u32 = 50;
/// Keeping the strip alive and in place, following settings and Windows (backup for the hooks).
const WATCH_MS: u32 = 1000;
/// No taskbar this long (not just Explorer restarting): float.
const FLOAT_AFTER: Duration = Duration::from_secs(10);
/// Attaching failed: try again this often.
const ATTACH_RETRY: Duration = Duration::from_secs(60);
/// Floating without a taskbar: the strip's height at 100%.
const FLOAT_HEIGHT: f32 = 48.0;
const WM_MOUSELEAVE: u32 = 0x02A3;

struct State {
    spec: Spec,
    content: Box<dyn Content>,
    core: Core,
    look: Look,
    main: HWND,
    /// The strip window (0: none right now).
    strip: HWND,
    floating: bool,
    /// Put in place once since it was created (the first placement jumps, never slides).
    placed: bool,
    /// Floating and hidden for a full-screen app.
    fs_hidden: bool,
    /// The main panel's window while open (0: none; [`shell`]).
    panel: HWND,
    fade: Fade,
    hooks: dock::Hooks,
    settings_mtime: Option<SystemTime>,
    taskbar_created: u32,
    no_taskbar_since: Option<Instant>,
    attach_failed: Option<Instant>,
    quitting: bool,
    /// A widget's switch in the right-click panel is being carried out ([`flip`]).
    flipping: bool,
    /// The Content draws a tray icon ([`Content::tray_icon`]): it has the Show on taskbar switch.
    has_tray: bool,
    /// The tray icon is added; its icon (0: none made yet), size and tooltip.
    tray_on: bool,
    tray_icon: HICON,
    tray_size: i32,
    tray_tip: String,
}

/// Block's keys in the widget's settings section.
const COMPACT_KEY: &str = "compact";
const SHOW_KEY: &str = "show_on_taskbar";

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Short, non-reentrant access to the state (None while it's borrowed further up the stack).
fn st<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn log(line: &str) {
    if let Some(l) = st(|s| s.spec.log) {
        l(line);
    }
}

// ---------------------------------------------------------------- public entry points

/// Runs the widget: one instance only, panics logged, the strip created and kept in place, until
/// Quit. Returns when the program should end.
pub fn run<C: Content>(spec: Spec, content: C) {
    unsafe {
        let name = wide(&spec.mutex);
        let _mutex = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        // a crash closes the widget at once (panic = abort): write why to the log first
        crate::crash::log_panics(spec.log);

        let hinst = GetModuleHandleW(std::ptr::null());
        let cursor = LoadCursorW(0, IDC_ARROW);
        let main_class = wide(&format!("{}Main", spec.class));
        let strip_class = wide(spec.class);
        for (cls, proc_) in [(&main_class, main_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT), (&strip_class, strip_proc)] {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(proc_);
            wc.hInstance = hinst;
            wc.hCursor = cursor;
            wc.lpszClassName = cls.as_ptr();
            RegisterClassW(&wc);
        }
        shell::register(spec.class, hinst, cursor);
        let title = wide(spec.title);
        let main = CreateWindowExW(0, main_class.as_ptr(), title.as_ptr(), WS_OVERLAPPED, 0, 0, 0, 0, 0, 0, hinst, std::ptr::null());
        if main == 0 {
            (spec.log)("creating the main window failed");
            return;
        }
        let compact = read_bool(spec.section, COMPACT_KEY, false);
        let show = read_bool(spec.section, SHOW_KEY, true);
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                content: Box::new(content),
                core: Core { compact, show, ..Default::default() },
                look: Look::current(),
                main,
                strip: 0,
                floating: false,
                placed: false,
                fs_hidden: false,
                panel: 0,
                fade: Fade::default(),
                hooks: dock::Hooks::default(),
                settings_mtime: settings::mtime(),
                taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
                no_taskbar_since: None,
                attach_failed: None,
                quitting: false,
                flipping: false,
                has_tray: false,
                tray_on: false,
                tray_icon: 0,
                tray_size: 0,
                tray_tip: String::new(),
                spec,
            })
        });
        // whether it draws a tray icon decides whether it has the switch (and so what shows)
        st(|s| {
            let size = tray::size();
            let icon = tray::draw(s.content.as_mut(), &s.look, size);
            s.has_tray = icon.is_some();
            (s.tray_icon, s.tray_size) = (icon.unwrap_or(0), size);
        });
        call(|c, ctx| c.start(ctx));
        redraw(); // creates the strip
        tray_sync(false, false);
        update_hooks();
        SetTimer(main, T_WATCH, WATCH_MS, None);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// The widget's Content, from outside its callbacks (its own windows, worker posts handled
/// elsewhere): a short borrow, then the requests are carried out. None while Block is busy
/// further up the stack (e.g. a message sent during a Content callback), or for the wrong type.
pub fn with<C: Content, R>(f: impl FnOnce(&mut C, &mut Ctx) -> R) -> Option<R> {
    let (r, reqs) = st(|s| {
        let c = <dyn Content as AsAny>::as_any(s.content.as_mut()).downcast_mut::<C>()?;
        let mut ctx = Ctx::new(s.main);
        let r = f(c, &mut ctx);
        Some((r, ctx.reqs))
    })
    .flatten()?;
    apply(reqs);
    Some(r)
}

/// The strip window (0: none right now), e.g. to place a tray-opened panel.
pub fn strip_window() -> isize {
    st(|s| s.strip).unwrap_or(0)
}

/// The main panel closes for a reason of its own (it lost focus, Esc; or a right click, another
/// panel): the behaviour is told why, the window destroyed.
fn panel_closed(lost_focus: bool) {
    let had = st(|s| {
        if s.panel == 0 {
            return false;
        }
        s.core.panel.closed(Instant::now(), lost_focus);
        true
    });
    if had == Some(true) {
        close_panel_window();
        hover_timer_sync();
    }
}

// ---------------------------------------------------------------- requests

/// A Content callback with a fresh Ctx; its requests are carried out after the borrow.
fn call<R>(f: impl FnOnce(&mut dyn Content, &mut Ctx) -> R) -> Option<R> {
    let (r, reqs) = st(|s| {
        let mut ctx = Ctx::new(s.main);
        let r = f(s.content.as_mut(), &mut ctx);
        (r, ctx.reqs)
    })?;
    apply(reqs);
    Some(r)
}

fn apply(reqs: Vec<Request>) {
    let (mut redraw_after, mut resize_after) = (false, false);
    for r in reqs {
        match r {
            Request::Redraw => redraw_after = true,
            Request::RedrawPanel => {
                if let Some(p) = st(|s| s.panel).filter(|p| *p != 0) {
                    unsafe { InvalidateRect(p, std::ptr::null(), 0) };
                }
            }
            Request::ResizePanel => resize_after = true,
            Request::Every(id, ms) => {
                if let Some(main) = st(|s| s.main) {
                    unsafe { SetTimer(main, id, ms, None) };
                }
            }
            Request::Stop(id) => {
                if let Some(main) = st(|s| s.main) {
                    unsafe { KillTimer(main, id) };
                }
            }
            Request::OpenPanel => {
                if let Some(a) = st(|s| if s.core.panel.is_open() { Action::None } else { s.core.panel.click(Instant::now()) }) {
                    panel_do(a);
                }
            }
            Request::TogglePanel => {
                if let Some(a) = st(|s| s.core.panel.click(Instant::now())) {
                    panel_do(a);
                }
            }
            Request::ClosePanel => close_panels(),
            Request::Menu => right_click(false),
            Request::Quit => quit(),
            Request::After(f) => f(),
        }
    }
    if redraw_after {
        redraw();
        tray_sync(false, false); // the tooltip may follow the data, the icon the state
    }
    // after the redraw: placed by the strip as it is now (it may just have appeared or gone)
    if resize_after {
        if let Some((p, strip)) = st(|s| (s.panel, s.strip)).filter(|(p, _)| *p != 0) {
            shell::resize(p, strip);
        }
    }
}

fn timer(id: usize, on: bool) {
    let Some(main) = st(|s| s.main) else { return };
    unsafe {
        if on {
            SetTimer(main, id, if id == T_HOVER { HOVER_MS } else { FRAME_MS }, None);
        } else {
            KillTimer(main, id);
        }
    }
}

// ---------------------------------------------------------------- window procedures

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            on_timer(wp);
            0
        }
        WM_SETTINGCHANGE | WM_DISPLAYCHANGE | WM_DPICHANGED => {
            // theme, scaling, time format or another Windows setting changed: follow it now
            refresh_look(false);
            redraw();
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_COPYDATA => remote::on_copydata(wp, lp),
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => {
            // Explorer restarted: the taskbar window is new
            if msg != 0 && st(|s| s.taskbar_created) == Some(msg) {
                taskbar_restarted();
                return 0;
            }
            if msg == tray::WM_TRAY {
                match (lp as u32) & 0xFFFF {
                    WM_LBUTTONUP => tray_click(true),
                    WM_RBUTTONUP => tray_click(false),
                    _ => {}
                }
                return 0;
            }
            if msg == remote::click_message() {
                remote::pending_click();
                return 0;
            }
            if msg >= WM_USER {
                if let Some(Some(r)) = call(|c, ctx| c.on_message(msg, wp, lp, ctx)) {
                    return r;
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    }
}

unsafe extern "system" fn strip_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_LBUTTONDOWN => {
            mouse_down(hwnd, lp);
            SetCapture(hwnd);
            0
        }
        WM_LBUTTONUP => {
            mouse_up(hwnd, true, lp);
            0
        }
        WM_CAPTURECHANGED => {
            mouse_up(hwnd, false, 0);
            0
        }
        WM_RBUTTONUP => {
            right_click(false);
            0
        }
        WM_MOUSEMOVE => {
            mouse_move(hwnd, lp);
            0
        }
        WM_MOUSELEAVE => {
            mouse_leave(hwnd);
            0
        }
        WM_DESTROY => {
            // also when Explorer takes the taskbar down: the watch timer makes a new one
            st(|s| {
                if s.strip == hwnd {
                    s.strip = 0;
                }
            });
            0
        }
        // another dock widget opened its panel: close ours (one panel at a time)
        _ if msg == dock::close_panel_message() => {
            close_panels();
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// A click on the tray icon: left opens the main panel, right the right-click panel, both at
/// the pointer.
fn tray_click(left: bool) {
    if left {
        apply(vec![Request::TogglePanel]);
    } else {
        right_click(false);
    }
}

fn on_timer(id: usize) {
    match id {
        T_GLIDE => glide_step(),
        T_ZOOM => zoom_step(),
        T_RESIZE => resize_step(),
        T_HOVER => hover_step(),
        T_FADE => fade_step(),
        T_FEEL => feel_step(),
        T_WATCH => watch(),
        id if id < CONTENT_TIMERS => {
            call(|c, ctx| c.timer(id, ctx));
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- the mouse on the strip

fn hit_at(x: i32, y: i32) -> Hit {
    st(|s| s.content.hit(x, y)).unwrap_or_default()
}

fn feel_sync() {
    if st(|s| s.core.update_feel(s.look.animations)) == Some(true) {
        timer(T_FEEL, true);
    }
}

fn mouse_down(h: HWND, lp: LPARAM) {
    let (x, y) = strip::client_pos(lp);
    let at = hit_at(x, y);
    let (sx, cx) = unsafe { (dock::rect(h).left, dock::cursor().x) };
    st(|s| {
        s.core.down(at, cx, sx);
        s.core.hot = at;
    });
    feel_sync();
    redraw();
}

fn mouse_move(h: HWND, lp: LPARAM) {
    let entered = st(|s| !std::mem::replace(&mut s.core.hover, true)) == Some(true);
    if entered {
        unsafe {
            let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
            TrackMouseEvent(&mut tme);
        }
        // the panel opens if the mouse stays a moment (hover_step)
        let wants = st(|s| {
            if s.spec.hover_panel {
                s.core.panel.mouse_entered(Instant::now());
            }
            s.core.panel.wants_ticks()
        });
        if wants == Some(true) {
            timer(T_HOVER, true);
        }
    }
    let dragging = st(|s| s.core.motion.dragging()).unwrap_or(false);
    let changed = if dragging {
        false
    } else {
        let (x, y) = strip::client_pos(lp);
        let at = hit_at(x, y);
        st(|s| std::mem::replace(&mut s.core.hot, at) != at).unwrap_or(false)
    };
    feel_sync();
    if entered || changed {
        redraw();
    }
    drag(h);
}

fn mouse_leave(h: HWND) {
    // the right-click panel also "leaves" (it takes the mouse); only a real leave, with the
    // pointer outside the strip, ends the block on hover-opening
    let outside = unsafe { !contains(dock::rect(h), dock::cursor()) };
    st(|s| {
        s.core.hover = false;
        s.core.hot = Hit::Strip;
        s.core.panel.mouse_left(outside);
    });
    feel_sync();
    redraw();
}

/// Released (`click`), or the capture lost: a drag ends like a drop either way.
fn mouse_up(h: HWND, click: bool, lp: LPARAM) {
    let at = if click {
        let (x, y) = strip::client_pos(lp);
        hit_at(x, y)
    } else {
        Hit::Strip
    };
    let Some((up, reqs)) = st(|s| {
        let mut ctx = Ctx::new(s.main);
        let up = s.core.up(click, at, s.spec.click, s.content.as_mut(), &mut ctx, Instant::now());
        (up, ctx.reqs)
    }) else { return };
    if click {
        unsafe { ReleaseCapture() }; // its WM_CAPTURECHANGED then finds nothing left to end
    }
    apply(reqs);
    match up {
        Up::Drop => drag_ended(h),
        Up::Panel(a) => panel_do(a),
        Up::Nothing => {}
    }
    feel_sync();
    redraw();
}

/// How far the strip `h`, `w` wide, may go while dragged (screen x): against the tray and (on
/// the right) not over the taskbar's buttons; floating, on the screen.
fn limits(h: HWND, w: i32) -> Limits {
    let Some((floating, side, look)) = st(|s| (s.floating, s.content.side(), s.look.clone())) else { return Limits::NONE };
    if floating {
        return Limits { min: Some(0), max: Some(place::screen_width() - w) };
    }
    unsafe {
        let tb = GetParent(h);
        let (_, gap) = scale_gap(tb, &look);
        let min = dock::icons_right(tb).filter(|_| side == Side::Right).map(|r| r + gap);
        Limits { min, max: Some(max_x(tb, w, &look)) }
    }
}

/// The furthest right an attached strip `w` wide may go: against the tray.
unsafe fn max_x(tb: HWND, w: i32, look: &Look) -> i32 {
    let (scale, gap) = scale_gap(tb, look);
    dock::tray_boundary(tb, scale) - w - gap
}

fn scale_gap(h: HWND, look: &Look) -> (f32, i32) {
    let scale = system::scale_for(h);
    (scale, (look.gap * scale).round() as i32)
}

fn width_of(h: HWND) -> i32 {
    let r = unsafe { dock::rect(h) };
    r.right - r.left
}

/// The mouse moved with the button down: past the threshold it's a drag.
fn drag(h: HWND) {
    let Some((draggable, floating, float_at)) = st(|s| (s.spec.draggable, s.floating, s.spec.float)) else { return };
    // floating beside the dock there's no order to drag in
    if !draggable || (floating && matches!(float_at, Float::Fallback(FloatAt::BesideDock))) {
        return;
    }
    let (cx, w) = (unsafe { dock::cursor().x }, width_of(h));
    let lim = limits(h, w);
    match st(|s| s.core.motion.mouse_move(cx, w, lim, &s.look, Instant::now())) {
        Some(DragMove::Started(x)) => {
            timer(T_GLIDE, false);
            if !floating {
                unsafe { dock::set_dragging(h, true) };
            }
            if let Some(a) = st(|s| s.core.panel.drag_started()) {
                panel_do(a); // a hover-opened panel closes
            }
            crate::menu_panel::close();
            timer(T_ZOOM, true); // picked up: the content grows a little
            feel_sync(); // the cover leaves the size to the zoom
            unsafe { dock::notify(h) }; // the others close up
            drag_move(h, x);
        }
        Some(DragMove::Moved(x)) => drag_move(h, x),
        _ => {}
    }
}

/// While dragged: the strip at x (screen), never past the tray; the slot it would drop into.
fn drag_move(h: HWND, x: i32) {
    let Some((floating, look)) = st(|s| (s.floating, s.look.clone())) else { return };
    unsafe {
        if floating {
            strip::move_strip(h, dock::taskbar(), x, true);
            return;
        }
        let tb = GetParent(h);
        let (scale, gap) = scale_gap(tb, &look);
        strip::move_strip(h, tb, x.min(max_x(tb, width_of(h), &look)), true);
        dock::track_drag(tb, h, dock::tray_boundary(tb, scale), gap);
    }
}

fn drag_ended(h: HWND) {
    timer(T_ZOOM, true); // put down: back to normal size while it glides into place
    let Some((floating, side, class, float_at, look)) =
        st(|s| {
            s.core.panel.drag_ended();
            (s.floating, s.content.side(), s.spec.class, s.spec.float, s.look.clone())
        })
    else { return };
    unsafe {
        if floating {
            if let Float::Fallback(FloatAt::Saved(path)) = float_at {
                place::write_saved(&path(), dock::rect(h).left); // stays where dropped
            }
        } else {
            // save the new order first, then end the drag: the others keep the preview till then
            let tb = GetParent(h);
            if side == Side::Right {
                let (scale, gap) = scale_gap(tb, &look);
                dock::drop_reorder(tb, h, class, dock::tray_boundary(tb, scale), gap);
            }
            dock::set_dragging(h, false);
        }
    }
    reposition(true);
    unsafe { dock::notify(h) };
}

fn contains(r: RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

// ---------------------------------------------------------------- panels

/// Does what the panel behaviour decided.
fn panel_do(a: Action) {
    match a {
        Action::OpenByHover => open_panel(false),
        Action::OpenByClick => open_panel(true),
        Action::Pin => {
            // a click elsewhere now closes it
            if let Some(p) = st(|s| s.panel).filter(|p| *p != 0) {
                unsafe { SetForegroundWindow(p) };
            }
        }
        Action::Close => close_panel_window(),
        Action::None => {}
    }
    hover_timer_sync();
}

fn hover_timer_sync() {
    if st(|s| s.core.panel.wants_ticks()) == Some(false) {
        timer(T_HOVER, false);
    }
}

/// Opens the main panel, fading in. `activate`: opened by a click (takes focus; a click
/// elsewhere closes it); opened by hovering it doesn't take focus. Without a strip (from a tray
/// icon) it opens above the pointer.
fn open_panel(activate: bool) {
    let Some(strip) = st(|s| s.strip) else { return };
    unsafe {
        // only one panel at a time: clicking a widget doesn't activate anything, so the other
        // widgets' panels wouldn't notice the click by themselves
        if strip != 0 {
            dock::close_other_panels(dock::taskbar(), strip);
        }
        let p = shell::open(strip);
        if p == 0 {
            // no panel (the Content has none): the behaviour forgets it opened
            st(|s| s.core.panel.closed(Instant::now(), false));
            return;
        }
        SetLayeredWindowAttributes(p, 0, 0, LWA_ALPHA); // fade_step brings it in
        st(|s| {
            s.panel = p;
            s.fade.start(Instant::now());
        });
        if activate {
            ShowWindow(p, SW_SHOW);
            SetForegroundWindow(p);
        } else {
            ShowWindow(p, SW_SHOWNOACTIVATE);
        }
    }
    timer(T_FADE, true);
}

/// Forgets the panel window and destroys it. Callers tell the behaviour why (or it decided it).
fn close_panel_window() {
    let Some(p) = st(|s| {
        s.fade.stop();
        std::mem::replace(&mut s.panel, 0)
    }) else { return };
    if p != 0 {
        shell::destroy(p);
    }
}

/// Closes every panel of this widget (drag, no room, another widget's panel, Quit).
fn close_panels() {
    panel_closed(false);
    crate::menu_panel::close();
}

/// Every 50 ms while hovering or while the panel is hover-opened.
fn hover_step() {
    let Some((strip, panel, look)) = st(|s| (s.strip, s.panel, s.look.clone())) else { return };
    let pt = unsafe { dock::cursor() };
    let on = |h: HWND| h != 0 && contains(unsafe { dock::rect(h) }, pt);
    if let Some(a) = st(|s| s.core.panel.tick(Instant::now(), on(strip), on(panel), &look)) {
        panel_do(a);
    }
}

fn fade_step() {
    let Some((p, alpha)) = st(|s| (s.panel, s.fade.alpha(Instant::now(), &s.look))) else { return };
    match (p, alpha) {
        (p, Some(a)) if p != 0 => unsafe {
            SetLayeredWindowAttributes(p, 0, a, LWA_ALPHA);
        },
        _ => timer(T_FADE, false),
    }
}

/// The right-click panel (from the strip, or `on_panel`: from the widget's panel, which it
/// replaces).
fn right_click(on_panel: bool) {
    let Some((a, strip, main, look, menu, section)) = st(|s| {
        // a right-click isn't a hover: cancels a pending hover-open, closes a hover-opened panel
        let a = s.core.panel.right_click(on_panel);
        let show = s.has_tray.then_some(s.core.show);
        let menu = core::menu(s.content.as_mut(), show, s.core.compact, startup::is_on(s.spec.section));
        (a, s.strip, s.main, s.look.clone(), menu, s.spec.section)
    }) else { return };
    panel_do(a);
    // the left-click panel switches to this one
    panel_closed(false);
    let open = crate::menu_panel::is_open();
    unsafe {
        let r = dock::rect(strip);
        let pt = dock::cursor();
        let (left, top, right, bottom) = core::menu_anchor((strip != 0).then_some((r.left, r.top, r.right, r.bottom)), (pt.x, pt.y));
        let anchor = RECT { left, top, right, bottom };
        if !open && strip != 0 {
            dock::close_other_panels(dock::taskbar(), strip); // one panel at a time across widgets
        }
        let scale = system::scale_for(if strip != 0 { strip } else { main });
        let mut on_switch = |id: usize| flip(id, section);
        let cmd = crate::menu_panel::run(anchor, menu, &look, scale, &mut on_switch);
        // the mouse is likely still on the strip: no hover-open until it leaves and comes back
        st(|s| s.core.panel.menu_closed());
        match cmd.map(core::route) {
            Some(Route::Settings) => {
                let (op, app, path) = (wide("open"), wide("notepad.exe"), wide(&settings::path().display().to_string()));
                ShellExecuteW(0, op.as_ptr(), app.as_ptr(), path.as_ptr(), std::ptr::null(), SW_SHOWNORMAL);
            }
            Some(Route::Quit) => quit(),
            Some(Route::Content(id)) => {
                call(|c, ctx| c.menu_action(id, ctx));
            }
            _ => {}
        }
    }
}

/// A switch in the right-click panel flipped: its new state (the panel stays open).
fn flip(id: usize, section: &str) -> bool {
    match core::route(id) {
        Route::Show => {
            let on = !st(|s| s.core.show).unwrap_or(true);
            if let Err(e) = save_bool(section, SHOW_KEY, on) {
                log(&format!("saving Show on taskbar failed: {e}"));
            }
            st(|s| {
                s.core.show = on;
                s.settings_mtime = settings::mtime();
                s.flipping = true; // the strip goes, this panel stays open
            });
            redraw();
            st(|s| s.flipping = false);
            tray_sync(false, false);
            on
        }
        Route::Compact => {
            let on = !st(|s| s.core.compact).unwrap_or(false);
            if let Err(e) = save_bool(section, COMPACT_KEY, on) {
                log(&format!("saving compact mode failed: {e}"));
            }
            st(|s| {
                s.core.compact = on;
                s.settings_mtime = settings::mtime(); // our own write isn't an edit to follow
            });
            redraw(); // animates the width
            on
        }
        Route::Startup => {
            if let Err(e) = startup::set(section, !startup::is_on(section)) {
                log(&format!("changing Start with Windows failed: {e}"));
            }
            startup::is_on(section)
        }
        Route::Content(id) => {
            st(|s| s.flipping = true);
            let on = call(|c, ctx| c.menu_switch(id, ctx)).unwrap_or(false);
            st(|s| s.flipping = false);
            on
        }
        _ => false,
    }
}

fn read_bool(section: &str, key: &str, default: bool) -> bool {
    settings::section(section).and_then(|v| v.get(key)?.as_bool()).unwrap_or(default)
}

fn save_bool(section: &str, key: &str, on: bool) -> std::io::Result<()> {
    use serde_json::{Map, Value};
    settings::update(|root| {
        if !root.is_object() {
            *root = Value::Object(Map::new());
        }
        let sec = root.as_object_mut().map(|o| o.entry(section).or_insert_with(|| Value::Object(Map::new())));
        if let Some(sec) = sec {
            if !sec.is_object() {
                *sec = Value::Object(Map::new());
            }
            sec[key] = Value::Bool(on);
        }
    })
}

fn quit() {
    let Some((main, already)) = st(|s| (s.main, std::mem::replace(&mut s.quitting, true))) else { return };
    if already {
        return;
    }
    close_panels();
    destroy_strip();
    tray_sync(false, false); // quitting: removes the tray icon
    st(|s| s.content.closing());
    unsafe { DestroyWindow(main) }; // WM_DESTROY ends the message loop
}

/// What [`tray_sync`] does after the borrow.
enum TrayDo {
    Remove(HWND, HICON),
    Set { main: HWND, icon: HICON, tip: String, add: bool, old: HICON },
}

/// Adds, updates or removes the tray icon: shown whenever the strip isn't (`core::tray_shown`),
/// never while quitting. `redo`: draw the logo again (theme, scaling); `re_add`: Explorer
/// restarted (the tray lost it). The tooltip is compared and changed only when it changed.
fn tray_sync(redo: bool, re_add: bool) {
    let size = tray::size();
    let todo = st(|s| {
        let want = !s.quitting && core::tray_shown(s.core.show, s.content.visible(), s.has_tray);
        if !want {
            if !s.tray_on {
                return None;
            }
            s.tray_on = false;
            s.tray_tip.clear();
            return Some(TrayDo::Remove(s.main, std::mem::replace(&mut s.tray_icon, 0)));
        }
        let add = re_add || !s.tray_on;
        let mut changed = add;
        let mut old = 0;
        if redo || s.tray_icon == 0 || s.tray_size != size {
            if let Some(icon) = tray::draw(s.content.as_mut(), &s.look, size) {
                old = std::mem::replace(&mut s.tray_icon, icon);
                s.tray_size = size;
                changed = true;
            }
        }
        let mut tip = s.content.tray_tip();
        if tip.is_empty() {
            tip = s.spec.title.to_string();
        }
        if tip != s.tray_tip {
            s.tray_tip = tip.clone();
            changed = true;
        }
        s.tray_on = true;
        changed.then_some(TrayDo::Set { main: s.main, icon: s.tray_icon, tip, add, old })
    })
    .flatten();
    unsafe {
        match todo {
            Some(TrayDo::Remove(main, icon)) => {
                tray::remove(main);
                tray::destroy(icon);
                log("tray icon removed");
            }
            Some(TrayDo::Set { main, icon, tip, add, old }) => {
                tray::set(main, icon, &tip, add);
                tray::destroy(old);
                if add {
                    log("tray icon added");
                }
            }
            None => {}
        }
    }
}

// ---------------------------------------------------------------- the strip window

/// The strip as a taskbar child, or None (logged; floating may follow).
fn attach(tb: HWND) -> Option<HWND> {
    let (class, title) = st(|s| (wide(s.spec.class), wide(s.spec.title)))?;
    let h = unsafe {
        // visible at 1×1 and empty: in the taskbar's list of children (dock::slot) from the start
        CreateWindowExW(WS_EX_LAYERED | WS_EX_NOACTIVATE, class.as_ptr(), title.as_ptr(), WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0, 0, 1, 1, tb, 0, GetModuleHandleW(std::ptr::null()), std::ptr::null())
    };
    if h == 0 {
        log(&format!("attaching the strip to the taskbar failed (error {})", unsafe { GetLastError() }));
        st(|s| s.attach_failed = Some(Instant::now()));
        return None;
    }
    Some(h)
}

/// Creates the strip: a taskbar child if possible, else (attaching failed, no taskbar for
/// [`FLOAT_AFTER`], or `force_float`) a floating topmost window, if the Spec allows.
fn create_strip() {
    let Some((ok, float, force, failed)) =
        st(|s| (s.strip == 0 && !s.quitting && core::shown(s.core.show, s.content.visible(), s.has_tray), s.spec.float, s.spec.force_float, s.attach_failed))
    else { return };
    if !ok {
        return;
    }
    let tb = unsafe { dock::taskbar() };
    let retry = failed.is_none_or(|t| t.elapsed() >= ATTACH_RETRY);
    if tb != 0 && !force && retry {
        if let Some(h) = attach(tb) {
            return adopt(h, false);
        }
    }
    if matches!(float, Float::Off) {
        return; // the watch timer tries again
    }
    // right after an Explorer restart the taskbar is missing for a moment: wait for it
    if tb == 0 && !force && failed.is_none() {
        let since = st(|s| *s.no_taskbar_since.get_or_insert_with(Instant::now));
        if since.is_none_or(|t| t.elapsed() < FLOAT_AFTER) {
            return;
        }
    }
    let Some((class, title)) = st(|s| (wide(s.spec.class), wide(s.spec.title))) else { return };
    let h = unsafe {
        CreateWindowExW(WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW, class.as_ptr(), title.as_ptr(), WS_POPUP,
            0, 0, 1, 1, 0, 0, GetModuleHandleW(std::ptr::null()), std::ptr::null())
    };
    if h == 0 {
        log(&format!("creating the floating strip failed (error {})", unsafe { GetLastError() }));
        return;
    }
    adopt(h, true);
}

fn adopt(h: HWND, floating: bool) {
    let was = st(|s| {
        let was = s.floating;
        s.strip = h;
        s.floating = floating;
        s.placed = false;
        s.fs_hidden = false;
        // a new strip starts full, at rest
        s.core.fit = FitState::default();
        s.core.drawn_compact = None;
        s.core.motion = Motion::default();
        s.core.hover = false;
        s.core.hot = Hit::Strip;
        if !floating {
            s.attach_failed = None;
            s.no_taskbar_since = None;
        }
        was
    });
    log(match (floating, was) {
        (true, _) => "strip floating above the taskbar",
        (false, Some(true)) => "strip attached to the taskbar again",
        _ => "strip attached to the taskbar",
    });
    redraw();
}

/// Closes the panels and destroys the strip (it leaves the dock stack).
fn destroy_strip() {
    close_panels();
    remove_strip();
}

/// Destroys the strip (it leaves the dock stack); the panels are the caller's business.
fn remove_strip() {
    let Some(h) = st(|s| std::mem::replace(&mut s.strip, 0)).filter(|h| *h != 0) else { return };
    for t in [T_GLIDE, T_ZOOM, T_RESIZE, T_FEEL] {
        timer(t, false);
    }
    st(|s| {
        s.core.motion = Motion::default();
        s.core.hover = false;
    });
    unsafe {
        dock::leave(h);
        DestroyWindow(h);
    }
}

/// Explorer restarted: keeps the strip if it's already on the new taskbar, otherwise replaces
/// it. (Forgetting a live strip left an untracked duplicate on the taskbar, 2026-10-10.)
fn taskbar_restarted() {
    call(|c, ctx| c.taskbar_restarted(ctx));
    tray_sync(false, true); // the new tray doesn't have it
    let Some((strip, floating)) = st(|s| {
        s.attach_failed = None;
        s.no_taskbar_since = None;
        (s.strip, s.floating)
    }) else { return };
    if floating {
        float_watch(); // attaches now if it can
    } else if strip != 0 {
        let current = unsafe { IsWindow(strip) != 0 && GetParent(strip) == dock::taskbar() };
        if !current {
            destroy_strip();
        }
    }
    redraw();
    update_hooks();
}

/// Floating: back onto the taskbar when possible; hidden while a full-screen app is in front.
fn float_watch() {
    let Some((strip, floating, force, failed, fs_hidden)) = st(|s| (s.strip, s.floating, s.spec.force_float, s.attach_failed, s.fs_hidden)) else { return };
    if !floating || strip == 0 {
        return;
    }
    let tb = unsafe { dock::taskbar() };
    if tb != 0 && !force && failed.is_none_or(|t| t.elapsed() >= ATTACH_RETRY) {
        if let Some(h) = attach(tb) {
            destroy_strip();
            return adopt(h, false);
        }
    }
    let fs = unsafe { place::fullscreen_app_active() };
    if fs != fs_hidden {
        st(|s| s.fs_hidden = fs);
        unsafe { ShowWindow(strip, if fs { SW_HIDE } else { SW_SHOWNOACTIVATE }) };
    }
}

/// Every second: follows the look and the settings file, keeps the strip alive and in place
/// (backup for the hooks; re-asserts the z-order), re-hooks restarted processes.
fn watch() {
    refresh_look(false);
    follow_settings();
    if let Some(strip) = st(|s| s.strip).filter(|h| *h != 0) {
        if unsafe { IsWindow(strip) } == 0 {
            st(|s| s.strip = 0);
        }
    }
    float_watch();
    redraw(); // also creates the strip when it's missing and wanted
    tray_sync(false, false); // the switch or the state may have changed
    update_hooks();
}

/// The settings file changed (an edit, or dockctl's `set`): Block's keys, then the widget.
fn follow_settings() {
    let m = settings::mtime();
    if let Some(Some(section)) = st(|s| (s.settings_mtime != m).then_some(s.spec.section)) {
        let (compact, show) = (read_bool(section, COMPACT_KEY, false), read_bool(section, SHOW_KEY, true));
        st(|s| {
            s.settings_mtime = m;
            s.core.compact = compact;
            s.core.show = show;
        });
        call(|c, ctx| c.settings_changed(ctx));
    }
}

/// Re-reads the shared style and Windows' settings; tells the Content and the panel if changed
/// (`force`: even if not, for dockctl's `simulate look-changed`).
fn refresh_look(force: bool) {
    let new = Look::current();
    let Some((changed, panel)) = st(|s| {
        let changed = force || s.look != new;
        s.look = new;
        (changed, s.panel)
    }) else { return };
    if changed {
        call(|c, ctx| c.look_changed(ctx));
        tray_sync(true, false); // the logo follows the theme
        if panel != 0 {
            unsafe { InvalidateRect(panel, std::ptr::null(), 0) };
        }
    }
}

fn update_hooks() {
    let Some(mut hooks) = st(|s| std::mem::take(&mut s.hooks)) else { return };
    unsafe { hooks.update(dock::taskbar(), Some(on_win_event)) };
    st(|s| s.hooks = hooks);
}

/// A neighbour (tray, another widget) changed: reposition right away instead of waiting for the
/// watch timer.
unsafe extern "system" fn on_win_event(_: isize, _event: u32, hwnd: isize, id_object: i32, _: i32, _: u32, _: u32) {
    let tb = dock::taskbar();
    let Some((strip, floating)) = st(|s| (s.strip, s.floating)) else { return };
    if strip == 0 || !dock::is_neighbour_event(hwnd, id_object, tb, strip) {
        return;
    }
    if floating {
        redraw(); // beside the dock: follows its left end
    } else {
        reposition(false);
    }
}

// ---------------------------------------------------------------- drawing and placing

fn frame<'a>(look: &'a Look, core: &Core, floating: bool, height: i32, dpi: f32, zoom: f32, compact: bool) -> Frame<'a> {
    Frame {
        look,
        height,
        dpi,
        zoom,
        scale: dpi * zoom,
        compact,
        hover: core.hover,
        hot: core.hot,
        pressed: core.motion.pressed(),
        floating,
        fit_width: None,
    }
}

/// Measures, draws and places the strip; creates it when it's missing and wanted, destroys it
/// when [`Content::visible`] says no.
fn redraw() {
    let Some((strip, visible, quitting, floating, fs_hidden, flipping)) =
        st(|s| (s.strip, core::shown(s.core.show, s.content.visible(), s.has_tray), s.quitting, s.floating, s.fs_hidden, s.flipping))
    else { return };
    if quitting {
        return;
    }
    if !visible {
        if strip != 0 {
            if flipping {
                // a switch in the open right-click panel did this (Show on taskbar): that panel
                // stays open, as switches do
                panel_closed(false);
                remove_strip();
            } else {
                destroy_strip(); // hidden by state (discord, media): every panel closes
            }
            log("switched off or nothing to show: strip removed");
        }
        return;
    }
    if strip == 0 {
        return create_strip(); // draws it once made
    }
    unsafe {
        if IsWindow(strip) == 0 {
            st(|s| s.strip = 0);
            return;
        }
        let tb = if floating { dock::taskbar() } else { GetParent(strip) };
        let dpi = system::scale_for(strip);
        let height = match (tb, floating) {
            (0, _) => (FLOAT_HEIGHT * dpi).round() as i32,
            (tb, true) => {
                let r = dock::rect(tb);
                r.bottom - r.top
            }
            (tb, false) => {
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(tb, &mut rc);
                rc.bottom - rc.top
            }
        };
        if height <= 0 {
            return;
        }
        // its widths for the dock's fit: in the chosen mode, and compact (unzoomed)
        let Some((size, side, zoom, dragging, look, stretch)) = st(|s| {
            let f = frame(&s.look, &s.core, floating, height, dpi, 1.0, s.core.compact);
            let chosen = s.content.measure(&f, s.core.compact);
            let compact = s.content.measure(&f, true);
            (core::size(chosen, compact, &s.spec), s.content.side(), s.core.motion.zoom(), s.core.motion.dragging(), s.look.clone(), s.spec.stretch)
        }) else { return };
        let right = !floating && side == Side::Right;
        let gap = (look.gap * dpi).round() as i32;
        // on the right it's part of the dock stack against the tray; elsewhere it isn't
        if right {
            dock::mark_member(strip);
            if zoom == 1.0 {
                dock::publish_widths(strip, size);
            }
        } else if !floating {
            dock::leave(strip);
        }
        // the room next to the taskbar's buttons: narrower, compact, or hidden while there's none
        let slot = if right { dock::slot(tb, strip, size, dock::tray_boundary(tb, dpi), gap) } else { None };
        let fit = if right { slot.map(|s| s.fit) } else { Some(Fit::Full) };
        if let Some(f) = fit.filter(|_| !dragging) {
            apply_fit(strip, f, false);
        }
        if st(|s| s.core.fit.hidden()) != Some(false) {
            return;
        }
        // compact switched (chosen, or for room): the width animates; the full content is drawn
        // cut off meanwhile
        let Some((compact, narrowed)) = st(|s| s.core.mode(stretch, size.min)) else { return };
        let resize_w = start_resize(strip, compact, || {
            st(|s| {
                let f = frame(&s.look, &s.core, floating, height, dpi, 1.0, compact);
                s.content.measure(&f, compact)
            })
            .unwrap_or(1)
        });
        let shown_compact = compact && resize_w.is_none();
        let Some(Some((mut cv, feel, cover, hover, dragging_now, hot, tint))) = st(|s| {
            let mut f = frame(&s.look, &s.core, floating, height, dpi, zoom, shown_compact);
            f.fit_width = narrowed.map(|w| (w as f32 * zoom).round() as i32);
            let natural = s.content.measure(&f, shown_compact).max(1);
            let mut cv = Canvas::new(natural, height)?;
            s.content.draw(&mut cv, &f);
            let hot = s.core.hot;
            // a brand tint for the strip (and the hovered button's own)
            let tint = s.content.tint(Hit::Strip, &s.look).map(|t| (t, matches!(hot, Hit::Button(..)).then(|| s.content.tint(hot, &s.look)).flatten()));
            Some((cv, s.core.feel.value(), s.content.cover(), s.core.hover, s.core.motion.dragging(), hot, tint))
        }) else { return };
        let natural = cv.w;
        // the cover's hover / press feel, then the cut-off, then the background under it all
        if let Some(b) = cover {
            draw::grow_pixels(cv.pixels(), natural, b, feel);
        }
        let width = [resize_w, narrowed].into_iter().flatten().min().map_or(natural, |c| c.clamp(1, natural));
        let fade = (space::M * dpi * zoom).round() as i32;
        let mut px = core::cut(cv.pixels(), natural, height, width, fade);
        let (radius, inset) = draw::highlight_shape(&look, dpi * zoom);
        match tint {
            None => {
                let hl = core::highlight(hover, dragging_now, hot, width, height, inset);
                draw::background_rect(&mut px, width, height, hl, look.light, radius);
            }
            Some((strip_tint, button_tint)) => {
                // the faint base (the whole strip takes the mouse), then the brand boxes under it all
                draw::background_rect(&mut px, width, height, None, look.light, radius);
                for (b, rgb, alpha) in core::tints(strip_tint, button_tint, hover, dragging_now, hot, width, height, inset) {
                    draw::tint_box(&mut px, width, b, (rgb >> 16 & 0xff, rgb >> 8 & 0xff, rgb & 0xff), alpha, radius);
                }
            }
        }
        drop(cv);
        let Some(mut out) = Canvas::new(width, height) else { return };
        out.pixels().copy_from_slice(&px);
        let before = dock::rect(strip);
        let sz = SIZE { cx: width, cy: height };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        UpdateLayeredWindow(strip, 0, std::ptr::null(), &sz, out.dc, &src, 0, &blend, ULW_ALPHA);

        if floating {
            if !dragging {
                let x = floating_x(tb, width, gap, dpi);
                strip::move_strip(strip, tb, x, true);
            }
            if IsWindowVisible(strip) == 0 && !fs_hidden {
                ShowWindow(strip, SW_SHOWNOACTIVATE);
            }
            st(|s| s.placed = true);
            return;
        }
        let target = |w: i32| match (right, slot) {
            (true, Some(s)) => s.x_for(w),
            (true, None) => dock::rect(strip).left, // the list can't be trusted right now: stay
            (false, _) => place::left_side(tb, strip, w, gap, dpi),
        };
        let placed = st(|s| std::mem::replace(&mut s.placed, true)).unwrap_or(true);
        if IsWindowVisible(strip) == 0 || !placed {
            // appearing (new, or room again): straight into place
            st(|s| s.core.motion.stop_glide());
            timer(T_GLIDE, false);
            strip::move_strip(strip, tb, target(width), true);
            ShowWindow(strip, SW_SHOWNOACTIVATE);
            dock::notify(strip);
            return;
        }
        if dragging {
            return; // the mouse has it
        }
        let settled = st(|s| s.core.motion.settled()).unwrap_or(true);
        if right && settled {
            // docked against the tray: a new width keeps the right edge, not the left
            strip::keep_right_edge(strip, tb, before);
        }
        if dock::needs_raise(strip) {
            // the taskbar's own content came over it: back on top (never on every redraw)
            SetWindowPos(strip, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        // after a drop (zooming back) or while another member is dragged: slide, so it flows
        move_to(strip, tb, target(width), !settled || slot.is_some_and(|s| s.slide));
    }
}

/// Floating: where the strip goes.
unsafe fn floating_x(tb: HWND, width: i32, gap: i32, dpi: f32) -> i32 {
    let saved = match st(|s| s.spec.float) {
        Some(Float::Fallback(FloatAt::Saved(path))) => place::read_saved(&path()),
        _ => None,
    };
    saved.unwrap_or_else(|| place::floating_x(tb, width, gap, dpi))
}

/// Compact mode switched since the last drawing: animate the width from the current one to the
/// new mode's (`to`), for a shown strip. The width to draw at while that runs.
fn start_resize(strip: HWND, compact: bool, to: impl FnOnce() -> i32) -> Option<i32> {
    let switched = st(|s| s.core.drawn_compact.replace(compact)).flatten().is_some_and(|c| c != compact);
    if switched && unsafe { IsWindowVisible(strip) } != 0 {
        let from = st(|s| s.core.motion.resize_width()).flatten().unwrap_or_else(|| width_of(strip));
        let to = to();
        if st(|s| s.core.motion.resize(from, to, &s.look, Instant::now())) == Some(true) {
            timer(T_RESIZE, true);
        }
    }
    st(|s| s.core.motion.resize_width()).flatten()
}

/// The room next to the taskbar's buttons changed (dock::fit): hide or show the strip, and with
/// `redraw` also redraw it (compact, narrower or full).
fn apply_fit(strip: HWND, fit: Fit, redraw_it: bool) -> FitChange {
    let Some(change) = st(|s| s.core.fit.update(fit)) else { return FitChange::None };
    unsafe {
        match change {
            FitChange::Hide => {
                close_panels();
                dock::set_squeezed(strip, true); // stays in the stack's order while hidden
                ShowWindow(strip, SW_HIDE);
                dock::notify(strip);
                log("no room next to the taskbar's buttons: strip hidden");
            }
            FitChange::Show => {
                dock::set_squeezed(strip, false);
                log("room again next to the taskbar's buttons: strip shown");
                if redraw_it {
                    redraw();
                }
            }
            FitChange::Redraw if redraw_it => redraw(),
            _ => {}
        }
    }
    change
}

/// An attached strip at x (screen), never past the tray.
unsafe fn put(strip: HWND, tb: HWND, x: i32) {
    let Some(look) = st(|s| s.look.clone()) else { return };
    strip::move_strip(strip, tb, x.min(max_x(tb, width_of(strip), &look)), false);
}

/// Heads for x: a slide or a jump (rules in `strip`).
fn move_to(strip: HWND, tb: HWND, x: i32, always_slide: bool) {
    let cur = unsafe { dock::rect(strip).left };
    match st(|s| s.core.motion.glide_to(cur, x, always_slide, &s.look, Instant::now())) {
        Some(Glide::Jump(x)) => {
            timer(T_GLIDE, false);
            unsafe { put(strip, tb, x) };
        }
        Some(Glide::Slide) => timer(T_GLIDE, true),
        _ => {}
    }
}

/// Recomputes only the position (no redraw): a neighbour moved, or a drop. Not while pressed.
fn reposition(always_slide: bool) {
    let Some((strip, floating, pressed, side, look)) =
        st(|s| (s.strip, s.floating, s.core.motion.pressed(), s.content.side(), s.look.clone()))
    else { return };
    if strip == 0 || floating || pressed {
        return;
    }
    unsafe {
        let tb = GetParent(strip);
        let (scale, gap) = scale_gap(tb, &look);
        let width = width_of(strip);
        let (x, slide) = if side == Side::Right {
            let size = dock_size(strip, width);
            let Some(slot) = dock::slot(tb, strip, size, dock::tray_boundary(tb, scale), gap) else { return };
            // the room changed: redrawn compact or full, or hidden
            if apply_fit(strip, slot.fit, true) != FitChange::None {
                return;
            }
            (slot.x_for(width), slot.slide)
        } else {
            (place::left_side(tb, strip, width, gap, scale), false)
        };
        if IsWindowVisible(strip) == 0 {
            return;
        }
        move_to(strip, tb, x, always_slide || slide);
    }
}

/// The widths this strip published for the fit; its window width if none yet.
unsafe fn dock_size(strip: HWND, width: i32) -> dock::Size {
    dock::children(GetParent(strip)).into_iter().find(|c| c.hwnd == strip).map_or(dock::Size::fixed(width), |c| c.size)
}

fn glide_step() {
    let step = st(|s| s.core.motion.glide_step(&s.look, Instant::now())).flatten();
    let strip = st(|s| (s.strip != 0 && !s.floating).then_some(s.strip)).flatten();
    let (Some(step), Some(strip)) = (step, strip) else {
        st(|s| s.core.motion.stop_glide());
        return timer(T_GLIDE, false);
    };
    unsafe { put(strip, GetParent(strip), step.value) };
    if step.done {
        timer(T_GLIDE, false);
    }
}

/// One frame of the zoom: redraw at the new size (which also heads for the slot after a drop);
/// while dragged keep it centred under the mouse.
fn zoom_step() {
    let Some(Some(step)) = st(|s| s.core.motion.zoom_step(&s.look, Instant::now())) else {
        return timer(T_ZOOM, false);
    };
    redraw();
    if let Some(strip) = st(|s| s.strip).filter(|h| *h != 0) {
        let (cx, w) = (unsafe { dock::cursor().x }, width_of(strip));
        let lim = limits(strip, w);
        if let Some(Some(x)) = st(|s| s.core.motion.drag_x(cx, w, lim)) {
            drag_move(strip, x);
        }
    }
    if step.done {
        timer(T_ZOOM, false);
        feel_sync(); // at rest again: the cover may react
    }
}

/// One frame of the collapse / expand animation (the last one draws the new mode).
fn resize_step() {
    let step = st(|s| s.core.motion.resize_step(&s.look, Instant::now())).flatten();
    if step.is_none_or(|s| s.done) {
        timer(T_RESIZE, false);
    }
    redraw();
}

fn feel_step() {
    if st(|s| s.core.feel.tick(Instant::now())) != Some(true) {
        timer(T_FEEL, false);
    }
    redraw();
}
