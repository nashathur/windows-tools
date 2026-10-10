//! "Change stop…" window: search a stop by name, then pick line + direction at that platform.
//! Reads the national timetable kept in .\data; the stop list is loaded only while this is open.

use std::cell::{Cell, RefCell};
use std::mem::zeroed;
use std::ptr::null;
use std::sync::Mutex;
use std::thread;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::{data, data_dir, set_config, wide};

const ID_EDIT: u16 = 10;
const ID_LIST: u16 = 11;
const ID_BACK: u16 = 12;
const WM_STOPS_DONE: u32 = WM_APP + 1;
const WM_LINES_DONE: u32 = WM_APP + 2;
const MAX_SHOWN: usize = 300;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Loading,
    Stops,
    Lines,
}

struct Picker {
    hwnd: Cell<HWND>,
    edit: Cell<HWND>,
    list: Cell<HWND>,
    status: Cell<HWND>,
    back: Cell<HWND>,
    font: Cell<HFONT>,
    mode: Cell<Mode>,
    stops: RefCell<Vec<data::StopHit>>,
    shown: RefCell<Vec<usize>>, // listbox row -> index into stops
    chosen: Cell<Option<usize>>,
    lines: RefCell<Vec<data::LineOpt>>,
}

thread_local!(static P: Picker = Picker {
    hwnd: Cell::new(0),
    edit: Cell::new(0),
    list: Cell::new(0),
    status: Cell::new(0),
    back: Cell::new(0),
    font: Cell::new(0),
    mode: Cell::new(Mode::Loading),
    stops: RefCell::new(Vec::new()),
    shown: RefCell::new(Vec::new()),
    chosen: Cell::new(None),
    lines: RefCell::new(Vec::new()),
});

fn p<R>(f: impl FnOnce(&Picker) -> R) -> R {
    P.with(f)
}

// results from the background threads
static STOPS: Mutex<Option<Result<Vec<data::StopHit>, String>>> = Mutex::new(None);
static LINES: Mutex<Option<Result<Vec<data::LineOpt>, String>>> = Mutex::new(None);

pub fn hwnd() -> HWND {
    p(|p| p.hwnd.get())
}

unsafe fn set_text(h: HWND, s: &str) {
    SetWindowTextW(h, wide(s).as_ptr());
}

unsafe fn status(s: &str) {
    set_text(p(|p| p.status.get()), s);
}

unsafe fn list_set(rows: impl Iterator<Item = String>) {
    let list = p(|p| p.list.get());
    SendMessageW(list, WM_SETREDRAW, 0, 0);
    SendMessageW(list, LB_RESETCONTENT, 0, 0);
    for r in rows {
        let w = wide(&r);
        SendMessageW(list, LB_ADDSTRING, 0, w.as_ptr() as LPARAM);
    }
    SendMessageW(list, WM_SETREDRAW, 1, 0);
    InvalidateRect(list, null(), 1);
    if SendMessageW(list, LB_GETCOUNT, 0, 0) > 0 {
        SendMessageW(list, LB_SETCURSEL, 0, 0);
    }
}

pub unsafe fn open(scale: f32) {
    let existing = hwnd();
    if existing != 0 {
        SetForegroundWindow(existing);
        return;
    }
    let hinst = GetModuleHandleW(null());
    let cls = wide("BusWidgetPicker");
    let mut wc: WNDCLASSW = zeroed();
    wc.hInstance = hinst;
    wc.hCursor = LoadCursorW(0, IDC_ARROW);
    wc.hbrBackground = (COLOR_WINDOW + 1) as HBRUSH;
    wc.lpfnWndProc = Some(proc_);
    wc.lpszClassName = cls.as_ptr();
    RegisterClassW(&wc); // fails harmlessly when already registered

    let px = |v: f32| (v * scale).round() as i32;
    let (cw, ch) = (px(600.0), px(470.0));
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let mut rc = RECT { left: 0, top: 0, right: cw, bottom: ch };
    AdjustWindowRectEx(&mut rc, style, 0, 0);
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    let x = (GetSystemMetrics(SM_CXSCREEN) - w) / 2;
    let y = (GetSystemMetrics(SM_CYSCREEN) - h) / 2;
    let title = wide("Bus widget – change stop");
    let win = CreateWindowExW(0, cls.as_ptr(), title.as_ptr(), style, x, y, w, h, 0, 0, hinst, null());

    let face = wide(taskbar_dock::theme::font_override::BUS_PICKER);
    let font = CreateFontW(-px(taskbar_dock::theme::BODY.px), 0, 0, 0, taskbar_dock::theme::BODY.weight, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr());
    let child = |class: &str, text: &str, style: u32, id: u16, x: i32, y: i32, w: i32, h: i32, ex: u32| {
        let (c, t) = (wide(class), wide(text));
        let hw = CreateWindowExW(ex, c.as_ptr(), t.as_ptr(), WS_CHILD | WS_VISIBLE | style, x, y, w, h, win, id as HMENU, hinst, null());
        SendMessageW(hw, WM_SETFONT, font as WPARAM, 1);
        hw
    };
    let m = px(14.0);
    child("STATIC", "Search for a stop (name or stop code), e.g. \u{201C}utrecht rhone\u{201D}:", 0, 0, m, m, cw - 2 * m, px(20.0), 0);
    let edit = child("EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32, ID_EDIT, m, px(38.0), cw - 2 * m, px(26.0), WS_EX_CLIENTEDGE);
    let list = child(
        "LISTBOX",
        "",
        WS_TABSTOP | WS_VSCROLL | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
        ID_LIST,
        m,
        px(72.0),
        cw - 2 * m,
        px(320.0),
        WS_EX_CLIENTEDGE,
    );
    let st = child("STATIC", "", 0, 0, m, px(400.0), cw - 2 * m, px(20.0), 0);
    let back = child("BUTTON", "← Back", WS_TABSTOP | BS_PUSHBUTTON as u32, ID_BACK, cw - m - px(220.0), ch - m - px(30.0), px(100.0), px(30.0), 0);
    child("BUTTON", "Select", WS_TABSTOP | BS_DEFPUSHBUTTON as u32, IDOK as u16, cw - m - px(110.0), ch - m - px(30.0), px(110.0), px(30.0), 0);
    EnableWindow(back, 0);

    p(|p| {
        p.hwnd.set(win);
        p.edit.set(edit);
        p.list.set(list);
        p.status.set(st);
        p.back.set(back);
        p.font.set(font);
        p.mode.set(Mode::Loading);
        p.chosen.set(None);
    });
    ShowWindow(win, SW_SHOW);
    SetForegroundWindow(win);
    SetFocus(edit);

    if !data::have_timetable(data_dir()) {
        status("The national timetable isn't on disk yet (it downloads once a day). Try again later.");
        return;
    }
    status("Loading stops…");
    thread::spawn(move || {
        *STOPS.lock().unwrap() = Some(data::all_stops(data_dir()).map_err(|e| e.to_string()));
        PostMessageW(win, WM_STOPS_DONE, 0, 0);
    });
}

unsafe fn refilter() {
    let edit = p(|p| p.edit.get());
    let mut buf = [0u16; 256];
    let n = GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32);
    let query = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
    let terms: Vec<String> = data::fold(&query).split_whitespace().map(String::from).collect();
    p(|p| p.mode.set(Mode::Stops));
    EnableWindow(p(|p| p.back.get()), 0);
    if terms.is_empty() {
        p(|p| p.shown.borrow_mut().clear());
        list_set(std::iter::empty());
        status("Type part of the stop name.");
        return;
    }
    let (rows, total) = p(|p| {
        let stops = p.stops.borrow();
        let hits: Vec<usize> = stops
            .iter()
            .enumerate()
            .filter(|(_, s)| s.code == query.trim() || terms.iter().all(|t| s.folded.contains(t.as_str())))
            .map(|(i, _)| i)
            .collect();
        let total = hits.len();
        let shown: Vec<usize> = hits.into_iter().take(MAX_SHOWN).collect();
        let rows: Vec<String> = shown.iter().map(|&i| format!("{}    ({})", stops[i].name, stops[i].code)).collect();
        *p.shown.borrow_mut() = shown;
        (rows, total)
    });
    list_set(rows.into_iter());
    status(&match total {
        0 => "No stops found.".to_string(),
        n if n > MAX_SHOWN => format!("{n} platforms; showing the first {MAX_SHOWN}, type more to narrow down."),
        n => format!("{n} platforms (each direction of a stop is a separate platform). Pick one, then Select."),
    });
}

unsafe fn select() {
    let (list, win, mode) = p(|p| (p.list.get(), p.hwnd.get(), p.mode.get()));
    let row = SendMessageW(list, LB_GETCURSEL, 0, 0);
    if row < 0 {
        return;
    }
    match mode {
        Mode::Loading => {}
        Mode::Stops => {
            let Some(i) = p(|p| p.shown.borrow().get(row as usize).copied()) else { return };
            let (code, name) = p(|p| {
                let s = &p.stops.borrow()[i];
                (s.code.clone(), s.name.clone())
            });
            p(|p| {
                p.chosen.set(Some(i));
                p.mode.set(Mode::Loading);
            });
            list_set(std::iter::empty());
            status(&format!("Finding the lines at {name} ({code})… this reads the whole timetable, up to a minute."));
            thread::spawn(move || {
                *LINES.lock().unwrap() = Some(data::lines_at_stop(data_dir(), &code).map_err(|e| e.to_string()));
                PostMessageW(win, WM_LINES_DONE, 0, 0);
            });
        }
        Mode::Lines => {
            let cfg = p(|p| {
                let lines = p.lines.borrow();
                let l = lines.get(row as usize)?;
                let stops = p.stops.borrow();
                let stop = &stops[p.chosen.get()?];
                // label: stop name without the "Town, " prefix, then where the bus goes
                let short = stop.name.split_once(", ").map_or(stop.name.as_str(), |x| x.1);
                let towards = if l.headsign.is_empty() { &l.next } else { &l.headsign };
                Some(data::Config {
                    agency: l.agency.clone(),
                    line: l.line.clone(),
                    stop_code: stop.code.clone(),
                    label: format!("{short} → {towards}"),
                    dest: String::new(),
                    badge_color: String::new(), // new line: its own colour from the timetable
                    badge_text: String::new(),
                    compact: crate::current().0.compact,
                })
            });
            if let Some(cfg) = cfg {
                set_config(cfg);
                DestroyWindow(win);
            }
        }
    }
}

unsafe extern "system" fn proc_(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_COMMAND => {
            let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            match (id, code) {
                (ID_EDIT, EN_CHANGE) if p(|p| p.mode.get()) != Mode::Loading => refilter(),
                (ID_LIST, LBN_DBLCLK) => select(),
                (ID_BACK, _) if p(|p| p.mode.get()) == Mode::Lines => refilter(),
                (x, _) if x == IDOK as u16 => select(),
                (x, _) if x == IDCANCEL as u16 => {
                    DestroyWindow(h);
                }
                _ => {}
            }
            0
        }
        WM_CTLCOLORSTATIC => {
            // labels on the window's white background instead of dialog grey
            SetBkMode(w as HDC, TRANSPARENT as _);
            GetSysColorBrush(COLOR_WINDOW) as LRESULT
        }
        WM_STOPS_DONE => {
            match STOPS.lock().unwrap().take() {
                Some(Ok(v)) => {
                    let n = v.len();
                    p(|p| *p.stops.borrow_mut() = v);
                    refilter();
                    if p(|p| p.shown.borrow().is_empty()) {
                        status(&format!("{n} platforms loaded. Type part of the stop name."));
                    }
                }
                Some(Err(e)) => status(&format!("Couldn't read the timetable: {e}")),
                None => {}
            }
            0
        }
        WM_LINES_DONE => {
            match LINES.lock().unwrap().take() {
                Some(Ok(v)) if !v.is_empty() => {
                    let rows: Vec<String> = v
                        .iter()
                        .map(|l| {
                            let next = if l.next.is_empty() { "trip ends here".to_string() } else { format!("next stop {}", l.next) };
                            let head = if l.headsign.is_empty() { String::new() } else { format!(" · {}", l.headsign) };
                            format!("Line {}  ({})  ·  {}{}", l.line, l.agency, next, head)
                        })
                        .collect();
                    p(|p| {
                        *p.lines.borrow_mut() = v;
                        p.mode.set(Mode::Lines);
                    });
                    list_set(rows.into_iter());
                    EnableWindow(p(|p| p.back.get()), 1);
                    status("Pick the line and direction (\u{201C}next stop\u{201D} shows which way it goes), then Select.");
                }
                Some(Ok(_)) => {
                    p(|p| p.mode.set(Mode::Stops));
                    status("No buses found at this platform. Press Back or search again.");
                    EnableWindow(p(|p| p.back.get()), 1);
                }
                Some(Err(e)) => {
                    p(|p| p.mode.set(Mode::Stops));
                    status(&format!("Couldn't read the timetable: {e}"));
                }
                None => {}
            }
            0
        }
        WM_DESTROY => {
            // free the stop list; the widget goes back to its small footprint
            p(|p| {
                p.hwnd.set(0);
                p.stops.borrow_mut().clear();
                p.stops.borrow_mut().shrink_to_fit();
                p.shown.borrow_mut().clear();
                p.lines.borrow_mut().clear();
                DeleteObject(p.font.get());
            });
            0
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}
