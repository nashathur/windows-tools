//! Shared, validated settings writes for Block, dockctl and the settings window.
use crate::{
    describe::{self, Setting},
    registry::Entry,
};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
/// The submitted setting value, either as text or as parsed JSON.
pub enum Input {
    /// Parse the user's entered text according to the setting description.
    Text(String),
    /// Validate an already parsed JSON value.
    Value(Value),
}
#[derive(Clone, Debug, PartialEq)]
/// A settings section with descriptions and defaults.
pub struct Target {
    /// The settings.json section name.
    pub section: String,
    /// The descriptions accepted by this target.
    pub settings: Vec<Setting>,
    /// The default values for described settings.
    pub defaults: Value,
}
#[derive(Clone, Debug, PartialEq)]
/// A validation failure or a settings-file write failure.
pub enum WriteError {
    /// The key or value is invalid.
    Invalid(String),
    /// The settings file could not be updated.
    NotSaved(String),
}
impl Target {
    /// Creates a target from a widget's parsed description.
    pub fn from_describe(d: &crate::block::Describe) -> Self {
        Self {
            section: d.section.clone(),
            settings: d.settings.clone(),
            defaults: d.defaults.clone(),
        }
    }
    /// Creates a target for shared dock settings.
    pub fn dock() -> Self {
        Self {
            section: "dock".into(),
            settings: crate::style::dock_settings(),
            defaults: crate::style::dock_defaults(),
        }
    }
    /// Builds a widget description from its spec and Block-owned rows.
    pub fn from_spec(spec: &crate::block::Spec) -> Self {
        Self::from_describe(&crate::block::describe(
            spec.section,
            spec.class,
            spec.title,
            spec.description,
            spec.settings,
            spec.defaults,
            spec.log_file.clone(),
        ))
    }
    /// Finds the description for one key.
    pub fn setting(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.key == key)
    }
}
#[derive(Clone, Debug, PartialEq)]
/// Reports the stored value and whether the write and widget reload succeeded.
pub struct Applied {
    /// The canonical value written, if validation succeeded.
    pub value: Option<Value>,
    /// Whether the settings file was updated.
    pub saved: Result<(), String>,
    /// Whether each running widget confirmed the reload.
    pub followed: Result<(), String>,
    /// Whether the submitted value failed validation.
    pub invalid: bool,
}
/// Validates a value and writes only the described setting key.
pub fn write(target: &Target, key: &str, input: Input) -> Result<Value, WriteError> {
    let setting = target
        .setting(key)
        .ok_or_else(|| WriteError::Invalid(format!("unknown key {key:?}")))?;
    let value = match input {
        Input::Text(s) => describe::parse(setting, &s),
        Input::Value(v) => describe::check(setting, &v),
    }
    .map_err(WriteError::Invalid)?;
    crate::settings::update(|root| {
        if !root[target.section.as_str()].is_object() {
            root[&target.section] = serde_json::json!({});
        }
        describe::set_at(&mut root[&target.section], key, value.clone());
    })
    .map_err(|e| WriteError::NotSaved(e.to_string()))?;
    Ok(value)
}
/// Writes the described default for one key.
pub fn write_default(target: &Target, key: &str) -> Result<Value, WriteError> {
    let value = describe::value_at(&target.defaults, key)
        .cloned()
        .ok_or_else(|| WriteError::Invalid(format!("unknown key {key:?}")))?;
    write(target, key, Input::Value(value))
}
/// Writes a setting and asks the listed running widgets to reload.
pub fn apply(target: &Target, key: &str, input: Input, follow: &[Entry]) -> Applied {
    apply_inner(target, key, input, follow)
}
/// Restores a setting's described default and asks widgets to reload.
pub fn reset(target: &Target, key: &str, follow: &[Entry]) -> Applied {
    let Some(v) = describe::value_at(&target.defaults, key).cloned() else {
        return Applied {
            value: None,
            saved: Err(format!("Invalid: unknown key {key:?}")),
            followed: Ok(()),
            invalid: true,
        };
    };
    apply_inner(target, key, Input::Value(v), follow)
}
/// Implements setting writes and records reload confirmations.
fn apply_inner(target: &Target, key: &str, input: Input, follow: &[Entry]) -> Applied {
    let value = match write(target, key, input) {
        Ok(v) => v,
        Err(WriteError::Invalid(e)) => {
            return Applied {
                value: None,
                saved: Err(format!("Invalid: {e}")),
                followed: Ok(()),
                invalid: true,
            }
        }
        Err(WriteError::NotSaved(e)) => {
            return Applied {
                value: None,
                saved: Err(e),
                followed: Ok(()),
                invalid: false,
            }
        }
    };
    let mut not_confirmed = Vec::new();
    for e in follow {
        let args = vec!["reload".to_string()];
        match crate::client::send(
            &e.class,
            crate::client::ExeMatch::Path(e.exe.clone()),
            &args,
            std::time::Duration::from_secs(5),
        ) {
            Ok(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => {}
            Err(crate::client::SendError::NotRunning) => {}
            _ => not_confirmed.push(e.name.clone()),
        }
    }
    Applied {
        value: Some(value),
        saved: Ok(()),
        followed: if not_confirmed.is_empty() {
            Ok(())
        } else {
            Err(format!("{} hasn't confirmed", not_confirmed.join(", ")))
        },
        invalid: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    /// Checks dotted lookup and writes to a nested leaf only.
    fn nested_lookup_and_leaf_write() {
        let mut v = json!({"tint_dark":{"quiet":0.2,"hover":0.3}});
        assert_eq!(describe::value_at(&v, "tint_dark.quiet"), Some(&json!(0.2)));
        describe::set_at(&mut v, "tint_dark.active", json!(0.4));
        assert_eq!(
            v,
            json!({"tint_dark":{"quiet":0.2,"hover":0.3,"active":0.4}})
        );
    }
    #[test]
    /// Checks that dock settings include all tint leaves.
    fn described_key_lookup_includes_nested_dock_leaves() {
        let t = Target::dock();
        assert!(t.setting("tint_dark.quiet").is_some());
        assert!(t.setting("tint_light.hover").is_some());
        assert!(t.setting("missing").is_none());
    }
}
