//! Pure page and row state for the settings application.
use serde_json::{json, Value};
use taskbar_dock::describe::{Kind, Setting};

/// A settings application page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    /// The registered widget overview.
    Widgets,
    /// One widget's settings page.
    Widget(String),
    /// Reserved shared dock page.
    Dock,
    /// Reserved developer page.
    Developer,
}

/// The kind of native control used for a setting row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// A switch.
    Switch,
    /// Two or three inline options.
    Segments,
    /// A popup list.
    Dropdown,
    /// A numeric range control.
    Slider,
    /// A single-line editor.
    Text,
    /// A text editor with a swatch.
    Color,
    /// A masked text editor.
    Secret,
    /// An automatic switch paired with its inner control.
    Auto,
}

/// A described setting and its mutable UI state.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The settings section name.
    pub section: String,
    /// The setting key, or startup for Start with Windows.
    pub key: String,
    /// The setting's presentation and validation contract.
    pub setting: Setting,
    /// The currently saved value.
    pub value: Value,
    /// The reset value.
    pub default: Value,
    /// Text being edited, if any.
    pub draft: Option<String>,
    /// Whether an Other-enabled choice is currently in its text editor mode.
    pub choice_text_mode: bool,
    /// Sequence of the latest queued write.
    pub pending: Option<u64>,
    /// Validation or write error shown on this row.
    pub error: Option<String>,
    /// Whether user interaction is allowed.
    pub enabled: bool,
}

impl Row {
    /// Creates a row from its description and current and default values.
    pub fn new(section: &str, setting: Setting, value: Value, default: Value) -> Self {
        let choice_text_mode = control_for_value(&setting.kind, &value) == Control::Text;
        Self {
            section: section.into(),
            key: setting.key.clone(),
            setting,
            value,
            default,
            draft: None,
            choice_text_mode,
            pending: None,
            error: None,
            enabled: true,
        }
    }
    /// Reports whether this row should show a Reset action.
    pub fn reset_visible(&self) -> bool {
        self.value != self.default
    }
    /// Accepts an apply result only when it belongs to the newest row write.
    pub fn apply_result(
        &mut self,
        sequence: u64,
        value: Option<Value>,
        error: Option<String>,
    ) -> bool {
        if self.pending != Some(sequence) {
            return false;
        }
        self.pending = None;
        self.draft = None;
        if let Some(value) = value {
            self.value = value;
            self.choice_text_mode =
                control_for_value(&self.setting.kind, &self.value) == Control::Text;
            self.error = None;
        } else {
            self.error = error;
        }
        true
    }
    /// Refreshes from disk only when no local edit, write, or error owns the row.
    pub fn reload(&mut self, value: Value) -> bool {
        if self.draft.is_some() || self.pending.is_some() || self.error.is_some() {
            return false;
        }
        self.value = value;
        self.choice_text_mode = control_for_value(&self.setting.kind, &self.value) == Control::Text;
        true
    }
    /// Chooses a control for this row's kind.
    pub fn control(&self) -> Control {
        if self.choice_text_mode && matches!(self.setting.kind, Kind::Choice { other: true, .. }) {
            return Control::Text;
        }
        control_for_value(&self.setting.kind, &self.value)
    }
}

/// A registered widget and its overview state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetRow {
    /// Registry section name.
    pub name: String,
    /// User-facing title, falling back to its name.
    pub title: String,
    /// One-line description, when available.
    pub description: String,
    /// Current process status text.
    pub status: String,
    /// Current process id, if known.
    pub pid: Option<u32>,
    /// Diagnostic reason, including a failed description.
    pub reason: Option<String>,
    /// Whether the widget's taskbar strip is shown.
    pub show_on_taskbar: bool,
}

/// The keyboard action at the current input precedence level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyTarget {
    /// An active editor consumes all keys.
    Text,
    /// An open dropdown consumes navigation and dismissal keys.
    Dropdown,
    /// Normal page controls receive the key.
    Page,
}

/// Selects which control has keyboard precedence.
pub fn key_target(editing: bool, dropdown_open: bool) -> KeyTarget {
    if editing {
        KeyTarget::Text
    } else if dropdown_open {
        KeyTarget::Dropdown
    } else {
        KeyTarget::Page
    }
}

/// Builds everyday widget rows in description order, with Block and startup rows first.
pub fn widget_rows(
    section: &str,
    settings: &[Setting],
    current: &Value,
    defaults: &Value,
    startup: bool,
) -> Vec<Row> {
    let mut rows = Vec::new();
    for (key, label, fallback) in [
        ("show_on_taskbar", "Show on taskbar", true),
        ("compact", "Compact mode", false),
    ] {
        let setting = Setting::new(key, label, Kind::Switch);
        rows.push(Row::new(
            section,
            setting,
            current.get(key).cloned().unwrap_or(Value::Bool(fallback)),
            defaults.get(key).cloned().unwrap_or(Value::Bool(fallback)),
        ));
    }
    let setting = Setting::new("startup", "Start with Windows", Kind::Switch);
    rows.push(Row::new(
        section,
        setting,
        Value::Bool(startup),
        Value::Bool(false),
    ));
    for setting in settings.iter().filter(|setting| {
        !setting.advanced && setting.key != "show_on_taskbar" && setting.key != "compact"
    }) {
        let value = taskbar_dock::describe::value_at(current, &setting.key)
            .cloned()
            .unwrap_or(Value::Null);
        let default = taskbar_dock::describe::value_at(defaults, &setting.key)
            .cloned()
            .unwrap_or(Value::Null);
        rows.push(Row::new(section, setting.clone(), value, default));
    }
    rows
}

/// Returns the non-empty group heading that begins at a row index.
pub fn group_heading(rows: &[Row], index: usize) -> Option<&str> {
    let row = rows.get(index)?;
    let group = row.setting.group.as_str();
    (!group.is_empty() && (index == 0 || rows[index - 1].setting.group != row.setting.group))
        .then_some(group)
}

/// Chooses a UI control recursively for optional and automatic kinds.
pub fn control_for(kind: &Kind) -> Control {
    match kind {
        Kind::Switch => Control::Switch,
        Kind::Number { .. } => Control::Slider,
        Kind::Text { .. } => Control::Text,
        Kind::Color { .. } => Control::Color,
        Kind::Secret => Control::Secret,
        Kind::Choice {
            options,
            other: false,
        } if (2..=3).contains(&options.len()) => Control::Segments,
        Kind::Choice { .. } => Control::Dropdown,
        Kind::Optional(inner) => control_for(inner),
        Kind::Auto { .. } => Control::Auto,
    }
}

/// Chooses a control using the stored value, including the Choice-with-other text mode.
pub fn control_for_value(kind: &Kind, value: &Value) -> Control {
    match kind {
        Kind::Choice {
            options,
            other: true,
        } if !options.iter().any(|(option, _)| option == value) => Control::Text,
        _ => control_for(kind),
    }
}

/// Returns the value written when an automatic setting is switched off.
pub fn auto_off_value(kind: &Kind, word: &str, default: &Value) -> Option<Value> {
    let Kind::Auto { inner, .. } = kind else {
        return None;
    };
    if default.as_str() != Some(word) {
        return Some(default.clone());
    }
    default_for_kind(inner)
}

/// Produces the minimum or first option used by an Auto inner control.
fn default_for_kind(kind: &Kind) -> Option<Value> {
    match kind {
        Kind::Number { min, integer, .. } if *integer => Some(json!(*min as i64)),
        Kind::Number { min, .. } => Some(json!(min)),
        Kind::Choice { options, .. } => options.first().map(|(value, _)| value.clone()),
        Kind::Optional(inner) => default_for_kind(inner),
        Kind::Auto { word, .. } => Some(json!(word)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Checks Block rows, startup and advanced-row exclusion.
    #[test]
    fn builds_everyday_rows_in_description_order() {
        let settings = vec![
            Setting::new("one", "One", Kind::Switch),
            Setting::new("secret", "Secret", Kind::Secret).advanced(),
            Setting::new(
                "two",
                "Two",
                Kind::Text {
                    placeholder: String::new(),
                    empty_ok: true,
                },
            ),
        ];
        let rows = widget_rows(
            "weather",
            &settings,
            &json!({"one":true,"two":"x"}),
            &json!({"one":false,"two":""}),
            true,
        );
        assert_eq!(
            rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
            ["show_on_taskbar", "compact", "startup", "one", "two"]
        );
        assert_eq!(rows[2].value, json!(true));
    }

    /// Checks stale write results, reload protection and reset visibility.
    #[test]
    fn newest_write_and_reload_rules() {
        let mut row = Row::new(
            "w",
            Setting::new("x", "X", Kind::Switch),
            json!(false),
            json!(false),
        );
        assert!(!row.reset_visible());
        row.pending = Some(2);
        assert!(!row.apply_result(1, Some(json!(true)), None));
        assert_eq!(row.value, json!(false));
        assert!(row.apply_result(2, Some(json!(true)), None));
        assert!(row.reset_visible());
        row.draft = Some("false".into());
        assert!(!row.reload(json!(false)));
        row.draft = None;
        assert!(row.reload(json!(false)));
    }

    /// Choice controls with an Other option remain dropdowns until text is selected.
    #[test]
    fn other_choice_control_tracks_stored_value() {
        let kind = Kind::Choice {
            options: vec![(json!("a"), "A".into()), (json!("b"), "B".into())],
            other: true,
        };
        assert_eq!(control_for(&kind), Control::Dropdown);
        assert_eq!(control_for_value(&kind, &json!("a")), Control::Dropdown);
        assert_eq!(control_for_value(&kind, &json!("custom")), Control::Text);
        let mut row = Row::new(
            "w",
            Setting::new("model", "Model", kind),
            json!("a"),
            json!("a"),
        );
        row.draft = Some(String::new());
        row.choice_text_mode = true;
        assert_eq!(row.control(), Control::Text);
    }

    /// Auto switches off to a described default or the inner kind's first value.
    #[test]
    fn auto_off_uses_default_then_inner_fallback() {
        let kind = Kind::Auto {
            word: "system".into(),
            label: "Follow Windows".into(),
            inner: Box::new(Kind::Number {
                min: 0.5,
                max: 2.0,
                step: 0.1,
                integer: false,
                unit: String::new(),
            }),
        };
        assert_eq!(control_for(&kind), Control::Auto);
        assert_eq!(
            auto_off_value(&kind, "system", &json!("system")),
            Some(json!(0.5))
        );
        assert_eq!(
            auto_off_value(&kind, "system", &json!(1.25)),
            Some(json!(1.25))
        );
    }

    /// Group headings appear once, in description order, and skip empty groups.
    #[test]
    fn group_headings_follow_description_order() {
        let settings = [
            Setting::new("a", "A", Kind::Switch).group("Display"),
            Setting::new("b", "B", Kind::Switch).group("Display"),
            Setting::new("c", "C", Kind::Switch),
            Setting::new("d", "D", Kind::Switch).group("Data"),
        ];
        let rows = settings
            .iter()
            .cloned()
            .map(|setting| Row::new("w", setting, Value::Null, Value::Null))
            .collect::<Vec<_>>();
        assert_eq!(group_heading(&rows, 0), Some("Display"));
        assert_eq!(group_heading(&rows, 1), None);
        assert_eq!(group_heading(&rows, 2), None);
        assert_eq!(group_heading(&rows, 3), Some("Data"));
    }
}
