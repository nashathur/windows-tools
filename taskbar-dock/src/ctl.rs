//! Command-line control of the widgets on Block (`dockctl`), for testing without the mouse.
//!
//! `dockctl.exe` finds a widget's hidden Block main window (`<class>Main`, checked against the
//! widget's exe) and sends it a [`WM_COPYDATA`] tagged [`REQUEST`]: the arguments, UTF-8,
//! separated by [`SEP`]. Block parses them with [`parse`], does what the click or edit would do,
//! and sends a [`WM_COPYDATA`] tagged [`REPLY`] back to the sender's window (the message's
//! wParam): JSON, `{"ok":true,...}` or `{"ok":false,"error":"..."}`. Window messages aren't
//! affected by the Claude app's MSIX container, so whatever the widget reads or writes is the
//! real `%APPDATA%`.
//!
//! [`WM_COPYDATA`]: windows_sys::Win32::UI::WindowsAndMessaging::WM_COPYDATA

use serde_json::{json, Value};

/// `dwData` of a request: "DKCQ".
pub const REQUEST: usize = 0x444B_4351;
/// `dwData` of a reply: "DKCR".
pub const REPLY: usize = 0x444B_4352;
/// `dwData` of a request with a request id.
pub const REQUEST2: usize = 0x444B_4353;
/// `dwData` of a reply with a request id.
pub const REPLY2: usize = 0x444B_4354;
/// Between arguments (values may hold spaces).
pub const SEP: char = '\u{1f}';

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelOp {
    Open,
    Close,
    Toggle,
    OpenPicker,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelKey {
    Enter, Esc, Up, Down, PgUp, PgDn, Home, End, CtrlHome, CtrlEnd, Back,
    Backspace, Delete, Left, Right, CtrlA, CtrlC, CtrlV, CtrlX, CtrlBackspace,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Simulate {
    /// As if Explorer restarted (the TaskbarCreated broadcast).
    TaskbarRestarted,
    /// Re-read Windows' theme and settings, telling the widget even if nothing changed.
    LookChanged,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Command {
    State,
    Panel(PanelOp),
    PanelKey(PanelKey),
    PanelType(String),
    PanelState,
    MenuOpen,
    MenuClose,
    MenuRows,
    /// A row by id (decimal or 0x hex) or label; opens the menu first if it's closed.
    MenuClick(String),
    Tray(MouseButton),
    Set(bool, String, String),
    Get(bool, Option<String>),
    Keys(bool),
    Unset(bool, String),
    Do(String, Vec<String>),
    Log(usize),
    Redraw,
    Quit,
    Simulate(Simulate),
    Reload,
    Ping,
    Describe,
    Startup(Option<bool>),
}

pub const USAGE: &str = "\
usage: dockctl <widget|all> <command> [args] | dockctl widgets list|register <exe>|remove <name>
  state                          strip, panel, menu, tray and look as JSON
  panel open|close|toggle        the left-click panel, as a click does
  panel open picker              the widget's picker panel
  panel key <name>               send a panel key (enter, esc, up, down, ...)
  panel type \"<text>\"            send panel text input
  panel state                    active panel kind, open state, rect and content state
  menu open|close                the right-click panel
  menu rows                      its rows: id, label, switch state
  menu click <id|label>          a row, as a click does (opens the menu first)
  tray click left|right          a click on the tray icon
  set <key> <value>              the widget's settings section (value: JSON, or else text)
  get [key]                      one key, or the section (secrets masked)
  keys [--dock]                  list settings with current and default values
  set/get/unset --dock           edit or inspect shared dock settings
  describe                       setting descriptions and defaults
  startup on|off|get             change or inspect Start with Windows
  reload                         re-read settings and redraw the widget
  ping                           quiet process probe
  widgets list|register|remove   manage registered widget executables
  do <name> [args]               run a widget command; do help lists available commands
  --timeout <s>                  reply deadline (default 5; do commands default to 120)
  log [n]                        last n lines of the widget's log (default 20)
  redraw                         measure and draw again
  quit                           as the Quit row
  simulate taskbar-restarted     as if Explorer restarted
  simulate look-changed          re-read Windows' theme";

/// Default number of log lines.
pub const LOG_LINES: usize = 20;

/// The arguments after the widget name.
pub fn parse<S: AsRef<str>>(args: &[S]) -> Result<Command, String> {
    let a: Vec<&str> = args.iter().map(|s| s.as_ref()).collect();
    let bad = || Err(format!("unknown command: {}", a.join(" ")));
    Ok(match a.as_slice() {
        ["state"] => Command::State,
        ["panel", "open"] => Command::Panel(PanelOp::Open),
        ["panel", "close"] => Command::Panel(PanelOp::Close),
        ["panel", "toggle"] => Command::Panel(PanelOp::Toggle),
        ["panel", "open", "picker"] => Command::Panel(PanelOp::OpenPicker),
        ["panel", "state"] => Command::PanelState,
        ["panel", "type", text @ ..] if !text.is_empty() => Command::PanelType(text.join(" ")),
        ["panel", "key", name] => Command::PanelKey(parse_panel_key(name)?),
        ["menu", "open"] => Command::MenuOpen,
        ["menu", "close"] => Command::MenuClose,
        ["menu", "rows"] => Command::MenuRows,
        ["menu", "click", row @ ..] if !row.is_empty() => Command::MenuClick(row.join(" ")),
        ["tray", "click", "left"] => Command::Tray(MouseButton::Left),
        ["tray", "click", "right"] => Command::Tray(MouseButton::Right),
        ["set", "--dock", key, value @ ..] if !value.is_empty() => Command::Set(true, key.to_string(), value.join(" ")),
        ["set", key, value @ ..] if !value.is_empty() => Command::Set(false, key.to_string(), value.join(" ")),
        ["get", "--dock"] => Command::Get(true, None),
        ["get", "--dock", key] => Command::Get(true, Some(key.to_string())),
        ["get"] => Command::Get(false, None),
        ["get", key] => Command::Get(false, Some(key.to_string())),
        ["keys", "--dock"] => Command::Keys(true),
        ["keys"] => Command::Keys(false),
        ["unset", "--dock", key] => Command::Unset(true, key.to_string()),
        ["unset", key] => Command::Unset(false, key.to_string()),
        ["do", name, args @ ..] => Command::Do(name.to_string(), args.iter().map(|s| (*s).to_string()).collect()),
        ["log"] => Command::Log(LOG_LINES),
        ["log", n] => Command::Log(n.parse().map_err(|_| format!("log: not a number: {n}"))?),
        ["redraw"] => Command::Redraw,
        ["quit"] => Command::Quit,
        ["reload"] => Command::Reload,
        ["ping"] => Command::Ping,
        ["describe"] => Command::Describe,
        ["startup", "get"] => Command::Startup(None),
        ["startup", "on"] => Command::Startup(Some(true)),
        ["startup", "off"] => Command::Startup(Some(false)),
        ["simulate", "taskbar-restarted"] => Command::Simulate(Simulate::TaskbarRestarted),
        ["simulate", "look-changed"] => Command::Simulate(Simulate::LookChanged),
        _ => return bad(),
    })
}

fn parse_panel_key(name: &str) -> Result<PanelKey, String> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "enter" => PanelKey::Enter, "esc" => PanelKey::Esc, "up" => PanelKey::Up, "down" => PanelKey::Down,
        "pgup" => PanelKey::PgUp, "pgdn" => PanelKey::PgDn, "home" => PanelKey::Home, "end" => PanelKey::End,
        "ctrl+home" => PanelKey::CtrlHome, "ctrl+end" => PanelKey::CtrlEnd, "back" => PanelKey::Back,
        "backspace" => PanelKey::Backspace, "delete" => PanelKey::Delete, "left" => PanelKey::Left, "right" => PanelKey::Right,
        "ctrl+a" => PanelKey::CtrlA, "ctrl+c" => PanelKey::CtrlC, "ctrl+v" => PanelKey::CtrlV, "ctrl+x" => PanelKey::CtrlX,
        "ctrl+backspace" => PanelKey::CtrlBackspace,
        _ => return Err(format!("unknown panel key: {name}")),
    })
}

/// A `set` value: JSON when it parses (`true`, `5`, `"x"`, `[1]`), else the text itself.
pub fn value_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

/// The arguments as sent (UTF-8, [`SEP`] between them).
pub fn encode<S: AsRef<str>>(args: &[S]) -> Vec<u8> {
    args.iter().map(|s| s.as_ref()).collect::<Vec<_>>().join(&SEP.to_string()).into_bytes()
}

/// The arguments from a request's bytes.
pub fn decode(bytes: &[u8]) -> Result<Vec<String>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "request isn't UTF-8".to_string())?;
    Ok(if text.is_empty() { Vec::new() } else { text.split(SEP).map(str::to_string).collect() })
}

/// Encodes a v2 request as a decimal id followed by the ordinary arguments.
pub fn encode2<S: AsRef<str>>(id: u64, args: &[S]) -> Vec<u8> {
    let mut fields = Vec::with_capacity(args.len() + 1);
    fields.push(id.to_string());
    fields.extend(args.iter().map(|s| s.as_ref().to_string()));
    encode(&fields)
}

/// Decodes a v2 request and returns its id and arguments.
pub fn decode2(bytes: &[u8]) -> Result<(u64, Vec<String>), String> {
    let mut fields = decode(bytes)?.into_iter();
    let id = fields.next().ok_or_else(|| "request has no id".to_string())?.parse().map_err(|_| "request id isn't a decimal u64".to_string())?;
    Ok((id, fields.collect()))
}

/// Encodes a v2 reply as a decimal id followed by its JSON text.
pub fn encode_reply2(id: u64, json: &str) -> Vec<u8> {
    encode(&[id.to_string(), json.to_string()])
}

/// Decodes a v2 reply and returns its id and JSON text.
pub fn decode_reply2(bytes: &[u8]) -> Result<(u64, String), String> {
    let mut fields = decode(bytes)?.into_iter();
    let id = fields.next().ok_or_else(|| "reply has no id".to_string())?.parse().map_err(|_| "reply id isn't a decimal u64".to_string())?;
    let json = fields.next().ok_or_else(|| "reply has no JSON".to_string())?;
    if fields.next().is_some() { return Err("reply has extra fields".into()); }
    Ok((id, json))
}

/// A key whose value is never printed (API keys, tokens, passwords).
pub fn is_secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["key", "token", "secret", "password"].iter().any(|s| k.contains(s))
}

/// Masks a setting value when its name is secret, for both logs and replies.
pub fn mask_setting(key: &str, value: &Value) -> Value {
    if is_secret(key) { mask_secret(value) } else { value.clone() }
}

/// Masks a non-empty secret value.
fn mask_secret(value: &Value) -> Value {
    if !value.is_null() && value.as_str() != Some("") {
        json!("***")
    } else {
        value.clone()
    }
}

/// `v` with every secret key's value (at any depth) replaced by `"***"` (an empty one stays
/// empty, so "not set" still shows).
pub fn mask(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    (k.clone(), if is_secret(k) { mask_setting(k, v) } else { mask(v) })
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(mask).collect()),
        _ => v.clone(),
    }
}

/// Masks values whose keys are described as Secret, while retaining the name-based fallback.
pub fn mask_with(v: &Value, secret_keys: &[String]) -> Value {
    fn walk(v: &Value, keys: &[String], prefix: &str) -> Value {
        match v {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| {
                        let path = if prefix.is_empty() {
                            key.clone()
                        } else {
                            format!("{prefix}.{key}")
                        };
                        let described = keys.iter().any(|secret| secret == &path || secret == key);
                        let value = if described {
                            mask_secret(value)
                        } else if is_secret(key) {
                            mask_setting(key, value)
                        } else {
                            walk(value, keys, &path)
                        };
                        (key.clone(), value)
                    })
                    .collect(),
            ),
            Value::Array(values) => {
                Value::Array(values.iter().map(|value| walk(value, keys, prefix)).collect())
            }
            _ => v.clone(),
        }
    }

    walk(v, secret_keys, "")
}

/// One settings row, masking the current and default values by the setting name.
pub fn key_row(key: &str, current: Option<&Value>, default: Option<&Value>) -> Value {
    let hide = |v: Option<&Value>| v.map_or(Value::Null, |v| mask_setting(key, v));
    json!({ "key": key, "current": hide(current), "default": hide(default) })
}

/// Resolves a setting to its configured default or reports all known keys.
pub fn unset_default(defaults: &Value, key: &str) -> Result<Value, String> {
    defaults.get(key).cloned().ok_or_else(|| {
        let mut keys: Vec<String> = defaults.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
        keys.sort();
        format!("unknown key {key:?}; known keys: {}", keys.join(", "))
    })
}

/// A success reply: `fields` (an object) with `"ok": true`.
pub fn ok(fields: Value) -> String {
    let mut m = serde_json::Map::new();
    m.insert("ok".into(), json!(true));
    if let Value::Object(f) = fields {
        m.extend(f);
    }
    Value::Object(m).to_string()
}

pub fn error(msg: impl Into<String>) -> String {
    json!({ "ok": false, "error": msg.into() }).to_string()
}

/// The last `n` lines of `text`.
pub fn tail(text: &str, n: usize) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

/// A menu row named on the command line: its id (decimal or `0x` hex) or label (case and a
/// trailing "…" ignored). `rows` are (id, label).
pub fn find_row(rows: &[(usize, String)], name: &str) -> Option<usize> {
    let id = match name.strip_prefix("0x").or_else(|| name.strip_prefix("0X")) {
        Some(hex) => usize::from_str_radix(hex, 16).ok(),
        None => name.parse().ok(),
    };
    if let Some(id) = id.filter(|id| rows.iter().any(|(r, _)| r == id)) {
        return Some(id);
    }
    let norm = |s: &str| s.trim_end_matches(['…', '.']).trim().to_lowercase();
    rows.iter().find(|(_, l)| norm(l) == norm(name)).map(|(id, _)| *id)
}

/// One widget dockctl knows: its name on the command line, its strip class (the main window
/// is `<class>Main`), and its exe's file name.
#[derive(Clone, Copy, Debug)]
pub struct Known {
    pub name: &'static str,
    pub class: &'static str,
    pub exe: &'static str,
}

pub const WIDGETS: [Known; 4] = [
    Known { name: "weather", class: "TaskbarWeatherLabel", exe: "taskbar-weather.exe" },
    Known { name: "bus", class: "BusWidgetBar", exe: "buswidget.exe" },
    Known { name: "discord", class: "TaskbarDiscordStrip", exe: "taskbar-discord.exe" },
    Known { name: "media", class: "TaskbarMediaStrip", exe: "taskbar-media.exe" },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Command, String> {
        parse(&s.split(' ').collect::<Vec<_>>())
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(p("state"), Ok(Command::State));
        assert_eq!(p("panel open"), Ok(Command::Panel(PanelOp::Open)));
        assert_eq!(p("panel close"), Ok(Command::Panel(PanelOp::Close)));
        assert_eq!(p("panel toggle"), Ok(Command::Panel(PanelOp::Toggle)));
        assert_eq!(p("panel open picker"), Ok(Command::Panel(PanelOp::OpenPicker)));
        assert_eq!(p("panel state"), Ok(Command::PanelState));
        assert_eq!(p("panel key ctrl+backspace"), Ok(Command::PanelKey(PanelKey::CtrlBackspace)));
        assert_eq!(p("panel type Utrecht CS"), Ok(Command::PanelType("Utrecht CS".into())));
        for (name, key) in [
            ("enter", PanelKey::Enter), ("esc", PanelKey::Esc), ("up", PanelKey::Up), ("down", PanelKey::Down),
            ("pgup", PanelKey::PgUp), ("pgdn", PanelKey::PgDn), ("home", PanelKey::Home), ("end", PanelKey::End),
            ("ctrl+home", PanelKey::CtrlHome), ("ctrl+end", PanelKey::CtrlEnd), ("back", PanelKey::Back),
            ("backspace", PanelKey::Backspace), ("delete", PanelKey::Delete), ("left", PanelKey::Left),
            ("right", PanelKey::Right), ("ctrl+a", PanelKey::CtrlA), ("ctrl+c", PanelKey::CtrlC),
            ("ctrl+v", PanelKey::CtrlV), ("ctrl+x", PanelKey::CtrlX), ("ctrl+backspace", PanelKey::CtrlBackspace),
        ] {
            assert_eq!(parse(&["panel", "key", name]), Ok(Command::PanelKey(key)));
        }
        assert_eq!(p("menu open"), Ok(Command::MenuOpen));
        assert_eq!(p("menu close"), Ok(Command::MenuClose));
        assert_eq!(p("menu rows"), Ok(Command::MenuRows));
        assert_eq!(p("menu click Compact mode"), Ok(Command::MenuClick("Compact mode".into())));
        assert_eq!(p("tray click left"), Ok(Command::Tray(MouseButton::Left)));
        assert_eq!(p("tray click right"), Ok(Command::Tray(MouseButton::Right)));
        assert_eq!(p("set compact true"), Ok(Command::Set(false, "compact".into(), "true".into())));
        assert_eq!(p("set --dock gap 4"), Ok(Command::Set(true, "gap".into(), "4".into())));
        assert_eq!(p("get"), Ok(Command::Get(false, None)));
        assert_eq!(p("get compact"), Ok(Command::Get(false, Some("compact".into()))));
        assert_eq!(p("get --dock"), Ok(Command::Get(true, None)));
        assert_eq!(p("keys"), Ok(Command::Keys(false)));
        assert_eq!(p("keys --dock"), Ok(Command::Keys(true)));
        assert_eq!(p("unset --dock gap"), Ok(Command::Unset(true, "gap".into())));
        assert_eq!(p("unset compact"), Ok(Command::Unset(false, "compact".into())));
        assert_eq!(p("do play --app VLC"), Ok(Command::Do("play".into(), vec!["--app".into(), "VLC".into()])));
        assert_eq!(p("do help"), Ok(Command::Do("help".into(), vec![])));
        assert_eq!(p("log"), Ok(Command::Log(LOG_LINES)));
        assert_eq!(p("log 5"), Ok(Command::Log(5)));
        assert_eq!(p("redraw"), Ok(Command::Redraw));
        assert_eq!(p("quit"), Ok(Command::Quit));
        assert_eq!(p("simulate taskbar-restarted"), Ok(Command::Simulate(Simulate::TaskbarRestarted)));
        assert_eq!(p("simulate look-changed"), Ok(Command::Simulate(Simulate::LookChanged)));
        assert_eq!(p("reload"), Ok(Command::Reload));
        assert_eq!(p("ping"), Ok(Command::Ping));
        assert_eq!(p("describe"), Ok(Command::Describe));
        assert_eq!(p("startup on"), Ok(Command::Startup(Some(true))));
        assert_eq!(p("startup off"), Ok(Command::Startup(Some(false))));
        assert_eq!(p("startup get"), Ok(Command::Startup(None)));
    }

    #[test]
    fn refuses_bad_input() {
        for bad in ["", "panel", "panel shut", "menu click", "set compact", "get a b", "tray click middle", "simulate dark", "log x", "nope"] {
            assert!(p(bad).is_err(), "{bad:?} parsed");
        }
        assert!(parse::<&str>(&[]).is_err());
    }

    #[test]
    fn set_values_are_json_or_text() {
        assert_eq!(value_of("5"), json!(5));
        assert_eq!(value_of("2.5"), json!(2.5));
        assert_eq!(value_of("false"), json!(false));
        assert_eq!(value_of("\"5\""), json!("5"));
        assert_eq!(value_of("[1,2]"), json!([1, 2]));
        assert_eq!(value_of("Utrecht"), json!("Utrecht"));
        assert_eq!(value_of("null"), Value::Null);
        assert_eq!(
            parse(&["set", "stop_code", "59150850"]),
            Ok(Command::Set(
                false,
                "stop_code".into(),
                "59150850".into()
            ))
        );
    }

    #[test]
    fn arguments_survive_the_wire() {
        let args = ["set", "city", "De Bilt → Utrecht"];
        assert_eq!(decode(&encode(&args)).unwrap(), args);
        assert_eq!(decode(b"").unwrap(), Vec::<String>::new());
        assert!(decode(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn v2_wire_round_trips() {
        let args = ["do", "say", "hello there"];
        assert_eq!(decode2(&encode2(42, &args)).unwrap(), (42, args.map(str::to_string).to_vec()));
        let reply = encode_reply2(42, r#"{"ok":true}"#);
        assert_eq!(decode_reply2(&reply).unwrap(), (42, r#"{"ok":true}"#.into()));
        assert!(decode2(b"bad\x1fstate").is_err());
        assert!(decode_reply2(b"42").is_err());
    }

    #[test]
    fn secrets_are_masked_at_any_depth_but_empty_ones_show() {
        let v = json!({ "knmi_api_key": "abc", "city": "Utrecht", "nested": { "Token": "t", "n": 1 }, "api_key": "", "password": null });
        assert_eq!(mask(&v), json!({ "knmi_api_key": "***", "city": "Utrecht", "nested": { "Token": "***", "n": 1 }, "api_key": "", "password": null }));
        assert!(is_secret("KNMI_API_KEY") && !is_secret("compact"));
        let described = mask_with(
            &json!({"opaque":"value","nested":{"credential":"x","empty":""}}),
            &["opaque".into(), "nested.credential".into()],
        );
        assert_eq!(
            described,
            json!({"opaque":"***","nested":{"credential":"***","empty":""}})
        );
    }

    #[test]
    fn keys_mask_secret_current_and_default_values() {
        assert_eq!(key_row("api_token", Some(&json!("secret")), Some(&json!("default"))), json!({ "key": "api_token", "current": "***", "default": "***" }));
        assert_eq!(key_row("api_token", None, Some(&json!(""))), json!({ "key": "api_token", "current": null, "default": "" }));
    }

    #[test]
    fn unset_unknown_key_lists_known_keys() {
        assert_eq!(unset_default(&json!({ "alpha": 1, "beta": 2 }), "gamma"), Err("unknown key \"gamma\"; known keys: alpha, beta".into()));
    }

    #[test]
    fn replies() {
        let r = ok(json!({ "pid": 7 }));
        assert_eq!(r, r#"{"ok":true,"pid":7}"#);
        let e: Value = serde_json::from_str(&error("x \"y\"")).unwrap();
        assert_eq!(e, json!({ "ok": false, "error": "x \"y\"" }));
    }

    #[test]
    fn tail_takes_the_last_lines() {
        assert_eq!(tail("a\nb\nc\n", 2), ["b", "c"]);
        assert_eq!(tail("a", 5), ["a"]);
        assert!(tail("", 3).is_empty());
    }

    #[test]
    fn rows_by_id_or_label() {
        let rows = vec![(1, "Refresh now".to_string()), (0xF000, "Compact mode".into()), (0xF002, "Edit settings…".into())];
        assert_eq!(find_row(&rows, "1"), Some(1));
        assert_eq!(find_row(&rows, "0xF000"), Some(0xF000));
        assert_eq!(find_row(&rows, "61440"), Some(0xF000));
        assert_eq!(find_row(&rows, "compact MODE"), Some(0xF000));
        assert_eq!(find_row(&rows, "Edit settings"), Some(0xF002));
        assert_eq!(find_row(&rows, "Edit settings..."), Some(0xF002));
        assert_eq!(find_row(&rows, "7"), None);
        assert_eq!(find_row(&rows, "Quit"), None);
    }
}
