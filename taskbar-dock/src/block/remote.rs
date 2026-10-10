//! dockctl's side ([`crate::ctl`]): the main window's WM_COPYDATA. Every command goes through
//! Block's own paths (requests, `right_click`, `flip` via the menu panel, `follow_settings`), so
//! it does exactly what the click or edit would. Commands that start a modal loop (the
//! right-click panel) or end the program reply first and release the sender
//! (`ReplyMessage`), then act; the others act, then reply. Either way the next command sees
//! the result.

use super::*;
use crate::ctl::{self, Command, MouseButton, PanelOp, Simulate};
use serde_json::{json, Value};
use std::sync::OnceLock;
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;

thread_local! {
    /// A row `menu click` will click once the menu it opened is up.
    static PENDING: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Posted to the main window to click [`PENDING`] from inside the menu's loop.
pub(super) fn click_message() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(wide("TaskbarDock.dockctl.click").as_ptr()) })
}

pub(super) fn pending_click() {
    if let Some(id) = PENDING.with(|p| p.take()) {
        if !crate::menu_panel::click(id) {
            log(&format!("dockctl: menu click {id:#x}: the menu didn't open"));
        }
    }
}

/// WM_COPYDATA on the main window: a dockctl request (anything else: 0, not handled).
pub(super) fn on_copydata(wp: WPARAM, lp: LPARAM) -> LRESULT {
    let cds = unsafe { &*(lp as *const COPYDATASTRUCT) };
    if cds.dwData != ctl::REQUEST {
        return 0;
    }
    let bytes = if cds.lpData.is_null() { &[][..] } else { unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) } };
    let sender = wp as HWND;
    match ctl::decode(bytes).and_then(|a| ctl::parse(&a)) {
        Ok(cmd) => {
            log(&format!("dockctl: {cmd:?}"));
            run(sender, cmd)
        }
        Err(e) => reply(sender, &ctl::error(e)),
    }
    1
}

fn reply(to: HWND, text: &str) {
    let main = st(|s| s.main).unwrap_or(0);
    let cds = COPYDATASTRUCT { dwData: ctl::REPLY, cbData: text.len() as u32, lpData: text.as_ptr() as *mut _ };
    unsafe {
        let mut _r = 0;
        SendMessageTimeoutW(to, WM_COPYDATA, main as WPARAM, &cds as *const _ as LPARAM, SMTO_ABORTIFHUNG, 2000, &mut _r);
    }
}

/// Reply, and let the sender go on while this thread carries on with the command.
fn reply_and_release(to: HWND, text: &str) {
    reply(to, text);
    unsafe { ReplyMessage(1) };
}

fn run(to: HWND, cmd: Command) {
    let ok = |v: Value| reply(to, &ctl::ok(v));
    match cmd {
        Command::State => ok(state()),
        Command::Panel(op) => {
            match op {
                PanelOp::Open => apply(vec![Request::OpenPanel]),
                PanelOp::Toggle => apply(vec![Request::TogglePanel]),
                PanelOp::Close => panel_closed(false),
            }
            ok(json!({ "panel": panel_state() }))
        }
        Command::MenuOpen => {
            if crate::menu_panel::is_open() {
                return ok(json!({ "menu": "already open" }));
            }
            reply_and_release(to, &ctl::ok(json!({ "menu": "opening" })));
            right_click(false);
        }
        Command::MenuClose => {
            let was = crate::menu_panel::is_open();
            crate::menu_panel::close();
            ok(json!({ "was_open": was }));
        }
        Command::MenuRows => {
            let (open, rows) = menu_rows();
            ok(json!({ "open": open, "rows": rows.iter().map(|r| json!({ "id": r.id, "hex": format!("{:#x}", r.id), "label": r.label, "switch": r.switch })).collect::<Vec<_>>() }));
        }
        Command::MenuClick(name) => {
            let (open, rows) = menu_rows();
            let named: Vec<(usize, String)> = rows.iter().map(|r| (r.id, r.label.clone())).collect();
            let Some(row) = ctl::find_row(&named, &name).and_then(|id| rows.into_iter().find(|r| r.id == id)) else {
                return reply(to, &ctl::error(format!("no menu row {name:?}")));
            };
            let what = json!({ "clicked": row.label, "id": row.id, "switch_was": row.switch, "opened_menu": !open });
            if open {
                crate::menu_panel::click(row.id);
                return ok(what);
            }
            // open it as a right click would; the click runs from inside its loop
            PENDING.with(|p| p.set(Some(row.id)));
            if let Some(main) = st(|s| s.main) {
                unsafe { PostMessageW(main, click_message(), 0, 0) };
            }
            reply_and_release(to, &ctl::ok(what));
            right_click(false);
        }
        Command::Tray(button) => {
            if st(|s| s.tray_on) != Some(true) {
                return reply(to, &ctl::error("the tray icon isn't shown"));
            }
            match button {
                MouseButton::Left => {
                    tray_click(true);
                    ok(json!({ "panel": panel_state() }));
                }
                MouseButton::Right => {
                    reply_and_release(to, &ctl::ok(json!({ "menu": "opening" })));
                    tray_click(false);
                }
            }
        }
        Command::Set(key, value) => {
            let Some(section) = st(|s| s.spec.section) else { return };
            let res = settings::update(|root| {
                if !root.is_object() {
                    *root = json!({});
                }
                if !root[section].is_object() {
                    root[section] = json!({});
                }
                root[section][&key] = value.clone();
            });
            match res {
                Ok(()) => {
                    // as the watch timer would, at once
                    follow_settings();
                    redraw();
                    tray_sync(false, false);
                    let shown = if ctl::is_secret(&key) { json!("***") } else { value };
                    ok(json!({ "key": key, "value": shown }));
                }
                Err(e) => reply(to, &ctl::error(format!("saving failed: {e}"))),
            }
        }
        Command::Get(key) => {
            let Some(section) = st(|s| s.spec.section) else { return };
            let sec = ctl::mask(&settings::section(section).unwrap_or(Value::Null));
            match key {
                None => ok(json!({ "section": section, "value": sec })),
                Some(k) => match sec.get(&k) {
                    Some(v) => ok(json!({ "key": k, "value": v })),
                    None => reply(to, &ctl::error(format!("{section}: no key {k:?}"))),
                },
            }
        }
        Command::Log(n) => {
            let Some(file) = st(|s| s.spec.log_file.clone()).flatten() else {
                return reply(to, &ctl::error("this widget has no log file (Spec::log_file)"));
            };
            match std::fs::read(&file) {
                Ok(b) => {
                    let text = String::from_utf8_lossy(&b);
                    ok(json!({ "file": file.display().to_string(), "lines": ctl::tail(&text, n) }));
                }
                Err(e) => reply(to, &ctl::error(format!("{}: {e}", file.display()))),
            }
        }
        Command::Redraw => {
            redraw();
            tray_sync(false, false);
            ok(json!({}));
        }
        Command::Quit => {
            reply_and_release(to, &ctl::ok(json!({ "quitting": true })));
            quit();
        }
        Command::Simulate(Simulate::TaskbarRestarted) => {
            taskbar_restarted();
            ok(json!({ "strip": strip_state() }));
        }
        Command::Simulate(Simulate::LookChanged) => {
            refresh_look(true);
            redraw();
            ok(json!({ "look": look_state() }));
        }
    }
}

/// The open menu's rows, or else the rows a right click would show now.
fn menu_rows() -> (bool, Vec<Row>) {
    if let Some((_, rows)) = crate::menu_panel::rows() {
        return (true, rows);
    }
    let menu = st(|s| {
        let show = s.has_tray.then_some(s.core.show);
        core::menu(s.content.as_mut(), show, s.core.compact, startup::is_on(s.spec.section))
    });
    let rows = menu
        .map(|m| {
            m.items
                .into_iter()
                .flat_map(|i| match i {
                    Item::Row(r) => vec![r],
                    Item::Pair(a, b) => vec![a, b],
                    Item::Sep => vec![],
                })
                .collect()
        })
        .unwrap_or_default();
    (false, rows)
}

fn rect_of(h: HWND) -> Value {
    if h == 0 {
        return Value::Null;
    }
    let r = unsafe { dock::rect(h) };
    json!({ "x": r.left, "y": r.top, "w": r.right - r.left, "h": r.bottom - r.top })
}

fn strip_state() -> Value {
    let Some((strip, floating, show, compact, fit, side, visible, has_tray)) =
        st(|s| (s.strip, s.floating, s.core.show, s.core.compact, s.core.fit.get(), s.content.side(), s.content.visible(), s.has_tray))
    else {
        return Value::Null;
    };
    json!({
        "hwnd": strip,
        "rect": rect_of(strip),
        "shown": strip != 0 && unsafe { IsWindowVisible(strip) } != 0,
        "visible_by_state": core::shown(show, visible, has_tray),
        "show_on_taskbar": show,
        "compact": compact,
        "fit": format!("{fit:?}"),
        "floating": floating,
        "side": format!("{side:?}"),
    })
}

fn panel_state() -> Value {
    let Some((h, open, hover)) = st(|s| (s.panel, s.core.panel.is_open(), s.core.panel.by_hover())) else { return Value::Null };
    json!({ "open": open, "pinned": open && !hover, "focused": h != 0 && unsafe { GetForegroundWindow() } == h, "hwnd": h, "rect": rect_of(h) })
}

fn look_state() -> Value {
    let Some((light, h)) = st(|s| (s.look.light, if s.strip != 0 { s.strip } else { s.main })) else { return Value::Null };
    json!({ "theme": if light { "light" } else { "dark" }, "scale": system::scale_for(h) })
}

fn state() -> Value {
    let exe = std::env::current_exe().ok();
    let built = exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok()?.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let menu = crate::menu_panel::rows().map(|(h, _)| h).unwrap_or(0);
    let (tray_on, tip, title, section) = st(|s| (s.tray_on, s.tray_tip.clone(), s.spec.title, s.spec.section)).unwrap_or_default();
    json!({
        "widget": title,
        "section": section,
        "pid": std::process::id(),
        "exe": exe.map(|p| p.display().to_string()),
        "exe_built": built,
        "strip": strip_state(),
        "panel": panel_state(),
        "menu": { "open": menu != 0, "focused": menu != 0 && unsafe { GetForegroundWindow() } == menu, "rect": rect_of(menu) },
        "tray": { "shown": tray_on, "tip": tip },
        "look": look_state(),
    })
}
