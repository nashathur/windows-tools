//! Registered widget executables stored in settings.json.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
/// A registered widget executable and its Block window class.
pub struct Entry {
    /// The widget section name in settings.json.
    pub name: String,
    /// The canonical executable path.
    pub exe: PathBuf,
    /// The class used for the widget's hidden Block window.
    pub class: String,
}

/// Reads registered widgets from settings.json.
pub fn list() -> Vec<Entry> {
    match crate::settings::read()
        .ok()
        .and_then(|v| v.get("widgets").cloned())
    {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| {
                Some(Entry {
                    name: v.get("name")?.as_str()?.into(),
                    exe: PathBuf::from(v.get("exe")?.as_str()?),
                    class: v.get("class")?.as_str()?.into(),
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Describes and registers a widget executable.
pub fn register(exe: &Path) -> Result<Entry, String> {
    let canonical = crate::describe::canonical_exe(exe)?;
    let desc = crate::describe::cached(&canonical)?;
    if ["dock", "startup", "widgets", "settings"].contains(&desc.section.as_str()) {
        return Err(format!("reserved widget name {:?}", desc.section));
    }
    if list()
        .iter()
        .any(|e| e.class == desc.class && e.name != desc.section)
    {
        return Err(format!("class {:?} is already registered", desc.class));
    }
    let entry = Entry {
        name: desc.section,
        exe: canonical,
        class: desc.class,
    };
    let exe_text = entry.exe.to_string_lossy().to_string();
    crate::settings::update(|root| {
        if !root.is_object() {
            *root = json!({});
        }
        if !root["widgets"].is_array() {
            root["widgets"] = json!([]);
        }
        let list = root["widgets"].as_array_mut().unwrap();
        if let Some(at) = list
            .iter()
            .position(|v| v.get("name").and_then(Value::as_str) == Some(&entry.name))
        {
            list[at] = json!({"name":entry.name,"exe":exe_text,"class":entry.class});
        } else {
            list.push(json!({"name":entry.name,"exe":exe_text,"class":entry.class}));
        }
        if let Some(startup) = root["startup"].as_array_mut() {
            if let Some(v) = startup
                .iter_mut()
                .find(|v| v.get("name").and_then(Value::as_str) == Some(&entry.name))
            {
                v["exe"] = json!(exe_text);
            }
        }
    })
    .map_err(|e| e.to_string())?;
    Ok(entry)
}

/// Removes a widget and its startup entry in one settings update.
pub fn remove(name: &str) -> Result<(), String> {
    crate::settings::update(|root| {
        if let Some(a) = root.get_mut("widgets").and_then(Value::as_array_mut) {
            a.retain(|v| v.get("name").and_then(Value::as_str) != Some(name));
        }
        if let Some(a) = root.get_mut("startup").and_then(Value::as_array_mut) {
            a.retain(|v| v.get("name").and_then(Value::as_str) != Some(name));
        }
    })
    .map_err(|e| e.to_string())
}
