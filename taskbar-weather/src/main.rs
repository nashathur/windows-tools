#![windows_subsystem = "windows"]

mod config;
mod knmi;
mod render;
mod weather;
mod fetch;
mod flyout;
mod label;
mod menu;
mod tray;

use fetch::*;
use flyout::*;
use label::*;
use menu::*;
use tray::*;

use config::Config;
use render::{pcwstr, wide, LabelContent};
use std::cell::RefCell;
use std::ffi::c_void;
use std::time::{Instant, SystemTime};
use weather::{deg, describe, Location, Weather};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use taskbar_dock::panel::{Action, Fade, PanelBehaviour};
use taskbar_dock::style::Look;
use taskbar_dock::{dock, strip, system};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const WM_DATA: u32 = WM_APP + 2;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_REFRESH: usize = 1;
const TIMER_WATCH: usize = 2;
const TIMER_GLIDE: usize = 3;
const TIMER_ZOOM: usize = 4;
const TIMER_HOVER: usize = 5;
const TIMER_FADE: usize = 6;
/// The label's width animating after compact mode was switched.
const TIMER_RESIZE: usize = 7;

const ID_REFRESH: usize = 100;
const ID_LABEL: usize = 101;
const ID_FAHRENHEIT: usize = 103;
const ID_STARTUP: usize = 104;
const ID_SETTINGS: usize = 105;
const ID_EXIT: usize = 106;
const ID_COMPACT: usize = 107;

const MAIN_CLASS: PCWSTR = w!("TaskbarWeatherMain");
const LABEL_CLASS: PCWSTR = w!("TaskbarWeatherLabel");
const POPUP_CLASS: PCWSTR = w!("TaskbarWeatherPopup");

struct App {
    main: HWND,
    label: Option<HWND>,
    popup: Option<HWND>,
    cfg: Config,
    cfg_mtime: Option<SystemTime>,
    loc: Option<(String, Location)>,
    station: Option<(String, knmi::Station)>,
    weather: Option<Weather>,
    error: Option<String>,
    fetching: bool,
    icon: Option<HICON>,
    /// Shared style + Windows settings (theme, animations, hover time, time format, …), kept current.
    look: Look,
    hover: bool,
    taskbar_created: u32,
    /// WinEvent hooks on each process owning taskbar child windows (Explorer, other widgets).
    hooks: dock::Hooks,
    /// Gliding, dragging and zoom of the label (the shared behaviour; x in taskbar client coordinates).
    motion: strip::Motion,
    /// Compact mode as last drawn: a switch animates the width.
    drawn_compact: Option<bool>,
    /// Room next to the taskbar's buttons (dock::fit): full, compact or hidden.
    fit: dock::FitState,
    /// The label's widths for the fit, as last measured.
    size: Option<dock::Size>,
    /// When the flyout opens and closes (the shared behaviour for every widget).
    panel: PanelBehaviour,
    /// Flyout fade-in.
    fade: Fade,
}

/// Appends a line to taskbar-weather.log in the settings folder.
pub(crate) fn log(line: &str) {
    use std::io::Write;
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let path = taskbar_dock::settings::dir().join("taskbar-weather.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {line}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
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

struct Fetched {
    key: String,
    loc: Option<Location>,
    station: Option<(String, knmi::Station)>,
    result: Result<Weather, String>,
}

fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).unwrap_or_default().into() }
}

fn config_mtime() -> Option<SystemTime> {
    std::fs::metadata(Config::path()).and_then(|m| m.modified()).ok()
}

fn startup_enabled() -> bool {
    taskbar_dock::startup::is_on("weather")
}

/// Puts the widget on the shared startup list (started at sign-in by taskbar-dock-start).
fn set_startup(on: bool) {
    let _ = taskbar_dock::startup::set("weather", on);
}

// ---------------------------------------------------------------- window procedures

extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAY => {
                match (lp.0 as u32) & 0xFFFF {
                    WM_LBUTTONUP => toggle_popup(),
                    WM_RBUTTONUP => show_menu(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DATA => {
                on_data(Box::from_raw(lp.0 as *mut Fetched));
                LRESULT(0)
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_REFRESH => start_fetch(),
                    TIMER_WATCH => {
                        watch_config();
                        watch_label();
                    }
                    TIMER_GLIDE => glide_step(),
                    TIMER_ZOOM => zoom_step(),
                    TIMER_HOVER => hover_step(),
                    TIMER_FADE => fade_step(),
                    TIMER_RESIZE => resize_step(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_SETTINGCHANGE | WM_DISPLAYCHANGE | WM_DPICHANGED => {
                // theme, scaling, time format or another Windows setting changed: follow it now
                refresh_look();
                update_tray(false);
                redraw_label();
                if let Some(Some(p)) = with(|a| a.popup) {
                    let _ = InvalidateRect(p, None, false);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_DESTROY => {
                destroy_label();
                remove_tray(hwnd);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => {
                // Explorer restarted: the tray icon and the taskbar window are new.
                if with(|a| a.taskbar_created) == Some(msg) && msg != 0 {
                    update_tray(true);
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
                mouse_down(hwnd);
                SetCapture(hwnd);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                mouse_up(hwnd, true);
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                mouse_up(hwnd, false);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                show_menu();
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if with(|a| !std::mem::replace(&mut a.hover, true)) == Some(true) {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                    redraw_label();
                    hover_started(); // the flyout opens if the mouse stays a moment
                }
                mouse_drag(hwnd);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                with(|a| a.hover = false);
                // the right-click menu also "leaves" (it takes the mouse); only a real leave,
                // with the pointer outside the label, ends the block on hover-opening
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
                let painted = with(|a| {
                    let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
                    render::paint_popup(hwnd, a.weather.as_ref(), a.error.as_deref(), scale, &a.look);
                });
                if painted.is_none() {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
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

fn main() {
    unsafe {
        // One instance only.
        let _mutex = CreateMutexW(None, true, w!("Local\\TaskbarWeather.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        // a crash closes the widget at once (panic = abort): write why to the log first
        taskbar_dock::crash::log_panics(log);

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

        let Ok(main) = CreateWindowExW(WINDOW_EX_STYLE(0), MAIN_CLASS, w!("Taskbar Weather"), WS_OVERLAPPED,
            0, 0, 0, 0, None, None, hinst, None) else { return };

        let cfg = Config::load();
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                main,
                label: None,
                popup: None,
                cfg_mtime: config_mtime(),
                cfg,
                loc: None,
                station: None,
                weather: None,
                error: None,
                fetching: false,
                icon: None,
                look: Look::current(),
                hover: false,
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

        update_tray(true);
        create_label();
        update_hooks();
        start_fetch();
        SetTimer(main, TIMER_WATCH, 1000, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
