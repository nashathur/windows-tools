#![windows_subsystem = "windows"]

mod auth;
mod avatars;
mod config;
mod flyout;
mod label;
mod menu;
mod open;
mod render;
mod rpc;
mod voice;

use flyout::*;
use label::*;
use menu::*;

use config::Config;
use render::{pcwstr, wide, Button};
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::mpsc::Sender;
use std::time::{Instant, SystemTime};
use taskbar_dock::panel::{Action, Fade, PanelBehaviour};
use taskbar_dock::style::Look;
use taskbar_dock::{dock, strip, system};
use voice::{Status, Voice};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// A new `Voice` from the RPC worker (lparam: Box<Voice>).
const WM_VOICE: u32 = WM_APP + 2;
/// An avatar finished loading.
const WM_AVATAR: u32 = WM_APP + 3;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_WATCH: usize = 2;
const TIMER_GLIDE: usize = 3;
const TIMER_ZOOM: usize = 4;
const TIMER_HOVER: usize = 5;
const TIMER_FADE: usize = 6;
/// The strip's width animating after compact mode was switched.
const TIMER_RESIZE: usize = 7;
/// The logo's ring pulsing (25 frames a second while unmuted in a call).
const TIMER_BEAT: usize = 8;
/// The logo growing on hover / dipping on press (anim::Feel), only while it moves.
const TIMER_FEEL: usize = 9;

const ID_MUTE: usize = 100;
const ID_DEAF: usize = 101;
const ID_LEAVE: usize = 102;
const ID_COMPACT: usize = 103;
const ID_IDLE: usize = 104;
const ID_STARTUP: usize = 105;
const ID_SIGNIN: usize = 106;
const ID_RECONNECT: usize = 107;
const ID_SETTINGS: usize = 108;
const ID_EXIT: usize = 109;

const MAIN_CLASS: PCWSTR = w!("TaskbarDiscordMain");
const LABEL_CLASS: PCWSTR = w!("TaskbarDiscordStrip");
const POPUP_CLASS: PCWSTR = w!("TaskbarDiscordPanel");

struct App {
    main: HWND,
    label: Option<HWND>,
    popup: Option<HWND>,
    cfg: Config,
    cfg_mtime: Option<SystemTime>,
    /// The call as last reported by the RPC worker.
    voice: Voice,
    /// Who has the strip's avatar places.
    slots: voice::Slots,
    rpc: Sender<rpc::Cmd>,
    /// Shared style + Windows settings, kept current.
    look: Look,
    hover: bool,
    /// The strip's button under the mouse (Mute, Deafen, Leave).
    hot: Option<Button>,
    /// What the mouse was pressed on: a strip button, the server logo, or the strip itself.
    pressed: Option<Press>,
    /// The mouse is over the server logo; its hover / press size factor.
    on_logo: bool,
    feel: taskbar_dock::anim::Feel,
    feeling: bool,
    /// The strip as last drawn (its buttons, for hit tests).
    strip: Option<render::StripLayout>,
    /// Since when the ring has been pulsing (from dim), and whether its timer runs.
    ring_since: Option<Instant>,
    beating: bool,
    /// The media strip's text column (px at 100%): this strip is as wide as that one.
    media_w: f32,
    /// The panel button under the mouse.
    panel_hover: Option<Button>,
    taskbar_created: u32,
    hooks: dock::Hooks,
    /// Gliding, dragging and zoom of the strip (x in taskbar client coordinates).
    motion: strip::Motion,
    /// Compact mode as last drawn: a switch animates the width.
    drawn_compact: Option<bool>,
    /// Room next to the taskbar's buttons (dock::fit): full, compact or hidden.
    fit: dock::FitState,
    size: Option<dock::Size>,
    panel: PanelBehaviour,
    fade: Fade,
}

/// Appends a line to taskbar-discord.log in the settings folder. Never tokens or secrets.
pub(crate) fn log(line: &str) {
    use std::io::Write;
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let path = taskbar_dock::settings::dir().join("taskbar-discord.log");
    // one write per line: the RPC worker logs too, and appends of whole lines don't interleave
    let text = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} {line}\r\n", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Short, non-reentrant access to the app state. Never call into Win32 functions that send
/// messages to our own windows (CreateWindow, DestroyWindow, SetWindowPos, ...) inside `f`.
fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).unwrap_or_default().into() }
}

fn config_mtime() -> Option<SystemTime> {
    std::fs::metadata(Config::path()).and_then(|m| m.modified()).ok()
}

fn startup_enabled() -> bool {
    taskbar_dock::startup::is_on("discord")
}

fn set_startup(on: bool) {
    let _ = taskbar_dock::startup::set("discord", on);
}

/// Sends a command to the RPC worker.
pub(crate) fn rpc(cmd: rpc::Cmd) {
    with(|a| {
        let _ = a.rpc.send(cmd);
    });
}

/// A new state from Discord: show or hide the strip, redraw, resize the panel.
fn on_voice(v: Voice) {
    let Some((was_call, now_call)) = with(|a| {
        let r = (a.voice.in_call(), v.in_call());
        if a.voice.status != v.status {
            log(&format!("status: {}", v.status.text()));
        }
        a.slots.update(v.channel.as_ref().map_or(&[], |c| &c.members), a.cfg.max_avatars);
        a.voice = v;
        r
    }) else { return };
    if was_call != now_call {
        log(if now_call { "joined a voice channel" } else { "left the voice channel" });
    }
    update_visibility();
    redraw_label();
    refresh_popup();
}

// ---------------------------------------------------------------- window procedures

extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_VOICE => {
                on_voice(*Box::from_raw(lp.0 as *mut Voice));
                LRESULT(0)
            }
            WM_AVATAR => {
                redraw_label();
                if let Some(Some(p)) = with(|a| a.popup) {
                    let _ = InvalidateRect(p, None, false);
                }
                LRESULT(0)
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_WATCH => {
                        watch_config();
                        watch_label();
                    }
                    TIMER_GLIDE => glide_step(),
                    TIMER_ZOOM => zoom_step(),
                    TIMER_HOVER => hover_step(),
                    TIMER_FADE => fade_step(),
                    TIMER_RESIZE => resize_step(),
                    TIMER_BEAT => redraw_label(),
                    TIMER_FEEL => feel_step(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_SETTINGCHANGE | WM_DISPLAYCHANGE | WM_DPICHANGED => {
                refresh_look();
                redraw_label();
                if let Some(Some(p)) = with(|a| a.popup) {
                    let _ = InvalidateRect(p, None, false);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_DESTROY => {
                destroy_label();
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => {
                // Explorer restarted: the taskbar window is new
                if with(|a| a.taskbar_created) == Some(msg) && msg != 0 {
                    taskbar_restarted();
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
    }
}

extern "system" fn label_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONDOWN => {
                let p = { let (x, y) = strip::client_pos(lp.0); press_at(x, y) };
                with(|a| a.pressed = Some(p));
                feel_update();
                mouse_down(hwnd);
                SetCapture(hwnd);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                mouse_up(hwnd, Some(strip::client_pos(lp.0)));
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                mouse_up(hwnd, None);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                show_menu();
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = strip::client_pos(lp.0);
                let b = button_at(x, y);
                let logo = press_at(x, y) == Press::Logo;
                with(|a| a.on_logo = logo);
                if let Some((first, changed)) = with(|a| (!std::mem::replace(&mut a.hover, true), std::mem::replace(&mut a.hot, b) != b)) {
                    if first {
                        let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        let _ = TrackMouseEvent(&mut tme);
                        hover_started(); // the panel opens if the mouse stays a moment
                    }
                    if first || changed {
                        redraw_label();
                    }
                }
                mouse_drag(hwnd);
                feel_update(); // also when the strip came to rest under a still-hovered logo
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                with(|a| {
                    a.hover = false;
                    a.hot = None;
                    a.on_logo = false;
                });
                feel_update();
                // the right-click panel also "leaves"; only a real leave ends the hover block
                let (mut rc, mut pt) = (RECT::default(), POINT::default());
                let _ = GetWindowRect(hwnd, &mut rc);
                let _ = GetCursorPos(&mut pt);
                let outside = !contains(&rc, pt);
                with(|a| a.panel.mouse_left(outside));
                redraw_label();
                LRESULT(0)
            }
            WM_DESTROY => {
                with(|a| if a.label == Some(hwnd) { a.label = None });
                LRESULT(0)
            }
            // another dock widget opened its panel: close ours (one panel at a time)
            _ if msg == dock::close_panel_message() => {
                popup_closed(false);
                taskbar_dock::menu_panel::close();
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

extern "system" fn popup_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
                let painted = with(|a| {
                    let main = a.main.0 as usize;
                    let avatar = move |m: &voice::Member| avatars::get(m, move || notify_avatar(main));
                    let logo = guild_logo(&a.voice, main);
                    render::paint_panel(hwnd, &a.voice, &avatar, logo.as_deref(), a.panel_hover, scale, &a.look);
                });
                if painted.is_none() {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEMOVE => {
                panel_mouse(hwnd, lp);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                if with(|a| a.panel_hover.take().is_some()) == Some(true) {
                    let _ = InvalidateRect(hwnd, None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                panel_click(hwnd, lp);
                LRESULT(0)
            }
            WM_ACTIVATE => {
                // only if still ours: closing it ourselves also deactivates it
                if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE && with(|a| a.popup == Some(hwnd)) == Some(true) {
                    popup_closed(true);
                }
                LRESULT(0)
            }
            WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
                popup_closed(false);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                show_menu();
                LRESULT(0)
            }
            WM_DESTROY => {
                with(|a| if a.popup == Some(hwnd) { a.popup = None });
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Tells the UI thread an avatar is ready (from the loader thread).
fn notify_avatar(main: usize) {
    unsafe { let _ = PostMessageW(HWND(main as *mut c_void), WM_AVATAR, WPARAM(0), LPARAM(0)); }
}

fn main() {
    unsafe {
        // one instance only
        let _mutex = CreateMutexW(None, true, w!("Local\\TaskbarDiscord.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        // a crash closes the widget at once (panic = abort): write why to the log first
        taskbar_dock::crash::log_panics(log);
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let hinst = hinstance();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let classes: [(PCWSTR, WNDPROC, WNDCLASS_STYLES); 3] = [
            (MAIN_CLASS, Some(main_proc), WNDCLASS_STYLES(0)),
            (LABEL_CLASS, Some(label_proc), WNDCLASS_STYLES(0)),
            (POPUP_CLASS, Some(popup_proc), CS_DROPSHADOW),
        ];
        for (name, proc_, style) in classes {
            let wc = WNDCLASSW { lpfnWndProc: proc_, hInstance: hinst, lpszClassName: name, hCursor: cursor, style, ..Default::default() };
            RegisterClassW(&wc);
        }

        let Ok(main) = CreateWindowExW(WINDOW_EX_STYLE(0), MAIN_CLASS, w!("Taskbar Discord"), WS_OVERLAPPED,
            0, 0, 0, 0, None, None, hinst, None) else { return };

        let cfg = Config::load();
        let main_raw = main.0 as usize;
        let rpc = rpc::start(cfg.connection_key(), Box::new(move |v: Voice| {
            let msg = Box::into_raw(Box::new(v));
            if PostMessageW(HWND(main_raw as *mut c_void), WM_VOICE, WPARAM(0), LPARAM(msg as isize)).is_err() {
                drop(Box::from_raw(msg));
            }
        }));
        let status = if cfg.configured() || std::env::var_os("TASKBAR_DISCORD_TEST").is_some() { Status::Connecting } else { Status::NotConfigured };
        log("started");
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                main,
                label: None,
                popup: None,
                cfg_mtime: config_mtime(),
                cfg,
                voice: Voice { status, ..Default::default() },
                slots: voice::Slots::default(),
                rpc,
                look: Look::current(),
                hover: false,
                hot: None,
                pressed: None,
                on_logo: false,
                feel: taskbar_dock::anim::Feel::new(),
                feeling: false,
                strip: None,
                ring_since: None,
                beating: false,
                media_w: label::media_text_width(),
                panel_hover: None,
                taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
                hooks: dock::Hooks::default(),
                motion: strip::Motion::default(),
                drawn_compact: None,
                fit: dock::FitState::default(),
                size: None,
                panel: PanelBehaviour::default(),
                fade: Fade::default(),
            })
        });

        update_visibility();
        update_hooks();
        SetTimer(main, TIMER_WATCH, 1000, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
