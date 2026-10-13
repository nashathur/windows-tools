//! `dockctl <widget|all> <command> [args]` controls running widgets or shared settings.
//!
//! Running widgets are found by their `<class>Main` window and owning executable path. Replies
//! are accepted only from the requested window and matching request id. Commands that need no
//! running widget use the shared settings library, unless this process sees a private copy of
//! the settings (`settings::redirected`: started from the Claude app). It simulates no input.
//!
//! The default timeout is 5 seconds, or 120 seconds for `do`; `--timeout` sets another deadline.
//!
//! Exit code: 0 for success (a stopped widget in `all` is not an error), 1 for an error or a
//! missing single widget, and 2 for a usage error.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use taskbar_dock::{
    client::{self, ExeMatch, SendError},
    control::{self, Input, Target},
    ctl::{self, Known, WIDGETS},
    registry,
};

/// Default reply deadline for commands other than `do`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// Default reply deadline for widget `do` commands.
const DO_TIMEOUT: Duration = Duration::from_secs(120);

/// Does this process see a private copy of the settings (started from the Claude app)?
fn private_copy() -> bool {
    taskbar_dock::settings::redirected()
}
const PRIVATE_ERROR: &str = concat!(
    "dockctl is running inside a packaged app (the Claude app): it would use a private copy ",
    "of the settings. Start the widget, or run dockctl through a scheduled task."
);

#[derive(Clone)]
/// Identifies a target widget for both IPC and stopped-widget commands.
struct TargetExe {
    /// The settings section name or built-in widget name.
    name: String,
    /// The class of the widget's hidden Block window.
    class: String,
    /// The executable identity used to find a running widget.
    exe: ExeMatch,
    /// The executable path used to load stopped-widget descriptions.
    fallback_path: PathBuf,
}
/// Parses arguments, runs the requested commands and prints their replies.
fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let (args, timeout) = match parse_timeout(&raw) {
        Ok(x) => x,
        Err(e) => usage(&e),
    };
    let Some((who, cmd)) = args.split_first() else {
        usage("")
    };
    if who == "settings" {
        settings_command(cmd, timeout);
        return;
    }
    if who == "widgets" {
        if private_copy() {
            println!("{}", json!({"ok":false,"error":PRIVATE_ERROR}));
            std::process::exit(1);
        }
        widgets_command(cmd);
        return;
    }
    let process_command = ctl::parse_process_command(cmd);
    if process_command.is_none() {
        if let Err(e) = ctl::parse(cmd) {
            usage(&e);
        }
    }
    let targets = resolve(who);
    if targets.is_empty() {
        usage(&format!("unknown widget {who:?}"));
    }
    let all = who == "all";
    if process_command == Some(ctl::ProcessCommand::Status) {
        let private = private_copy();
        let entries: Vec<_> = targets.iter().map(process_entry).collect();
        let statuses = if private {
            control::status_all_with_records(&entries, std::collections::HashMap::new())
        } else {
            control::status_all(&entries)
        };
        let mut output = serde_json::Map::new();
        for status in statuses {
            let state = match status.state {
                control::State::Running => "running",
                control::State::Idle => "idle",
                control::State::NotResponding => "not responding",
                control::State::Stopped => "stopped",
                control::State::KeepsCrashing => "keeps crashing",
                control::State::Unknown => "unknown",
            };
            let state = if status.state == control::State::Unknown {
                format!(
                    "unknown: {}",
                    status.reason.as_deref().unwrap_or("unknown error")
                )
            } else {
                state.to_string()
            };
            let mut value = json!({
                "ok": true,
                "name": status.name,
                "status": state,
                "pid": status.pid,
                "build": status.build,
                "reason": status.reason,
            });
            if private {
                value["note"] = json!("crash records aren't visible from the Claude app");
            }
            output.insert(status.name, value);
        }
        let value = if all {
            Value::Object(output)
        } else {
            output.into_values().next().unwrap_or(Value::Null)
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
        return;
    }
    if matches!(
        process_command,
        Some(ctl::ProcessCommand::Start | ctl::ProcessCommand::Restart)
    ) {
        if all {
            usage("start and restart require one widget name");
        }
        if private_copy() {
            println!("{}", json!({"ok":false,"error":PRIVATE_ERROR}));
            std::process::exit(1);
        }
        let target = &targets[0];
        let entry = process_entry(target);
        let result = match process_command {
            Some(ctl::ProcessCommand::Start) => {
                control::start(&entry).map(|result| (result.pid, result.warning))
            }
            Some(ctl::ProcessCommand::Restart) => control::restart(&entry).map(|pid| (pid, None)),
            _ => unreachable!(),
        };
        let value = match result {
            Ok((pid, warning)) => json!({"ok":true,"name":target.name,"pid":pid,"warning":warning}),
            Err(error) => json!({"ok":false,"name":target.name,"error":error}),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
        if value["ok"] == false {
            std::process::exit(1);
        }
        return;
    }
    let mut failed = false;
    let mut usage_failed = false;
    let mut out = serde_json::Map::new();
    for t in targets {
        let v = run_target(
            &t,
            cmd,
            timeout.unwrap_or_else(|| {
                if matches!(cmd, [x, ..] if x == "do") {
                    DO_TIMEOUT
                } else {
                    DEFAULT_TIMEOUT
                }
            }),
        );
        let not_running = v.get("absent").and_then(Value::as_str) == Some("not running");
        if !(all && not_running) {
            failed |= v.get("ok").and_then(Value::as_bool) == Some(false);
        }
        usage_failed |= v.get("kind").and_then(Value::as_str) == Some("usage");
        out.insert(t.name, v);
    }
    let value = if all {
        Value::Object(out)
    } else {
        out.into_iter()
            .next()
            .map(|(_, v)| v)
            .unwrap_or(Value::Null)
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    );
    std::process::exit(exit_code(failed, usage_failed));
}

/// Sends a settings-app command and prints its JSON reply.
fn settings_command(args: &[String], timeout: Option<Duration>) {
    if let Err(error) = ctl::parse_settings(args) {
        usage(&error);
    }
    let default = if matches!(args.first().map(String::as_str), Some("click"))
        && args.get(1).is_some_and(|id| {
            id.starts_with("start:") || id.starts_with("quit:") || id.starts_with("restart:")
        }) {
        Duration::from_secs(20)
    } else {
        DEFAULT_TIMEOUT
    };
    let result = client::send(
        "TaskbarDockSettings",
        ExeMatch::FileName("taskbar-dock-settings.exe".into()),
        args,
        timeout.unwrap_or(default),
    );
    let reply = match result {
        Ok(value) => value,
        Err(SendError::NotRunning) => json!({"ok":false,"error":"the settings app isn't running"}),
        Err(error) => json!({"ok":false,"error":error.to_string()}),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&reply).unwrap_or_default()
    );
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        std::process::exit(1);
    }
}
/// Resolves a named widget through the registry, then the built-in table.
fn resolve(who: &str) -> Vec<TargetExe> {
    let entries = registry::list();
    if who == "all" {
        let mut out: Vec<_> = entries.iter().map(entry_target).collect();
        for w in WIDGETS {
            if !out.iter().any(|t| t.name == w.name) {
                out.push(known_target(&w));
            }
        }
        return out;
    }
    if let Some(e) = entries.iter().find(|e| e.name == who) {
        return vec![entry_target(e)];
    }
    WIDGETS
        .iter()
        .find(|w| w.name == who)
        .map(|w| vec![known_target(w)])
        .unwrap_or_default()
}
/// Converts a registry entry into a dockctl target.
fn entry_target(e: &registry::Entry) -> TargetExe {
    TargetExe {
        name: e.name.clone(),
        class: e.class.clone(),
        exe: ExeMatch::Path(e.exe.clone()),
        fallback_path: e.exe.clone(),
    }
}
/// The registry entry process control uses for a target. A target known only by file name
/// (no registry entry) takes the path of its running process, else dockctl's fallback path.
fn process_entry(t: &TargetExe) -> registry::Entry {
    let exe = match &t.exe {
        ExeMatch::Path(path) => path.clone(),
        ExeMatch::FileName(name) => {
            control::running_exe(name).unwrap_or_else(|| t.fallback_path.clone())
        }
    };
    registry::Entry {
        name: t.name.clone(),
        exe,
        class: t.class.clone(),
    }
}
/// Converts a built-in widget into a dockctl target.
fn known_target(w: &Known) -> TargetExe {
    let fallback_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join(w.exe)))
        .unwrap_or_else(|| PathBuf::from(w.exe));
    TargetExe {
        name: w.name.into(),
        class: w.class.into(),
        exe: ExeMatch::FileName(w.exe.into()),
        fallback_path,
    }
}
/// Sends a command to a running widget or uses the stopped-widget path.
fn run_target(t: &TargetExe, args: &[String], timeout: Duration) -> Value {
    match client::send(&t.class, t.exe.clone(), args, timeout) {
        Ok(v) => v,
        Err(SendError::NotRunning) => {
            if ctl::parse(args).is_ok_and(|c| {
                matches!(
                    c,
                    ctl::Command::Set(true, ..)
                        | ctl::Command::Get(true, _)
                        | ctl::Command::Keys(true)
                        | ctl::Command::Unset(true, _)
                )
            }) {
                for e in registry::list() {
                    match client::send(&e.class, ExeMatch::Path(e.exe.clone()), args, timeout) {
                        Ok(v) => return v,
                        Err(SendError::NotRunning) => {}
                        Err(err) => return json!({"ok":false,"error":err.to_string()}),
                    }
                }
            }
            stopped(t, args)
        }
        Err(SendError::RunningNotAnswering) => {
            json!({"ok":false,"absent":"running, not answering"})
        }
        Err(e) => json!({"ok":false,"error":e.to_string()}),
    }
}
/// Handles commands supported when their widget is stopped.
fn stopped(t: &TargetExe, args: &[String]) -> Value {
    let cmd = match ctl::parse(args) {
        Ok(c) => c,
        Err(e) => return json!({"ok":false,"error":e}),
    };
    let uses_own_process = matches!(
        &cmd,
        ctl::Command::Set(..)
            | ctl::Command::Get(..)
            | ctl::Command::Keys(..)
            | ctl::Command::Unset(..)
            | ctl::Command::Describe
            | ctl::Command::Startup(..)
    );
    if uses_own_process && private_copy() {
        return json!({"ok":false,"error":PRIVATE_ERROR});
    }
    match &cmd {
        ctl::Command::Startup(on) => {
            let Some(entry) = registry::list()
                .into_iter()
                .find(|entry| entry.name == t.name)
            else {
                return json!({"ok":false,"error":"widget is not registered"});
            };
            return match on {
                Some(on) => match taskbar_dock::startup::set_exe(&entry.name, &entry.exe, *on) {
                    Ok(()) => json!({"ok":true,"startup":on}),
                    Err(error) => json!({"ok":false,"error":error.to_string()}),
                },
                None => json!({"ok":true,"startup":taskbar_dock::startup::is_on(&entry.name)}),
            };
        }
        ctl::Command::Describe => {
            return match taskbar_dock::describe::cached(&t.fallback_path) {
                Ok(description) => {
                    let mut value = description.to_json();
                    value["ok"] = json!(true);
                    value
                }
                Err(error) => json!({"ok":false,"error":error}),
            };
        }
        ctl::Command::Set(..)
        | ctl::Command::Get(..)
        | ctl::Command::Keys(..)
        | ctl::Command::Unset(..) => {}
        _ => return json!({"ok":false,"absent":"not running"}),
    }
    let dock = match &cmd {
        ctl::Command::Set(d, ..)
        | ctl::Command::Get(d, _)
        | ctl::Command::Keys(d)
        | ctl::Command::Unset(d, _) => *d,
        _ => false,
    };
    let target = if dock {
        Target::dock()
    } else {
        let exe = t.fallback_path.clone();
        match taskbar_dock::describe::cached(&exe) {
            Ok(d) => Target::from_describe(&d),
            Err(e) => return json!({"ok":false,"error":e}),
        }
    };
    match cmd {
        ctl::Command::Set(_, key, text) => {
            let entries = registry::list();
            let follow = if dock { entries.as_slice() } else { &[] };
            if target.setting(&key).is_some() {
                let applied = control::apply(&target, &key, Input::Text(text), follow);
                applied_reply(&key, applied, &target)
            } else {
                let value = ctl::value_of(&text);
                match taskbar_dock::settings::set_key(&target.section, &key, value.clone()) {
                    Ok(()) => json!({"ok":true,"key":key,"value":mask_value(&key,value)}),
                    Err(error) => json!({"ok":false,"error":error.to_string()}),
                }
            }
        }
        ctl::Command::Unset(_, key) => {
            let entries = registry::list();
            let follow = if dock { entries.as_slice() } else { &[] };
            if target.setting(&key).is_some() {
                let applied = control::reset(&target, &key, follow);
                applied_reply(&key, applied, &target)
            } else {
                match ctl::unset_default(&target.defaults, &key) {
                    Ok(value) => {
                        match taskbar_dock::settings::set_key(&target.section, &key, value.clone())
                        {
                            Ok(()) => json!({"ok":true,"key":key,"value":mask_value(&key,value)}),
                            Err(error) => json!({"ok":false,"error":error.to_string()}),
                        }
                    }
                    Err(error) => json!({"ok":false,"error":error}),
                }
            }
        }
        ctl::Command::Get(_, key) => get_reply(&target, key.as_deref()),
        ctl::Command::Keys(_) => keys_reply(&target),
        _ => json!({"ok":false,"absent":"not running"}),
    }
}
/// Formats a setting write result and masks secret values.
fn applied_reply(key: &str, a: control::Applied, target: &Target) -> Value {
    if a.invalid {
        return json!({
            "ok": false,
            "kind": "usage",
            "error": a.saved.err().unwrap_or_default(),
        });
    }
    if let Err(e) = a.saved {
        return json!({"ok":false,"error":e});
    }
    let secret = target
        .setting(key)
        .is_some_and(|s| taskbar_dock::describe::is_secret_kind(&s.kind));
    let value = a.value.map(|value| {
        if secret {
            ctl::mask_with(&json!({key:value}), &[key.into()])[key].clone()
        } else {
            mask_value(key, value)
        }
    });
    let mut reply = json!({"ok":true,"key":key,"value":value});
    if let Err(e) = a.followed {
        reply["followed"] = json!(e);
    }
    reply
}
/// Masks secret values by key name when no description is available.
fn mask_value(key: &str, v: Value) -> Value {
    if ctl::is_secret(key) && v.as_str() != Some("") {
        json!("***")
    } else {
        v
    }
}
/// Reads one key or an entire section from settings.json.
fn get_reply(t: &Target, key: Option<&str>) -> Value {
    let current = taskbar_dock::settings::section(&t.section).unwrap_or(Value::Null);
    match key {
        None => {
            let secrets = t
                .settings
                .iter()
                .filter(|setting| taskbar_dock::describe::is_secret_kind(&setting.kind))
                .map(|setting| setting.key.clone())
                .collect::<Vec<_>>();
            json!({
                "ok": true,
                "section": t.section,
                "value": ctl::mask_with(&current, &secrets),
            })
        }
        Some(key) => taskbar_dock::describe::value_at(&current, key)
            .map(|value| {
                let secret = t
                    .setting(key)
                    .is_some_and(|setting| taskbar_dock::describe::is_secret_kind(&setting.kind));
                let value = if secret {
                    ctl::mask_with(&json!({key:value}), &[key.into()])[key].clone()
                } else {
                    mask_value(key, value.clone())
                };
                json!({"ok":true,"key":key,"value":value})
            })
            .unwrap_or_else(|| {
                json!({
                    "ok": false,
                    "error": format!("{}: no key {key:?}", t.section),
                })
            }),
    }
}
/// Lists described setting keys with their current and default values.
fn keys_reply(t: &Target) -> Value {
    let current = taskbar_dock::settings::section(&t.section).unwrap_or(Value::Null);
    let secrets: Vec<_> = t
        .settings
        .iter()
        .filter(|s| taskbar_dock::describe::is_secret_kind(&s.kind))
        .map(|s| s.key.clone())
        .collect();
    let rows: Vec<_> = t
        .settings
        .iter()
        .map(|setting| {
            let mask = |value: Option<&Value>| {
                value.map_or(Value::Null, |value| {
                    if secrets.contains(&setting.key) {
                        mask_value("secret", value.clone())
                    } else {
                        mask_value(&setting.key, value.clone())
                    }
                })
            };
            json!({
                "key": setting.key,
                "current": mask(taskbar_dock::describe::value_at(&current, &setting.key)),
                "default": mask(taskbar_dock::describe::value_at(&t.defaults, &setting.key)),
            })
        })
        .collect();
    json!({"ok":true,"section":t.section,"keys":rows})
}
/// A registry operation requested through `dockctl widgets`.
enum WidgetsCmd {
    List,
    Register(PathBuf),
    Remove(String),
}
/// Parses a registry command and its arguments.
fn parse_widgets(args: &[String]) -> Result<WidgetsCmd, String> {
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["list"] => Ok(WidgetsCmd::List),
        ["register", exe] => Ok(WidgetsCmd::Register(PathBuf::from(exe))),
        ["remove", name] => Ok(WidgetsCmd::Remove((*name).into())),
        _ => Err("widgets expects list, register <exe>, or remove <name>".into()),
    }
}
/// Runs a registry command and prints its JSON result.
fn widgets_command(args: &[String]) {
    let cmd = parse_widgets(args).unwrap_or_else(|e| usage(&e));
    let result = match cmd {
        WidgetsCmd::List => {
            let widgets: Vec<_> = registry::list()
                .iter()
                .map(|e| {
                    let error = taskbar_dock::describe::cached(&e.exe).err();
                    json!({"name":e.name,"exe":e.exe,"class":e.class,"describe_error":error})
                })
                .collect();
            json!({"ok":true,"widgets":widgets})
        }
        WidgetsCmd::Register(exe) => registry::register(&exe)
            .map(|e| json!({"ok":true,"widget":{"name":e.name,"exe":e.exe,"class":e.class}}))
            .unwrap_or_else(|e| json!({"ok":false,"error":e})),
        WidgetsCmd::Remove(name) => registry::remove(&name)
            .map(|_| json!({"ok":true,"removed":name}))
            .unwrap_or_else(|e| json!({"ok":false,"error":e})),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    if result["ok"] != true {
        std::process::exit(1);
    }
}
/// Prints a usage message and exits with the usage status.
fn usage(msg: &str) -> ! {
    if !msg.is_empty() {
        eprintln!("dockctl: {msg}");
    }
    eprintln!("{}", ctl::USAGE);
    std::process::exit(2)
}
/// Returns dockctl's documented success, failure, or usage exit code.
fn exit_code(failed: bool, usage_failed: bool) -> i32 {
    if usage_failed {
        2
    } else if failed {
        1
    } else {
        0
    }
}
/// Removes leading timeout options from the command arguments.
fn parse_timeout(raw: &[String]) -> Result<(Vec<String>, Option<Duration>), String> {
    let mut args = Vec::new();
    let mut timeout = None;
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == "--timeout" {
            let val = raw.get(i + 1).ok_or("--timeout needs seconds")?;
            let n: u64 = val
                .parse()
                .map_err(|_| format!("--timeout: not a number: {val}"))?;
            if n == 0 {
                return Err("--timeout must be at least 1 second".into());
            }
            timeout = Some(Duration::from_secs(n));
            i += 2;
        } else {
            args.extend_from_slice(&raw[i..]);
            break;
        }
    }
    Ok((args, timeout))
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Checks timeout parsing before the widget name.
    #[test]
    fn timeout_is_removed_before_widget_name() {
        assert_eq!(
            parse_timeout(&["--timeout".into(), "9".into(), "bus".into(), "state".into()]).unwrap(),
            (
                vec!["bus".into(), "state".into()],
                Some(Duration::from_secs(9))
            )
        );
        assert_eq!(
            parse_timeout(&["bus".into(), "state".into()]).unwrap(),
            (vec!["bus".into(), "state".into()], None)
        );
        assert!(parse_timeout(&["--timeout".into(), "x".into(), "bus".into()]).is_err());
    }
    /// Checks the documented success, failure and usage exit codes.
    #[test]
    fn exit_codes_distinguish_usage() {
        assert_eq!(exit_code(false, false), 0);
        assert_eq!(exit_code(true, false), 1);
        assert_eq!(exit_code(true, true), 2);
    }
    /// Checks the supported registry command forms.
    #[test]
    fn parses_every_registry_command() {
        assert!(matches!(
            parse_widgets(&["list".into()]),
            Ok(WidgetsCmd::List)
        ));
        assert!(matches!(
            parse_widgets(&["register".into(), "w.exe".into()]),
            Ok(WidgetsCmd::Register(_))
        ));
        assert!(matches!(
            parse_widgets(&["remove".into(), "weather".into()]),
            Ok(WidgetsCmd::Remove(_))
        ));
        assert!(parse_widgets(&["nope".into()]).is_err());
    }
}
