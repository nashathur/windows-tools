//! dockctl's side ([`crate::ctl`]): the main window's WM_COPYDATA. Every command goes through
//! Block's own paths (requests, `right_click`, `flip` via the menu panel, `follow_settings`), so
//! it does exactly what the click or edit would. Commands that start a modal loop (the
//! right-click panel) or end the program reply first and release the sender
//! (`ReplyMessage`), then act; the others act, then reply. Either way the next command sees
//! the result.

use super::*;
use crate::ctl::{self, Command, MouseButton, PanelKey as CtlPanelKey, PanelOp, Simulate};
use serde_json::{json, Value};
use std::sync::OnceLock;
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;

thread_local! {
    /// A row `menu click` will click once the menu it opened is up.
    static PENDING: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    /// A widget that answers `Done` has dropped or kept its token; ignore its later drop-reply.
    static DONE_IDS: std::cell::RefCell<std::collections::HashSet<u64>> = std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Posted to the main window to click [`PENDING`] from inside the menu's loop.
pub(super) fn click_message() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(wide("TaskbarDock.dockctl.click").as_ptr()) })
}

/// Completes a queued menu-row click from inside the menu's message loop.
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
    if cds.dwData != ctl::REQUEST && cds.dwData != ctl::REQUEST2 {
        return 0;
    }
    let bytes = if cds.lpData.is_null() {
        Vec::new()
    } else {
        unsafe {
            std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize).to_vec()
        }
    };
    let v2 = cds.dwData == ctl::REQUEST2;
    let (id, args) = if v2 {
        match ctl::decode2(&bytes) {
            Ok((id, args)) => (id, args),
            Err(e) => {
                send_reply(ReplyTo { hwnd: wp as HWND, id: 0, v2 }, &ctl::error(e));
                return 1;
            }
        }
    } else {
        (0, match ctl::decode(&bytes) {
            Ok(args) => args,
            Err(e) => {
                send_reply(ReplyTo { hwnd: wp as HWND, id: 0, v2 }, &ctl::error(e));
                return 1;
            }
        })
    };
    let to = ReplyTo { hwnd: wp as HWND, id, v2 };
    match ctl::parse(&args) {
        Ok(cmd) => {
            if !matches!(cmd, Command::Ping) {
                log(&command_log(&cmd));
            }
            run(to, cmd)
        }
        Err(e) => send_reply(to, &ctl::error(e)),
    }
    1
}

#[derive(Clone, Copy)]
struct ReplyTo {
    hwnd: HWND,
    id: u64,
    v2: bool,
}

/// Sends a control reply while its data remains valid for the message.
fn send_reply(to: ReplyTo, text: &str) {
    let main = st(|s| s.main).unwrap_or(0);
    let bytes = if to.v2 {
        ctl::encode_reply2(to.id, text)
    } else {
        text.as_bytes().to_vec()
    };
    let cds = COPYDATASTRUCT {
        dwData: if to.v2 { ctl::REPLY2 } else { ctl::REPLY },
        cbData: bytes.len() as u32,
        lpData: bytes.as_ptr() as *mut _,
    };
    unsafe {
        let mut _r = 0;
        if SendMessageTimeoutW(to.hwnd, WM_COPYDATA, main as WPARAM, &cds as *const _ as LPARAM, SMTO_ABORTIFHUNG, 2000, &mut _r) == 0 {
            log("dockctl: reply delivery failed; requester is gone or hung");
        }
    }
}

/// Reply, and let the sender go on while this thread carries on with the command.
fn reply_and_release(to: ReplyTo, text: &str) {
    send_reply(to, text);
    unsafe { ReplyMessage(1) };
}

/// Builds a usage-error reply, including command help when available.
fn usage_reply(message: &str, usage: Option<&str>) -> String {
    let mut value = json!({ "ok": false, "kind": "usage", "error": message });
    if let Some(usage) = usage {
        value["usage"] = json!(usage);
    }
    value.to_string()
}

/// Formats a command for the widget log, masking described secrets.
fn command_log(cmd: &Command) -> String {
    match cmd {
        Command::Set(dock, key, value) => {
            let described = if *dock {
                crate::style::dock_settings()
                    .into_iter()
                    .any(|setting| {
                        setting.key == *key
                            && crate::describe::is_secret_kind(&setting.kind)
                    })
            } else {
                st(|state| {
                    state
                        .spec
                        .settings
                        .and_then(|make| make().into_iter().find(|setting| setting.key == *key))
                })
                .flatten()
                .is_some_and(|setting| crate::describe::is_secret_kind(&setting.kind))
            };
            let shown = if (described || ctl::is_secret(key)) && !value.is_empty() {
                "***".to_string()
            } else {
                value.clone()
            };
            format!("dockctl: Set({key:?}, {shown:?})")
        }
        Command::Ping => "dockctl: Ping".into(),
        Command::Describe => "dockctl: Describe".into(),
        _ => format!("dockctl: {cmd:?}"),
    }
}

/// Builds the setting target for the widget or shared dock section.
fn target(dock: bool) -> crate::control::Target {
    if dock {
        crate::control::Target::dock()
    } else {
        st(|state| crate::control::Target::from_spec(&state.spec)).unwrap()
    }
}

/// Returns the described secret keys in the selected section.
fn secret_keys(dock: bool) -> Vec<String> {
    target(dock)
        .settings
        .into_iter()
        .filter_map(|setting| {
            crate::describe::is_secret_kind(&setting.kind).then_some(setting.key)
        })
        .collect()
}

/// Reports whether the widget spec provides its own setting descriptions.
fn has_widget_descriptions() -> bool {
    st(|state| state.spec.settings.is_some()).unwrap_or(true)
}

/// Builds widget defaults, adding Block's stored defaults where needed.
fn defaults_for(dock: bool) -> Result<(&'static str, Value), String> {
    let (section, mut defaults) = if dock {
        ("dock", crate::style::dock_defaults())
    } else {
        let Some((section, defaults)) = st(|s| (s.spec.section, s.spec.defaults)) else {
            return Err("this widget doesn't list its defaults (Spec::defaults)".into());
        };
        let Some(make_defaults) = defaults else {
            return Err("this widget doesn't list its defaults (Spec::defaults)".into());
        };
        (section, make_defaults())
    };
    if !defaults.is_object() {
        defaults = json!({});
    }
    if !dock {
        defaults["compact"] = json!(false);
        defaults["show_on_taskbar"] = json!(true);
    }
    Ok((section, defaults))
}

/// Refreshes the widget after a settings change.
fn settings_written(dock: bool) {
    if dock {
        refresh_look(true);
        redraw();
    } else {
        follow_settings(true);
        redraw();
        tray_sync(false, false);
    }
}

/// Replies after other registered widgets have had a chance to reload shared dock settings.
fn reply_after_write(to: ReplyTo, dock: bool, mut value: Value) {
    if !dock {
        return send_reply(to, &ctl::ok(value));
    }
    let current = st(|state| state.spec.section).unwrap_or("");
    std::thread::spawn(move || {
        let entries: Vec<_> = crate::registry::list()
            .into_iter()
            .filter(|entry| entry.name != current)
            .collect();
        let workers: Vec<_> = entries
            .into_iter()
            .map(|entry| {
                std::thread::spawn(move || {
                    let args = vec!["reload".to_string()];
                    let result = crate::client::send(
                        &entry.class,
                        crate::client::ExeMatch::Path(entry.exe),
                        &args,
                        std::time::Duration::from_secs(5),
                    );
                    // A stopped widget reads the new value when it starts (as control::apply).
                    let confirmed = matches!(
                        result,
                        Ok(ref reply) if reply.get("ok").and_then(Value::as_bool) == Some(true)
                    ) || matches!(result, Err(crate::client::SendError::NotRunning));
                    (entry.name, confirmed)
                })
            })
            .collect();
        let failed: Vec<_> = workers
            .into_iter()
            .filter_map(|worker| match worker.join() {
                Ok((_name, true)) => None,
                Ok((name, false)) => Some(name),
                Err(_) => Some("reload worker".into()),
            })
            .collect();
        if let Some(warning) = reload_warning(&failed) {
            value["warning"] = json!(warning);
        }
        send_reply(to, &ctl::ok(value));
    });
}

fn reload_warning(failed: &[String]) -> Option<String> {
    (!failed.is_empty()).then(|| format!("{} hasn't confirmed", failed.join(", ")))
}

/// Writes a described key through control validation or uses the legacy value conversion.
fn set_value(
    target: &crate::control::Target,
    key: &str,
    text: String,
    write_legacy: impl FnOnce(&str, &str, Value) -> Result<(), String>,
) -> Result<Value, crate::control::WriteError> {
    if target.setting(key).is_some() {
        return crate::control::write(target, key, crate::control::Input::Text(text));
    }

    let value = ctl::value_of(&text);
    write_legacy(&target.section, key, value.clone())
        .map_err(crate::control::WriteError::NotSaved)?;
    Ok(value)
}

/// Replies with current and default values for the section's keys.
fn keys(to: ReplyTo, dock: bool) {
    let (section, defaults) = match defaults_for(dock) {
        Ok(values) => values,
        Err(error) => return send_reply(to, &ctl::error(error)),
    };
    let current = settings::section(section).unwrap_or_else(|| json!({}));
    let described = target(dock).settings;
    let mut names: Vec<String> = if !dock && !has_widget_descriptions() {
        defaults
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    } else {
        described.into_iter().map(|setting| setting.key).collect()
    };
    names.sort();
    let secrets = secret_keys(dock);
    let rows: Vec<Value> = names.iter().map(|key| {
        let mask = |value: Option<&Value>| {
            value.map_or(Value::Null, |value| {
                if secrets.contains(key) {
                    ctl::mask_setting("secret", value)
                } else {
                    ctl::mask_setting(key, value)
                }
            })
        };
        json!({
            "key": key,
            "current": mask(crate::describe::value_at(&current, key)),
            "default": mask(crate::describe::value_at(&defaults, key)),
        })
    }).collect();
    send_reply(to, &ctl::ok(json!({ "section": section, "keys": rows })));
}

/// Restores a described default or uses the legacy default lookup.
fn unset(to: ReplyTo, dock: bool, key: &str) {
    let t = target(dock);
    let result = if t.setting(key).is_some() {
        crate::control::write_default(&t, key)
    } else {
        let defaults = defaults_for(dock).map(|(_, defaults)| defaults);
        match defaults.and_then(|defaults| ctl::unset_default(&defaults, key)) {
            Ok(value) => settings::set_key(&t.section, key, value.clone())
                .map(|_| value)
                .map_err(|error| crate::control::WriteError::NotSaved(error.to_string())),
            Err(error) => Err(crate::control::WriteError::Invalid(error)),
        }
    };
    match result {
        Ok(value) => {
            settings_written(dock);
            let shown = ctl::mask_with(&json!({key:value}), &secret_keys(dock))[key].clone();
            reply_after_write(to, dock, json!({"key":key,"value":shown}));
        }
        Err(crate::control::WriteError::Invalid(error)) => {
            send_reply(to, &usage_reply(&error, None))
        }
        Err(crate::control::WriteError::NotSaved(error)) => {
            send_reply(to, &ctl::error(format!("saving failed: {error}")))
        }
    }
}

fn do_command(to: ReplyTo, name: &str, args: &[String]) {
    let Some(names) = st(|s| s.content.commands()) else { return };
    if name == "help" && args.is_empty() {
        let list: Vec<Value> = names.iter().map(|(name, usage)| json!({ "name": name, "usage": usage })).collect();
        return send_reply(to, &ctl::ok(json!({ "commands": list })));
    }
    let Some((_, usage)) = names.iter().find(|(n, _)| *n == name) else {
        let message = if names.is_empty() {
            format!("unknown command {name:?}: this widget has no do commands")
        } else {
            let known = names.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ");
            format!("unknown command {name:?}; available: {known}")
        };
        return send_reply(to, &usage_reply(&message, None));
    };
    // a v1 request can't take a late reply
    if !to.v2 {
        return send_reply(to, &ctl::error("this dockctl is older than the widget: rebuild dockctl"));
    }
    let Some((main, logger)) = st(|s| (s.main, s.spec.log)) else { return };
    let token = ReplyToken::new(main, to.hwnd, to.id, Some(*usage), logger);
    let reply = call(|content, ctx| content.command(name, args, token, ctx));
    if let Some(CommandReply::Done(result)) = reply {
        DONE_IDS.with(|ids| ids.borrow_mut().insert(to.id));
        send_result(to, result, Some(usage));
    }
}

fn send_result(to: ReplyTo, result: Result<Value, CommandError>, usage: Option<&str>) {
    match result {
        Ok(value) => send_reply(to, &ctl::ok(value)),
        Err(error) if error.usage => send_reply(to, &usage_reply(&error.message, usage)),
        Err(error) => send_reply(to, &ctl::error(error.message)),
    }
}

/// Handles a posted result from a worker, on Block's UI thread.
pub(super) fn command_reply(lp: LPARAM) {
    if lp == 0 { return; }
    let result = unsafe { *Box::from_raw(lp as *mut CommandResult) };
    if DONE_IDS.with(|ids| ids.borrow_mut().remove(&result.id)) { return; }
    send_result(ReplyTo { hwnd: result.requester, id: result.id, v2: true }, result.result, result.usage);
}

fn run(to: ReplyTo, cmd: Command) {
    let ok = |v: Value| send_reply(to, &ctl::ok(v));
    match cmd {
        Command::State => ok(state()),
        Command::Panel(op) => {
            match op {
                PanelOp::Open => apply(vec![Request::OpenPanel]),
                PanelOp::Toggle => apply(vec![Request::TogglePanel]),
                PanelOp::Close => panel_closed(false),
                PanelOp::OpenPicker => {
                    let opened = open_panel_kind_with(PanelKind::Picker, true);
                    if !opened { return send_reply(to, &ctl::error("this widget has no picker panel")); }
                }
            }
            ok(json!({ "panel": panel_state() }))
        }
        Command::PanelKey(key) => {
            let (key, ctrl, alt) = match key {
                CtlPanelKey::Enter => (PanelKeyCode::Enter, false, false), CtlPanelKey::Esc => (PanelKeyCode::Escape, false, false),
                CtlPanelKey::Up => (PanelKeyCode::Up, false, false), CtlPanelKey::Down => (PanelKeyCode::Down, false, false),
                CtlPanelKey::PgUp => (PanelKeyCode::PageUp, false, false), CtlPanelKey::PgDn => (PanelKeyCode::PageDown, false, false),
                CtlPanelKey::Home => (PanelKeyCode::Home, false, false), CtlPanelKey::End => (PanelKeyCode::End, false, false),
                CtlPanelKey::CtrlHome => (PanelKeyCode::Home, true, false), CtlPanelKey::CtrlEnd => (PanelKeyCode::End, true, false),
                CtlPanelKey::Back => (PanelKeyCode::Left, false, true), CtlPanelKey::Backspace => (PanelKeyCode::Backspace, false, false),
                CtlPanelKey::Delete => (PanelKeyCode::Delete, false, false), CtlPanelKey::Left => (PanelKeyCode::Left, false, false),
                CtlPanelKey::Right => (PanelKeyCode::Right, false, false), CtlPanelKey::CtrlA => (PanelKeyCode::A, true, false),
                CtlPanelKey::CtrlC => (PanelKeyCode::C, true, false), CtlPanelKey::CtrlV => (PanelKeyCode::V, true, false),
                CtlPanelKey::CtrlX => (PanelKeyCode::X, true, false), CtlPanelKey::CtrlBackspace => (PanelKeyCode::Backspace, true, false),
            };
            dispatch_panel_key_with_fallback(PanelKey { key, ctrl, alt });
            ok(json!({ "panel": panel_state() }))
        }
        Command::PanelType(text) => {
            dispatch_panel_text(&text);
            ok(json!({ "panel": panel_state() }))
        }
        Command::PanelState => ok(json!({ "panel": panel_state() })),
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
                return send_reply(to, &ctl::error(format!("no menu row {name:?}")));
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
                return send_reply(to, &ctl::error("the tray icon isn't shown"));
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
        Command::Set(dock, key, text) => {
            let t = target(dock);
            let result = set_value(&t, &key, text, |section, key, value| {
                settings::set_key(section, key, value).map_err(|error| error.to_string())
            });
            match result {
                Ok(value) => {
                    settings_written(dock);
                    let shown = if secret_keys(dock).contains(&key) && value.as_str() != Some("") {
                        json!("***")
                    } else {
                        ctl::mask_setting(&key, &value)
                    };
                    reply_after_write(to, dock, json!({"key":key,"value":shown}));
                }
                Err(crate::control::WriteError::Invalid(error)) => {
                    send_reply(to, &usage_reply(&error, None))
                }
                Err(crate::control::WriteError::NotSaved(error)) => {
                    send_reply(to, &ctl::error(format!("saving failed: {error}")))
                }
            }
        }
        Command::Get(dock, key) => {
            let section = if dock { "dock" } else { let Some(s) = st(|s| s.spec.section) else { return }; s };
            let raw = settings::section(section).unwrap_or(Value::Null);
            let sec = ctl::mask_with(&raw, &secret_keys(dock));
            match key {
                None => ok(json!({ "section": section, "value": sec })),
                Some(k) => match crate::describe::value_at(&sec, &k) {
                    Some(v) => ok(json!({ "key": k, "value": v })),
                    None => send_reply(to, &ctl::error(format!("{section}: no key {k:?}"))),
                },
            }
        }
        Command::Keys(dock) => keys(to, dock),
        Command::Unset(dock, key) => unset(to, dock, &key),
        Command::Do(name, args) => do_command(to, &name, &args),
        Command::Log(n) => {
            let Some(file) = st(|s| s.spec.log_file.clone()).flatten() else {
                return send_reply(to, &ctl::error("this widget has no log file (Spec::log_file)"));
            };
            match std::fs::read(&file) {
                Ok(b) => {
                    let text = String::from_utf8_lossy(&b);
                    ok(json!({ "file": file.display().to_string(), "lines": ctl::tail(&text, n) }));
                }
                Err(e) => send_reply(to, &ctl::error(format!("{}: {e}", file.display()))),
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
        Command::Reload => {
            follow_settings(true);
            refresh_look(true);
            redraw();
            tray_sync(false, false);
            ok(json!({}));
        }
        Command::Ping => {
            let pid = std::process::id();
            let build = st(|state| state.build_time).unwrap_or(0);
            ok(json!({"pid":pid,"build":build}))
        }
        Command::Describe => {
            let description = describe_current();
            ok(description)
        }
        Command::Startup(on) => {
            let section = st(|state| state.spec.section).unwrap_or("");
            let result = match on {
                Some(on) => crate::startup::set(section, on).map(|_| on),
                None => Ok(crate::startup::is_on(section)),
            };
            match result {
                Ok(value) => ok(json!({"startup":value})),
                Err(error) => send_reply(
                    to,
                    &ctl::error(format!("startup failed: {error}")),
                ),
            }
        }
    }
}

/// Builds the running widget's description through Block's shared builder.
fn describe_current() -> Value {
    let Some((section, class, title, description, settings, defaults, log_file)) = st(|state| {
        (
            state.spec.section,
            state.spec.class,
            state.spec.title,
            state.spec.description,
            state.spec.settings,
            state.spec.defaults,
            state.spec.log_file.clone(),
        )
    }) else {
        return json!({});
    };
    crate::block::describe(
        section,
        class,
        title,
        description,
        settings,
        defaults,
        log_file,
    )
    .to_json()
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
    let Some((h, open, hover, kind, data)) = st(|s| (s.panel, s.core.panel.is_open(), s.core.panel.by_hover(), s.panel_kind, s.content.panel_state(s.panel_kind))) else { return Value::Null };
    json!({ "kind": if kind == PanelKind::Picker { "picker" } else { "main" }, "open": open, "pinned": open && !hover, "focused": h != 0 && unsafe { GetForegroundWindow() } == h, "hwnd": h, "rect": rect_of(h), "content": data })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks described values and keeps legacy values in their original form.
    #[test]
    fn set_uses_description_or_legacy_value_conversion() {
        let described = crate::control::Target {
            section: "widget".into(),
            settings: vec![crate::describe::Setting::number("width", "Width", 1.0, 5.0, 1.0)],
            defaults: json!({ "width": 3 }),
        };
        let mut wrote_legacy = false;
        let result = set_value(&described, "width", "6".into(), |_, _, _| {
            wrote_legacy = true;
            Ok(())
        });
        assert!(matches!(result, Err(crate::control::WriteError::Invalid(_))));
        assert!(!wrote_legacy);

        let undescribed = crate::control::Target {
            section: "widget".into(),
            settings: Vec::new(),
            defaults: json!({ "width": 3 }),
        };
        let mut written = None;
        let result = set_value(&undescribed, "label", "plain text".into(), |section, key, value| {
            written = Some((section.to_string(), key.to_string(), value));
            Ok(())
        });
        assert_eq!(result, Ok(json!("plain text")));
        assert_eq!(written, Some(("widget".into(), "label".into(), json!("plain text"))));
    }

    #[test]
    fn command_log_masks_secret_set_values() {
        let line = command_log(&Command::Set(false, "knmi_api_key".into(), "sensitive".into()));
        assert_eq!(line, "dockctl: Set(\"knmi_api_key\", \"***\")");
        assert!(!line.contains("sensitive"));
    }

    #[test]
    fn dock_reload_warning_names_every_widget_that_did_not_confirm() {
        assert_eq!(reload_warning(&[]), None);
        assert_eq!(
            reload_warning(&["weather".into(), "bus".into()]).as_deref(),
            Some("weather, bus hasn't confirmed")
        );
    }
}
