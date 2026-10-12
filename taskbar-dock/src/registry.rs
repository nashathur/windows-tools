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
    let entry = Entry {
        name: desc.section,
        exe: canonical,
        class: desc.class,
    };
    let exe_text = entry.exe.to_string_lossy().to_string();
    let mut registration = Ok(());
    crate::settings::update(|root| {
        registration = register_in(root, &entry, &exe_text);
    })
    .map_err(|e| e.to_string())?;
    registration?;
    Ok(entry)
}

/// Checks class uniqueness and applies one registration to the locked settings value.
fn register_in(root: &mut Value, entry: &Entry, exe_text: &str) -> Result<(), String> {
    if root
        .get("widgets")
        .and_then(Value::as_array)
        .is_some_and(|widgets| {
            widgets.iter().any(|widget| {
                widget.get("class").and_then(Value::as_str) == Some(&entry.class)
                    && widget.get("name").and_then(Value::as_str) != Some(&entry.name)
            })
        })
    {
        return Err(format!("class {:?} is already registered", entry.class));
    }
    if !root.is_object() {
        *root = json!({});
    }
    if !root["widgets"].is_array() {
        root["widgets"] = json!([]);
    }
    let widgets = root["widgets"].as_array_mut().unwrap();
    if let Some(at) = widgets
        .iter()
        .position(|v| v.get("name").and_then(Value::as_str) == Some(&entry.name))
    {
        widgets[at] = json!({"name":entry.name,"exe":exe_text,"class":entry.class});
    } else {
        widgets.push(json!({"name":entry.name,"exe":exe_text,"class":entry.class}));
    }
    if let Some(startup) = root["startup"].as_array_mut() {
        if let Some(v) = startup
            .iter_mut()
            .find(|v| v.get("name").and_then(Value::as_str) == Some(&entry.name))
        {
            v["exe"] = json!(exe_text);
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_conflict_leaves_registry_unchanged() {
        let mut settings = json!({
            "widgets": [{"name":"weather", "exe":"weather.exe", "class":"WeatherMain"}],
            "startup": [{"name":"weather", "exe":"weather.exe"}]
        });
        let before = settings.clone();
        let entry = Entry {
            name: "other".into(),
            exe: PathBuf::from("other.exe"),
            class: "WeatherMain".into(),
        };

        assert_eq!(
            register_in(&mut settings, &entry, "other.exe"),
            Err("class \"WeatherMain\" is already registered".into())
        );
        assert_eq!(settings, before);
    }
}
