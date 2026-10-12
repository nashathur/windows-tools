//! Which widgets start with Windows: the `startup` list in settings.json.
//!
//! ```json
//! "startup": [ { "name": "bus", "exe": "C:\\Users\\...\\buswidget.exe" }, ... ]
//! ```
//! Everything on the list is started at sign-in by `taskbar-dock-start.exe` (src/bin), which a
//! shortcut in the Startup folder runs. A widget's "Start with Windows" switch adds or removes
//! its own entry with [`set`]. Widgets refuse a second instance, so starting one that is already
//! running is harmless.
//!
//! Why not the Run registry key: on this Windows build, Run values without a Task Manager
//! "approved" mark were silently skipped at sign-in, while the Startup folder works.

use serde_json::{json, Value};
use std::io;
use std::path::PathBuf;

pub const SECTION: &str = "startup";

pub struct Entry {
    pub name: String,
    pub exe: PathBuf,
}

/// The widgets to start, in list order.
pub fn list() -> Vec<Entry> {
    match crate::settings::section(SECTION) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| {
                Some(Entry {
                    name: v.get("name")?.as_str()?.to_string(),
                    exe: PathBuf::from(v.get("exe")?.as_str()?),
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether the widget `name` is on the list.
pub fn is_on(name: &str) -> bool {
    list().iter().any(|e| e.name == name)
}

/// Puts the running exe on the list as `name`, or takes `name` off it.
pub fn set(name: &str, on: bool) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    set_exe(name, &exe, on)
}

/// Puts the supplied executable on the startup list, or takes its name off.
pub fn set_exe(name: &str, exe: &std::path::Path, on: bool) -> io::Result<()> {
    let exe = exe.to_string_lossy();
    crate::settings::update(|root| {
        if let Some(obj) = root.as_object_mut() {
            apply(
                obj.entry(SECTION).or_insert_with(|| json!([])),
                name,
                on.then_some(&*exe),
            );
        }
    })
}

/// Adds or updates (`Some(exe)`) or removes (`None`) the entry `name`, keeping the others and
/// their order.
fn apply(list: &mut Value, name: &str, exe: Option<&str>) {
    if !list.is_array() {
        *list = json!([]);
    }
    let items = list.as_array_mut().unwrap();
    let at = items
        .iter()
        .position(|v| v.get("name").and_then(Value::as_str) == Some(name));
    match (at, exe) {
        (Some(i), Some(exe)) => items[i]["exe"] = json!(exe),
        (None, Some(exe)) => items.push(json!({ "name": name, "exe": exe })),
        (Some(i), None) => {
            items.remove(i);
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_updates_and_removes_by_name_keeping_order() {
        let mut l = json!(null);
        apply(&mut l, "bus", Some("a.exe"));
        apply(&mut l, "weather", Some("b.exe"));
        apply(&mut l, "bus", Some("c.exe"));
        assert_eq!(
            l,
            json!([{ "name": "bus", "exe": "c.exe" }, { "name": "weather", "exe": "b.exe" }])
        );
        apply(&mut l, "bus", None);
        apply(&mut l, "media", None);
        assert_eq!(l, json!([{ "name": "weather", "exe": "b.exe" }]));
    }
}
