//! Block's windows: the hidden main window (timers, Explorer restarts, Windows settings, worker
//! posts), the strip (a taskbar child, or floating), and the main panel (`shell.rs`). Every
//! handler borrows the state briefly, never across a Win32 call that can send
//! messages, and carries out the Content's requests after the borrow ([`apply`]).

#[path = "remote.rs"]
mod remote;
#[path = "shell.rs"]
mod shell;

use super::core::{self, Core, Route, Up};
use super::*;
use super::{frames, place};
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
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const T_GLIDE: usize = CONTENT_TIMERS + 1;
const T_ZOOM: usize = CONTENT_TIMERS + 2;
const T_RESIZE: usize = CONTENT_TIMERS + 3;
const T_HOVER: usize = CONTENT_TIMERS + 4;
const T_FADE: usize = CONTENT_TIMERS + 5;
const T_FEEL: usize = CONTENT_TIMERS + 6;
const T_WATCH: usize = CONTENT_TIMERS + 7;
/// The strip fading in (new) or out (before it's removed).
const T_APPEAR: usize = CONTENT_TIMERS + 8;
/// The animations, run on the display's refresh ([`frames`]) instead of a timer.
const FRAME_TIMERS: [usize; 6] = [T_GLIDE, T_ZOOM, T_RESIZE, T_FADE, T_FEEL, T_APPEAR];
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
/// Floating: the backdrop's opacity.
const FLOAT_BACKDROP: f32 = 0.96;
const WM_MOUSELEAVE: u32 = 0x02A3;

struct State {
    spec: Spec,
    content: Box<dyn Content>,
    core: Core,
    look: Look,
    highlight: Highlight,
    build_time: u64,
    main: HWND,
    /// The strip window (0: none right now).
    strip: HWND,
    floating: bool,
    /// Put in place once since it was created (the first placement jumps, never slides).
    placed: bool,
    /// The strip's opacity: fading in when it's new, out before it's removed.
    appear: crate::anim::Appear,
    /// Fading out: switched off or nothing to show; removed when the fade ends. It keeps its
    /// slot meanwhile (the others close up after).
    leaving: bool,
    /// Floating and hidden for a full-screen app.
    fs_hidden: bool,
    /// The main panel's window while open (0: none; [`shell`]).
    panel: HWND,
    panel_kind: PanelKind,
    fade: Fade,
    /// The [`FRAME_TIMERS`] running now (bit i: `FRAME_TIMERS[i]`).
    frames: u32,
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
    idle: bool,
    content_timers: core::ContentTimers,
}

/// Block's keys in the widget's settings section.
const COMPACT_KEY: &str = "compact";
const SHOW_KEY: &str = "show_on_taskbar";
const HIGHLIGHT_KEY: &str = "highlight";
const HIGHLIGHT_COLOR_KEY: &str = "highlight_color";
const HIGHLIGHT_STRENGTH_KEY: &str = "highlight_strength";

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Short, non-reentrant access to the state (None while it's borrowed further up the stack).
fn st<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

pub(super) fn log(line: &str) {
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
        let section_existed = settings::section(spec.section).is_some();
        let mut defaults = spec
            .defaults
            .map(|make| make())
            .unwrap_or_else(|| serde_json::json!({}));
        if !defaults.is_object() {
            defaults = serde_json::json!({});
        }
        defaults[SHOW_KEY] = serde_json::json!(true);
        defaults[COMPACT_KEY] = serde_json::json!(false);
        defaults[HIGHLIGHT_KEY] = serde_json::json!("auto");
        defaults[HIGHLIGHT_COLOR_KEY] = serde_json::json!("#0078D4");
        defaults[HIGHLIGHT_STRENGTH_KEY] = serde_json::json!(1.0);
        if let Err(e) = settings::fill_missing(spec.section, &defaults) {
            (spec.log)(&format!("filling settings defaults failed: {e}"));
        }
        // a crash closes the widget at once (panic = abort): write why to the log first
        crate::crash::log_panics(spec.log);

        let hinst = GetModuleHandleW(std::ptr::null());
        let cursor = LoadCursorW(0, IDC_ARROW);
        let main_class = wide(&format!("{}Main", spec.class));
        let strip_class = wide(spec.class);
        for (cls, proc_) in [
            (
                &main_class,
                main_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
            ),
            (&strip_class, strip_proc),
        ] {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(proc_);
            wc.hInstance = hinst;
            wc.hCursor = cursor;
            wc.lpszClassName = cls.as_ptr();
            RegisterClassW(&wc);
        }
        shell::register(spec.class, hinst, cursor);
        let title = wide(spec.title);
        let main = CreateWindowExW(
            0,
            main_class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            0,
            0,
            hinst,
            std::ptr::null(),
        );
        if main == 0 {
            (spec.log)("creating the main window failed");
            return;
        }
        let compact = read_bool(spec.section, COMPACT_KEY, false);
        let show = read_bool(spec.section, SHOW_KEY, true);
        let highlight = read_highlight(spec.section);
        let build_time = std::env::current_exe()
            .ok()
            .and_then(|path| std::fs::metadata(path).ok()?.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_secs());
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                content: Box::new(content),
                core: Core {
                    compact,
                    show,
                    ..Default::default()
                },
                look: Look::current(),
                highlight,
                build_time,
                main,
                strip: 0,
                floating: false,
                placed: false,
                appear: crate::anim::Appear::hidden(Instant::now()),
                leaving: false,
                fs_hidden: false,
                panel: 0,
                panel_kind: PanelKind::Main,
                fade: Fade::default(),
                frames: 0,
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
                idle: false,
                content_timers: core::ContentTimers::default(),
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
        // idle before start, so a widget started switched off doesn't fetch first
        refresh_idle();
        call(|c, ctx| c.start(ctx));
        if core::first_run_startup(section_existed) {
            let name = st(|s| s.spec.section).unwrap_or("");
            if let Err(e) = startup::set(name, true) {
                log(&format!("enabling Start with Windows failed: {e}"));
            }
        }
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

/// Recomputes Block's Idle rule, pausing content timers and notifying the Content on transitions.
fn refresh_idle() {
    let change = st(|s| {
        let next = core::idle(s.core.show, s.has_tray, s.panel != 0);
        if next == s.idle {
            return None;
        }
        s.idle = next;
        let timers = s.content_timers.set_idle(next);
        Some((s.main, timers, next))
    })
    .flatten();
    let Some((main, timers, idle)) = change else {
        return;
    };
    for (id, interval) in timers {
        unsafe {
            if let Some(ms) = interval {
                SetTimer(main, id, ms, None);
            } else {
                KillTimer(main, id);
            }
        }
    }
    log(if idle { "idle" } else { "awake" });
    call(|c, ctx| c.idle(idle, ctx));
    tray_sync(false, false); // the tooltip says whether it's paused
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
            Request::RedrawTray => tray_sync(true, false),
            Request::Every(id, ms) => {
                let arm = st(|s| (s.content_timers.every(id, ms), s.main));
                if let Some((Some(ms), main)) = arm {
                    unsafe { SetTimer(main, id, ms, None) };
                }
            }
            Request::Stop(id) => {
                let kill = st(|s| (s.content_timers.stop(id), s.main));
                if let Some((true, main)) = kill {
                    unsafe { KillTimer(main, id) };
                }
            }
            Request::OpenPanel => {
                if let Some(a) = st(|s| {
                    if s.core.panel.is_open() {
                        Action::None
                    } else {
                        s.core.panel.click(Instant::now())
                    }
                }) {
                    panel_do(a);
                }
            }
            Request::OpenPanelKind(kind) => open_panel_kind(kind),
            Request::TogglePanel => {
                if let Some(a) = st(|s| s.core.panel.click(Instant::now())) {
                    panel_do(a);
                }
            }
            Request::ClosePanel => close_panels(),
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

/// Starts or stops one of Block's timers. The animations ([`FRAME_TIMERS`]) run on the display's
/// refresh ([`frames`]): each frame calls every running one ([`on_frame`]).
fn timer(id: usize, on: bool) {
    let Some(main) = st(|s| s.main) else { return };
    if let Some(bit) = FRAME_TIMERS.iter().position(|t| *t == id) {
        let Some(running) = st(|s| {
            if on {
                s.frames |= 1 << bit
            } else {
                s.frames &= !(1 << bit)
            }
            s.frames
        }) else {
            return;
        };
        return frames::run(main, running != 0);
    }
    unsafe {
        if on {
            SetTimer(main, id, HOVER_MS, None);
        } else {
            KillTimer(main, id);
        }
    }
}

/// A display refresh while animations run: one step of each (in [`FRAME_TIMERS`] order; one that
/// a step before stopped is skipped).
fn on_frame() {
    frames::done();
    for (bit, id) in FRAME_TIMERS.into_iter().enumerate() {
        if st(|s| s.frames & (1 << bit) != 0) == Some(true) {
            on_timer(id);
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
        WM_COMMAND_REPLY => {
            remote::command_reply(lp);
            0
        }
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
            if msg == frames::message() {
                on_frame();
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
    // fading out: it's going, clicks do nothing
    if matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONUP) && st(|s| s.leaving) == Some(true) {
        return 0;
    }
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
        T_APPEAR => appear_step(),
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
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: h,
                dwHoverTime: 0,
            };
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
        let up = s
            .core
            .up(click, at, s.content.as_mut(), &mut ctx, Instant::now());
        (up, ctx.reqs)
    }) else {
        return;
    };
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
    let Some((floating, side, look)) = st(|s| (s.floating, s.content.side(), s.look.clone()))
    else {
        return Limits::NONE;
    };
    if floating {
        return Limits {
            min: Some(0),
            max: Some(place::screen_width() - w),
        };
    }
    unsafe {
        let tb = GetParent(h);
        let (_, gap) = scale_gap(tb, &look);
        let min = dock::icons_right(tb)
            .filter(|_| side == Side::Right)
            .map(|r| r + gap);
        Limits {
            min,
            max: Some(max_x(tb, w, &look)),
        }
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
    let Some((draggable, floating, float_at)) =
        st(|s| (s.spec.draggable, s.floating, s.spec.float))
    else {
        return;
    };
    // floating beside the dock there's no order to drag in
    if !draggable || (floating && matches!(float_at, Float::Fallback(FloatAt::BesideDock))) {
        return;
    }
    let (cx, w) = (unsafe { dock::cursor().x }, width_of(h));
    let lim = limits(h, w);
    match st(|s| {
        s.core
            .motion
            .mouse_move(cx, w, lim, &s.look, Instant::now())
    }) {
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
    let Some((floating, look)) = st(|s| (s.floating, s.look.clone())) else {
        return;
    };
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
    let Some((floating, side, class, float_at, look)) = st(|s| {
        s.core.panel.drag_ended();
        (
            s.floating,
            s.content.side(),
            s.spec.class,
            s.spec.float,
            s.look.clone(),
        )
    }) else {
        return;
    };
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
    let _ = open_panel_kind_with(PanelKind::Main, activate);
}

fn open_panel_kind(kind: PanelKind) {
    let _ = open_panel_kind_with(kind, true);
}

fn open_panel_kind_with(kind: PanelKind, activate: bool) -> bool {
    let Some(strip) = st(|s| s.strip) else {
        return false;
    };
    let anchor = if strip != 0 && unsafe { IsWindowVisible(strip) } != 0 {
        strip
    } else {
        0
    };
    if kind != PanelKind::Main {
        let target = if anchor != 0 {
            anchor
        } else {
            st(|s| s.main).unwrap_or(0)
        };
        if st(|s| {
            s.content
                .panel_size(kind, system::scale_for(target))
                .is_some()
        }) != Some(true)
        {
            return false;
        }
    }
    if kind != PanelKind::Main {
        // Secondary panels are always pinned and focused. Close the current panel and menus
        // before opening so their callbacks each see one clean transition.
        panel_closed(false);
        crate::menu_panel::close();
        let _ = st(|s| s.core.panel.open_pinned());
        timer(T_HOVER, false);
    }
    unsafe {
        // only one panel at a time: clicking a widget doesn't activate anything, so the other
        // widgets' panels wouldn't notice the click by themselves
        if kind != PanelKind::Main || strip != 0 {
            let owner = if strip != 0 {
                strip
            } else {
                st(|s| s.main).unwrap_or(0)
            };
            dock::close_other_panels(dock::taskbar(), owner);
        }
        let p = shell::open(anchor, kind);
        if p == 0 {
            // no panel (the Content has none): the behaviour forgets it opened
            st(|s| s.core.panel.closed(Instant::now(), false));
            return false;
        }
        SetLayeredWindowAttributes(p, 0, 0, LWA_ALPHA); // fade_step brings it in
        st(|s| {
            s.panel = p;
            s.panel_kind = kind;
            s.fade.start(Instant::now());
        });
        if activate {
            ShowWindow(p, SW_SHOW);
            SetForegroundWindow(p);
            if kind != PanelKind::Main {
                SetFocus(p);
            }
        } else {
            ShowWindow(p, SW_SHOWNOACTIVATE);
        }
    }
    timer(T_FADE, true);
    call(|c, ctx| c.panel_shown(kind, true, ctx));
    refresh_idle();
    true
}

/// Forgets the panel window and destroys it. Callers tell the behaviour why (or it decided it).
fn close_panel_window() {
    let Some((p, kind)) = st(|s| {
        s.fade.stop();
        (std::mem::replace(&mut s.panel, 0), s.panel_kind)
    }) else {
        return;
    };
    if p != 0 {
        shell::destroy(p);
        call(|c, ctx| c.panel_shown(kind, false, ctx));
        refresh_idle();
    }
}

/// Closes every panel of this widget (drag, no room, another widget's panel, Quit).
fn close_panels() {
    panel_closed(false);
    crate::menu_panel::close();
}

/// Dispatches a normalized key through the same callback used by physical input.
pub(super) fn dispatch_panel_key(key: PanelKey) -> bool {
    let Some(kind) = st(|s| {
        if s.panel != 0 {
            Some(s.panel_kind)
        } else {
            None
        }
    })
    .flatten() else {
        return false;
    };
    call(|c, ctx| c.panel_key(kind, key, ctx)).unwrap_or(false)
}

pub(super) fn dispatch_panel_text(text: &str) {
    if let Some(kind) = st(|s| {
        if s.panel != 0 {
            Some(s.panel_kind)
        } else {
            None
        }
    })
    .flatten()
    {
        call(|c, ctx| c.panel_char(kind, text, ctx));
    }
}

pub(super) fn dispatch_panel_wheel(delta: i32) {
    if let Some(kind) = st(|s| {
        if s.panel != 0 {
            Some(s.panel_kind)
        } else {
            None
        }
    })
    .flatten()
    {
        call(|c, ctx| c.panel_wheel(kind, delta, ctx));
    }
}

pub(super) fn dispatch_panel_vk(vk: u32, ctrl: bool, alt: bool) {
    dispatch_panel_key_with_fallback(panel_key_from_vk(vk, ctrl, alt));
}

/// Converts a Win32 virtual key into Block's normalized panel key.
pub(super) fn panel_key_from_vk(vk: u32, ctrl: bool, alt: bool) -> PanelKey {
    let key = match vk {
        0x0D => PanelKeyCode::Enter,
        0x1B => PanelKeyCode::Escape,
        0x26 => PanelKeyCode::Up,
        0x28 => PanelKeyCode::Down,
        0x21 => PanelKeyCode::PageUp,
        0x22 => PanelKeyCode::PageDown,
        0x24 => PanelKeyCode::Home,
        0x23 => PanelKeyCode::End,
        0x08 => PanelKeyCode::Backspace,
        0x2E => PanelKeyCode::Delete,
        0x25 => PanelKeyCode::Left,
        0x27 => PanelKeyCode::Right,
        0x09 => PanelKeyCode::Tab,
        0x41 => PanelKeyCode::A,
        0x43 => PanelKeyCode::C,
        0x56 => PanelKeyCode::V,
        0x58 => PanelKeyCode::X,
        n => PanelKeyCode::Other(n),
    };
    PanelKey { key, ctrl, alt }
}

/// Dispatches a key and closes the panel when its Content declines Escape.
pub(super) fn dispatch_panel_key_with_fallback(key: PanelKey) -> bool {
    let taken = dispatch_panel_key(key);
    if !taken && key.key == PanelKeyCode::Escape {
        panel_closed(false);
    }
    taken
}

thread_local! { static HIGH_SURROGATE: std::cell::Cell<Option<u16>> = const { std::cell::Cell::new(None) }; }

pub(super) fn dispatch_panel_utf16(unit: u16) {
    if (0xD800..=0xDBFF).contains(&unit) {
        HIGH_SURROGATE.set(Some(unit));
        return;
    }
    if (0xDC00..=0xDFFF).contains(&unit) {
        if let Some(high) = HIGH_SURROGATE.take() {
            if let Some(ch) = char::decode_utf16([high, unit]).next().and_then(Result::ok) {
                dispatch_panel_text(&ch.to_string());
            }
        }
        return;
    }
    HIGH_SURROGATE.set(None);
    if let Some(ch) = char::from_u32(unit as u32).filter(|c| !c.is_control()) {
        dispatch_panel_text(&ch.to_string());
    }
}

/// Every 50 ms while hovering or while the panel is hover-opened.
fn hover_step() {
    let Some((strip, panel, look)) = st(|s| (s.strip, s.panel, s.look.clone())) else {
        return;
    };
    let pt = unsafe { dock::cursor() };
    let on = |h: HWND| h != 0 && contains(unsafe { dock::rect(h) }, pt);
    if let Some(a) = st(|s| {
        let now = Instant::now();
        // on one of the strip's buttons: no hover-open from there (the delay starts over off it)
        if s.core.hover && matches!(s.core.hot, Hit::Button(..)) {
            s.core.panel.over_button(now);
        }
        s.core.panel.tick(now, on(strip), on(panel), &look)
    }) {
        panel_do(a);
    }
}

fn fade_step() {
    let Some((p, alpha)) = st(|s| (s.panel, s.fade.alpha(Instant::now(), &s.look))) else {
        return;
    };
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
        let menu = core::menu(
            s.content.as_mut(),
            show,
            s.core.compact,
            startup::is_on(s.spec.section),
        );
        (a, s.strip, s.main, s.look.clone(), menu, s.spec.section)
    }) else {
        return;
    };
    panel_do(a);
    // the left-click panel switches to this one
    panel_closed(false);
    let open = crate::menu_panel::is_open();
    unsafe {
        let r = dock::rect(strip);
        let pt = dock::cursor();
        let (left, top, right, bottom) = core::menu_anchor(
            (strip != 0).then_some((r.left, r.top, r.right, r.bottom)),
            (pt.x, pt.y),
        );
        let anchor = RECT {
            left,
            top,
            right,
            bottom,
        };
        if !open && strip != 0 {
            dock::close_other_panels(dock::taskbar(), strip); // one panel at a time across widgets
        }
        let scale = system::scale_for(if strip != 0 { strip } else { main });
        let mut on_switch = |id: usize| flip(id, section);
        let mut on_choice = |id: usize, option: usize| pick(id, option);
        let cmd = crate::menu_panel::run(anchor, menu, &look, scale, &mut on_switch, &mut on_choice);
        // the mouse is likely still on the strip: no hover-open until it leaves and comes back
        st(|s| s.core.panel.menu_closed());
        match cmd.map(core::route) {
            Some(Route::Settings) => {
                let section = section.to_string();
                std::thread::spawn(move || launch_settings_app(&section));
            }
            Some(Route::Quit) => quit(),
            Some(Route::Content(id)) => {
                call(|c, ctx| c.menu_action(id, ctx));
            }
            _ => {}
        }
    }
}

/// Starts the settings app on its widget page, falling back to Notepad if it cannot open.
fn launch_settings_app(section: &str) {
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, WaitForSingleObject, PROCESS_INFORMATION, STARTUPINFOW,
    };

    const STILL_ACTIVE: u32 = 259;
    let exe = std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "\\taskbar-dock-settings.exe"
    ));
    if !exe.is_file() {
        open_settings_notepad();
        return;
    }
    let folder = exe.parent().unwrap_or_else(|| std::path::Path::new("."));
    let application = wide(&exe.to_string_lossy());
    let directory = wide(&folder.to_string_lossy());
    let command_line = format!("\"{}\" --widget {}", exe.to_string_lossy(), section);
    let mut command_line = wide(&command_line);
    unsafe {
        let mut startup: STARTUPINFOW = std::mem::zeroed();
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        if CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            directory.as_ptr(),
            &startup,
            &mut process,
        ) == 0
        {
            open_settings_notepad();
            return;
        }
        CloseHandle(process.hThread);
        let deadline = Instant::now() + Duration::from_secs(5);
        let decision = loop {
            let window_seen = settings_window_exists(process.dwProcessId);
            let mut code = STILL_ACTIVE;
            let exit_code = (GetExitCodeProcess(process.hProcess, &mut code) != 0
                && code != STILL_ACTIVE)
                .then_some(code);
            let timed_out = Instant::now() >= deadline;
            let decision = core::settings_launch_decision(exit_code, window_seen, timed_out);
            if decision != core::SettingsLaunchDecision::Wait {
                break decision;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let wait_ms = remaining.as_millis().min(100) as u32;
            WaitForSingleObject(process.hProcess, wait_ms);
        };
        CloseHandle(process.hProcess);
        if decision == core::SettingsLaunchDecision::OpenNotepad {
            open_settings_notepad();
        }
    }
}

/// Checks whether the settings process owns a top-level window of the app's class.
fn settings_window_exists(pid: u32) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowThreadProcessId,
    };
    struct Search {
        pid: u32,
        found: bool,
    }
    /// Stops enumerating after finding the settings window owned by this process.
    unsafe extern "system" fn visit(window: HWND, parameter: LPARAM) -> BOOL {
        let search = &mut *(parameter as *mut Search);
        let mut owner = 0;
        GetWindowThreadProcessId(window, &mut owner);
        if owner != search.pid {
            return 1;
        }
        let mut name = [0u16; 128];
        let length = GetClassNameW(window, name.as_mut_ptr(), name.len() as i32);
        let expected: Vec<u16> = "TaskbarDockSettings".encode_utf16().collect();
        if length > 0 && name[..length as usize] == expected {
            search.found = true;
            return 0;
        }
        1
    }
    let mut search = Search { pid, found: false };
    unsafe { EnumWindows(Some(visit), &mut search as *mut _ as LPARAM) };
    search.found
}

/// Opens settings.json in Notepad using the existing shell fallback.
fn open_settings_notepad() {
    let (operation, app, path) = (
        wide("open"),
        wide("notepad.exe"),
        wide(&settings::path().display().to_string()),
    );
    unsafe {
        ShellExecuteW(
            0,
            operation.as_ptr(),
            app.as_ptr(),
            path.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
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
            refresh_idle();
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

/// A choice in the right-click panel picked: the option now selected (the panel stays open).
fn pick(id: usize, option: usize) -> usize {
    match core::route(id) {
        Route::Content(id) => {
            st(|s| s.flipping = true);
            let now = call(|c, ctx| c.menu_choice(id, option, ctx)).unwrap_or(option);
            st(|s| s.flipping = false);
            now
        }
        _ => option,
    }
}

fn read_bool(section: &str, key: &str, default: bool) -> bool {
    settings::section(section)
        .and_then(|v| v.get(key)?.as_bool())
        .unwrap_or(default)
}

fn read_highlight(section: &str) -> Highlight {
    use serde_json::Value;
    let values = settings::section(section).unwrap_or(Value::Null);
    let mode = match values.get(HIGHLIGHT_KEY).and_then(Value::as_str) {
        Some("custom") => HighlightMode::Custom,
        Some("off") => HighlightMode::Off,
        _ => HighlightMode::Auto,
    };
    let color = values.get(HIGHLIGHT_COLOR_KEY).and_then(Value::as_str)
        .and_then(|s| u32::from_str_radix(s.trim_start_matches('#'), 16).ok())
        .filter(|c| *c <= 0xFFFFFF).unwrap_or(0x0078D4);
    let strength = values.get(HIGHLIGHT_STRENGTH_KEY).and_then(Value::as_f64)
        .unwrap_or(1.0).clamp(0.0, 3.0) as f32;
    Highlight { mode, rgb: color, strength }
}

fn save_bool(section: &str, key: &str, on: bool) -> std::io::Result<()> {
    use serde_json::{Map, Value};
    settings::update(|root| {
        if !root.is_object() {
            *root = Value::Object(Map::new());
        }
        let sec = root.as_object_mut().map(|o| {
            o.entry(section)
                .or_insert_with(|| Value::Object(Map::new()))
        });
        if let Some(sec) = sec {
            if !sec.is_object() {
                *sec = Value::Object(Map::new());
            }
            sec[key] = Value::Bool(on);
        }
    })
}

fn quit() {
    let Some((main, already)) = st(|s| (s.main, std::mem::replace(&mut s.quitting, true))) else {
        return;
    };
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
    Set {
        main: HWND,
        icon: HICON,
        tip: String,
        add: bool,
        old: HICON,
    },
}

/// Adds, updates or removes the tray icon: shown whenever the strip isn't (`core::tray_shown`),
/// never while quitting. `redo`: draw the logo again (theme, scaling); `re_add`: Explorer
/// restarted (the tray lost it). The tooltip is compared and changed only when it changed.
fn tray_sync(redo: bool, re_add: bool) {
    let size = tray::size();
    if redo {
        if let Some(old) = st(|s| {
            let icon = tray::draw(s.content.as_mut(), &s.look, size);
            s.has_tray = icon.is_some();
            s.tray_size = size;
            std::mem::replace(&mut s.tray_icon, icon.unwrap_or(0))
        }) {
            unsafe { tray::destroy(old) };
        }
        refresh_idle();
    }
    let todo = st(|s| {
        let want = !s.quitting && core::tray_shown(s.core.show, s.content.visible(), s.has_tray);
        if !want {
            if !s.tray_on {
                return None;
            }
            s.tray_on = false;
            s.tray_tip.clear();
            return Some(TrayDo::Remove(
                s.main,
                std::mem::replace(&mut s.tray_icon, 0),
            ));
        }
        let add = re_add || !s.tray_on;
        let mut changed = add || redo; // a redrawn logo must reach the tray
        let mut old = 0;
        if !redo && (s.tray_icon == 0 || s.tray_size != size) {
            if let Some(icon) = tray::draw(s.content.as_mut(), &s.look, size) {
                s.has_tray = true;
                old = std::mem::replace(&mut s.tray_icon, icon);
                s.tray_size = size;
                changed = true;
            } else {
                s.has_tray = false;
            }
        }
        let idle = core::idle(s.core.show, s.has_tray, s.panel != 0);
        let tip = core::tray_tip(s.content.tray_tip(), s.spec.title, idle, s.content.live_while_idle());
        if tip != s.tray_tip {
            s.tray_tip = tip.clone();
            changed = true;
        }
        s.tray_on = true;
        changed.then_some(TrayDo::Set {
            main: s.main,
            icon: s.tray_icon,
            tip,
            add,
            old,
        })
    })
    .flatten();
    unsafe {
        match todo {
            Some(TrayDo::Remove(main, icon)) => {
                tray::remove(main);
                tray::destroy(icon);
                log("tray icon removed");
            }
            Some(TrayDo::Set {
                main,
                icon,
                tip,
                add,
                old,
            }) => {
                tray::set(main, icon, &tip, add);
                tray::destroy(old);
                if add {
                    log("tray icon added");
                }
            }
            None => {}
        }
    }
    refresh_idle();
}

// ---------------------------------------------------------------- the strip window

/// The strip as a taskbar child, or None (logged; floating may follow).
fn attach(tb: HWND) -> Option<HWND> {
    let (class, title) = st(|s| (wide(s.spec.class), wide(s.spec.title)))?;
    let h = unsafe {
        // visible at 1×1 and empty: in the taskbar's list of children (dock::slot) from the start
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_NOACTIVATE,
            class.as_ptr(),
            title.as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            1,
            1,
            tb,
            0,
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    if h == 0 {
        log(&format!(
            "attaching the strip to the taskbar failed (error {})",
            unsafe { GetLastError() }
        ));
        st(|s| s.attach_failed = Some(Instant::now()));
        return None;
    }
    Some(h)
}

/// Creates the strip: a taskbar child if possible, else (attaching failed, no taskbar for
/// [`FLOAT_AFTER`], or `force_float`) a floating topmost window, if the Spec allows.
fn create_strip() {
    let Some((ok, float, force, failed)) = st(|s| {
        (
            s.strip == 0
                && !s.quitting
                && core::shown(s.core.show, s.content.visible(), s.has_tray),
            s.spec.float,
            s.spec.force_float,
            s.attach_failed,
        )
    }) else {
        return;
    };
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
    let Some((class, title)) = st(|s| (wide(s.spec.class), wide(s.spec.title))) else {
        return;
    };
    let h = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            0,
            0,
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    if h == 0 {
        log(&format!(
            "creating the floating strip failed (error {})",
            unsafe { GetLastError() }
        ));
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
        s.core.drawn_target = None;
        s.core.width = core::WidthHold::default();
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
    let Some(h) = st(|s| std::mem::replace(&mut s.strip, 0)).filter(|h| *h != 0) else {
        return;
    };
    for t in [T_GLIDE, T_ZOOM, T_RESIZE, T_FEEL, T_APPEAR] {
        timer(t, false);
    }
    st(|s| {
        s.core.motion = Motion::default();
        s.core.hover = false;
        s.leaving = false;
        s.appear = crate::anim::Appear::hidden(Instant::now());
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
    }) else {
        return;
    };
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
    let Some((strip, floating, force, failed, fs_hidden)) = st(|s| {
        (
            s.strip,
            s.floating,
            s.spec.force_float,
            s.attach_failed,
            s.fs_hidden,
        )
    }) else {
        return;
    };
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
    follow_settings(false);
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
fn follow_settings(force: bool) {
    let m = settings::mtime();
    if let Some(Some(section)) = st(|s| (force || s.settings_mtime != m).then_some(s.spec.section))
    {
        let (compact, show, highlight) = (
            read_bool(section, COMPACT_KEY, false),
            read_bool(section, SHOW_KEY, true),
            read_highlight(section),
        );
        st(|s| {
            s.settings_mtime = m;
            s.core.compact = compact;
            s.core.show = show;
            s.highlight = highlight;
        });
        call(|c, ctx| c.settings_changed(ctx));
        refresh_idle();
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
    }) else {
        return;
    };
    if changed {
        call(|c, ctx| c.look_changed(ctx));
        tray_sync(true, false); // the logo follows the theme
        if panel != 0 {
            unsafe { InvalidateRect(panel, std::ptr::null(), 0) };
        }
    }
}

fn update_hooks() {
    let Some(mut hooks) = st(|s| std::mem::take(&mut s.hooks)) else {
        return;
    };
    unsafe { hooks.update(dock::taskbar(), Some(on_win_event)) };
    st(|s| s.hooks = hooks);
}

/// A neighbour (tray, another widget) changed: reposition right away instead of waiting for the
/// watch timer.
unsafe extern "system" fn on_win_event(
    _: isize,
    _event: u32,
    hwnd: isize,
    id_object: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    let tb = dock::taskbar();
    let Some((strip, floating)) = st(|s| (s.strip, s.floating)) else {
        return;
    };
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

fn frame(look: &Look, highlight: Highlight, height: i32, scale: f32, compact: bool) -> Frame<'_> {
    Frame {
        look,
        highlight,
        height,
        scale,
        compact,
        fit_width: None,
    }
}

/// Measures, draws and places the strip; creates it when it's missing and wanted, destroys it
/// when [`Content::visible`] says no.
fn redraw() {
    let Some((strip, visible, quitting, floating, fs_hidden, flipping)) = st(|s| {
        (
            s.strip,
            core::shown(s.core.show, s.content.visible(), s.has_tray),
            s.quitting,
            s.floating,
            s.fs_hidden,
            s.flipping,
        )
    }) else {
        return;
    };
    if quitting {
        return;
    }
    if !visible {
        if strip != 0 && st(|s| !std::mem::replace(&mut s.leaving, true)) == Some(true) {
            if flipping {
                // a switch in the open right-click panel did this (Show on taskbar): that panel
                // stays open, as switches do
                panel_closed(false);
            } else {
                close_panels(); // hidden by state (discord, media): every panel closes
            }
            // fades out where it is, then goes (appear_step); the others close up after
            st(|s| s.appear.go(0.0, Instant::now(), s.look.glide_ms));
            appear_step();
        }
        return;
    }
    if strip == 0 {
        return create_strip(); // draws it once made
    }
    // fading out and wanted again: fades back in from where it is
    if st(|s| std::mem::replace(&mut s.leaving, false)) == Some(true) {
        st(|s| s.appear.go(1.0, Instant::now(), s.look.glide_ms));
        timer(T_APPEAR, true);
    }
    unsafe {
        if IsWindow(strip) == 0 {
            st(|s| s.strip = 0);
            return;
        }
        let tb = if floating {
            dock::taskbar()
        } else {
            GetParent(strip)
        };
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
        // its widths for the dock's fit: in the chosen mode (a gliding width held steady first),
        // and compact (unzoomed)
        let Some((size, raw, compact_w, side, zoom, dragging, look, stretch, glide)) = st(|s| {
            let f = frame(&s.look, s.highlight, height, dpi, s.core.compact);
            let chosen = s.content.measure(&f, s.core.compact);
            let compact = s.content.measure(&f, true);
            let held = if s.spec.glide_width {
                let key = (s.core.compact, height, (dpi * 1000.0).round() as i32);
                s.core
                    .width
                    .update(key, chosen, (core::WIDTH_SLACK * dpi).round() as i32)
            } else {
                chosen
            };
            let size = core::size(held, compact, &s.spec);
            (
                size,
                chosen,
                compact,
                s.content.side(),
                s.core.motion.zoom(),
                s.core.motion.dragging(),
                s.look.clone(),
                s.spec.stretch,
                s.spec.glide_width,
            )
        }) else {
            return;
        };
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
        let slot = if right {
            dock::slot(tb, strip, size, dock::tray_boundary(tb, dpi), gap)
        } else {
            None
        };
        let fit = if right {
            slot.map(|s| s.fit)
        } else {
            Some(Fit::Full)
        };
        if let Some(f) = fit.filter(|_| !dragging) {
            apply_fit(strip, f, false);
        }
        if st(|s| s.core.fit.hidden()) != Some(false) {
            return;
        }
        // the width it's headed for: compact (chosen, or for room), crowded, or the content's held
        // width. A compact switch animates it (the full content drawn cut off meanwhile), and so
        // does any change with Spec::glide_width
        let Some((compact, narrowed)) = st(|s| s.core.mode(stretch, size.min)) else {
            return;
        };
        let (natural_w, target) = if compact {
            (compact_w, compact_w)
        } else {
            (raw, narrowed.map_or(size.full, |n| n.min(size.full)))
        };
        let resize_w = start_resize(strip, compact, target, glide);
        let shown_compact = compact && resize_w.is_none();
        let fit_w = core::fit_width(resize_w, target, natural_w, zoom);
        let Some(Some((mut cv, feel, cover, hover, dragging_now, hot, tint))) = st(|s| {
            let mut f = frame(&s.look, s.highlight, height, dpi * zoom, shown_compact);
            // crowded, held or animating: the width it's drawn at (a Content that can narrow or
            // widen lays out to it, so its right end isn't cut off)
            f.fit_width = fit_w;
            let natural = s.content.measure(&f, shown_compact).max(1);
            let mut cv = Canvas::new(natural, height)?;
            s.content.draw(&mut cv, &f);
            let hot = s.core.hot;
            // a brand tint for the strip (and the hovered button's own)
            // at the shared levels (Look::tint) for the state the Content says it's in
            let strip_auto = s.content.tint(Hit::Strip, &s.look);
            let strip_auto_rgb = strip_auto.map(|t| t.rgb);
            let tint_allowed = s.content.tint_allowed();
            let resolved = tint_allowed.then(|| s.highlight.apply(strip_auto, strip_auto_rgb)).flatten();
            let shade = |t: Tint| core::Shade::of(t, &s.look.tint);
            let tint = resolved.map(|t| {
                (
                    shade(t),
                    matches!(hot, Hit::Button(..))
                        .then(|| tint_allowed.then(|| s.highlight.apply(s.content.tint(hot, &s.look), strip_auto_rgb)).flatten())
                        .flatten()
                        .map(shade),
                )
            });
            Some((
                cv,
                s.core.feel.value(),
                s.content.cover(),
                s.core.hover,
                s.core.motion.dragging(),
                hot,
                tint,
            ))
        }) else {
            return;
        };
        let natural = cv.w;
        // the cover's hover / press feel, then the cut-off, then the background under it all
        if let Some(b) = cover {
            draw::grow_pixels(cv.pixels(), natural, b, feel);
        }
        let width = fit_w.map_or(natural, |c| c.clamp(1, natural));
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
                for (b, rgb, alpha) in core::tints(
                    strip_tint,
                    button_tint,
                    hover,
                    dragging_now,
                    hot,
                    width,
                    height,
                    inset,
                ) {
                    draw::tint_box(
                        &mut px,
                        width,
                        b,
                        (rgb >> 16 & 0xff, rgb >> 8 & 0xff, rgb & 0xff),
                        alpha,
                        radius,
                    );
                }
            }
        }
        if floating {
            // over other windows: a backdrop in the panels' background keeps it readable, in the
            // highlight's shape, under everything
            let bg = Theme::new(&look).color.bg.tuple();
            draw::tint_box(
                &mut px,
                width,
                (0, inset, width, height - inset),
                bg,
                FLOAT_BACKDROP,
                radius,
            );
        }
        drop(cv);
        let Some(mut out) = Canvas::new(width, height) else {
            return;
        };
        out.pixels().copy_from_slice(&px);
        let before = dock::rect(strip);
        // a compact switch running: the others place themselves by this frame's width (published
        // before the window takes it, so the move it causes finds it); cleared on the last frame
        let drawn_cleared =
            right && dock::publish_drawn(strip, resize_w.map(|_| width)) && resize_w.is_none();
        let sz = SIZE {
            cx: width,
            cy: height,
        };
        let src = POINT { x: 0, y: 0 };
        // a new strip fades in (appear_step), from its first drawing
        let Some((alpha, appearing)) = st(|s| {
            let now = Instant::now();
            if !s.placed {
                s.appear = crate::anim::Appear::hidden(now);
                s.appear.go(1.0, now, s.look.glide_ms);
            }
            (s.appear.value(now), s.appear.running(now))
        }) else {
            return;
        };
        if appearing {
            timer(T_APPEAR, true);
        }
        let blend = blend_at(alpha);
        let settled = st(|s| s.core.motion.settled()).unwrap_or(true);
        // docked against the tray: a new width keeps the right edge, moved in the same step
        let dst = if right && settled && !dragging && IsWindowVisible(strip) != 0 {
            strip::right_anchored(strip, before, width)
        } else {
            None
        };
        UpdateLayeredWindow(
            strip,
            0,
            dst.as_ref().map_or(std::ptr::null(), |p| p as *const POINT),
            &sz,
            out.dc,
            &src,
            0,
            &blend,
            ULW_ALPHA,
        );
        if drawn_cleared {
            dock::notify(strip); // the last frame may not move it: the others look once more
        }

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
        if right && settled {
            // docked against the tray: a new width keeps the right edge, not the left (a no-op
            // when UpdateLayeredWindow already moved it)
            strip::keep_right_edge(strip, tb, before);
        }
        if dock::needs_raise(strip) {
            // the taskbar's own content came over it: back on top (never on every redraw)
            SetWindowPos(
                strip,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        if slot.is_some_and(|s| s.follow) {
            follow(strip, tb, target(width)); // a neighbour is changing width: keep flush with it
        } else {
            move_to(strip, tb, target(width)); // every move slides (strip.rs rules)
        }
    }
}

fn blend_at(alpha: f32) -> BLENDFUNCTION {
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: a,
        AlphaFormat: AC_SRC_ALPHA as u8,
    }
}

/// One frame of the strip fading in or out: only its opacity changes (the picture stays, so a
/// Content with nothing to show any more needn't draw). At the end of a fade-out it's removed.
fn appear_step() {
    let Some((strip, alpha, running, leaving)) = st(|s| {
        let now = Instant::now();
        (
            s.strip,
            s.appear.value(now),
            s.appear.running(now),
            s.leaving,
        )
    }) else {
        return;
    };
    timer(T_APPEAR, running && strip != 0);
    if strip == 0 {
        return;
    }
    if leaving && !running {
        remove_strip();
        return log("switched off or nothing to show: strip removed");
    }
    let blend = blend_at(alpha);
    unsafe {
        UpdateLayeredWindow(
            strip,
            0,
            std::ptr::null(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            &blend,
            ULW_ALPHA,
        );
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

/// Compact mode switched since the last drawing, or (`glide`: [`Spec::glide_width`]) the width
/// it's headed for changed: animate the width from the current one to `target`, for a shown
/// strip ([`core::animate_width`]). The width to draw at while that runs.
fn start_resize(strip: HWND, compact: bool, target: i32, glide: bool) -> Option<i32> {
    let Some((switched, prev, running)) = st(|s| {
        let switched = s
            .core
            .drawn_compact
            .replace(compact)
            .is_some_and(|c| c != compact);
        (
            switched,
            s.core.drawn_target.replace(target),
            s.core.motion.resize_width().is_some(),
        )
    }) else {
        return None;
    };
    if core::animate_width(switched, prev, target, glide, running)
        && unsafe { IsWindowVisible(strip) } != 0
    {
        let from = st(|s| s.core.motion.resize_width())
            .flatten()
            .unwrap_or_else(|| width_of(strip));
        if st(|s| s.core.motion.resize(from, target, &s.look, Instant::now())) == Some(true) {
            timer(T_RESIZE, true);
        }
    }
    st(|s| s.core.motion.resize_width()).flatten()
}

/// The room next to the taskbar's buttons changed (dock::fit): hide or show the strip, and with
/// `redraw` also redraw it (compact, narrower or full).
fn apply_fit(strip: HWND, fit: Fit, redraw_it: bool) -> FitChange {
    let Some(change) = st(|s| s.core.fit.update(fit)) else {
        return FitChange::None;
    };
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
    let Some(look) = st(|s| s.look.clone()) else {
        return;
    };
    strip::move_strip(strip, tb, x.min(max_x(tb, width_of(strip), &look)), false);
}

/// Heads for x: a slide or a jump (rules in `strip`).
fn move_to(strip: HWND, tb: HWND, x: i32) {
    let cur = unsafe { dock::rect(strip).left };
    match st(|s| s.core.motion.glide_to(cur, x, &s.look, Instant::now())) {
        Some(Glide::Jump(x)) => {
            timer(T_GLIDE, false);
            unsafe { put(strip, tb, x) };
        }
        Some(Glide::Slide) => timer(T_GLIDE, true),
        _ => {}
    }
}

/// Goes straight to x, either way: a neighbour is changing width ([`dock::Slot::follow`]).
fn follow(strip: HWND, tb: HWND, x: i32) {
    let cur = unsafe { dock::rect(strip).left };
    if let Some(Glide::Jump(x)) = st(|s| s.core.motion.follow(cur, x)) {
        timer(T_GLIDE, false);
        unsafe { put(strip, tb, x) };
    }
}

/// Recomputes only the position (no redraw): a neighbour moved, or a drop. Not while pressed.
fn reposition(always_slide: bool) {
    let Some((strip, floating, pressed, side, look)) = st(|s| {
        (
            s.strip,
            s.floating,
            s.core.motion.pressed(),
            s.content.side(),
            s.look.clone(),
        )
    }) else {
        return;
    };
    if strip == 0 || floating || pressed {
        return;
    }
    unsafe {
        let tb = GetParent(strip);
        let (scale, gap) = scale_gap(tb, &look);
        let width = width_of(strip);
        let (x, follows) = if side == Side::Right {
            let size = dock_size(strip, width);
            let Some(slot) = dock::slot(tb, strip, size, dock::tray_boundary(tb, scale), gap)
            else {
                return;
            };
            // the room changed: redrawn compact or full, or hidden
            if apply_fit(strip, slot.fit, true) != FitChange::None {
                return;
            }
            (slot.x_for(width), slot.follow)
        } else {
            (place::left_side(tb, strip, width, gap, scale), false)
        };
        if IsWindowVisible(strip) == 0 {
            return;
        }
        if follows && !always_slide {
            follow(strip, tb, x); // a neighbour is changing width: keep flush with it
        } else {
            move_to(strip, tb, x);
        }
    }
}

/// The widths this strip published for the fit; its window width if none yet.
unsafe fn dock_size(strip: HWND, width: i32) -> dock::Size {
    dock::children(GetParent(strip))
        .into_iter()
        .find(|c| c.hwnd == strip)
        .map_or(dock::Size::fixed(width), |c| c.size)
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
