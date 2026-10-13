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
    /// A popup list.
    Dropdown,
    /// A numeric slider.
    Slider,
    /// A bounded numeric editor.
    NumberBox,
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
    /// The text the pending write was made from; a draft typed after it survives the result.
    pub submitted: Option<String>,
    /// Validation or write error shown on this row.
    pub error: Option<String>,
    /// Whether user interaction is allowed.
    pub enabled: bool,
    /// Whether the described sibling-value condition currently allows this row to be shown.
    pub shown: bool,
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
            submitted: None,
            error: None,
            enabled: true,
            shown: true,
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
        let submitted = self.submitted.take();
        if let Some(value) = value {
            if self.draft == submitted {
                self.draft = None;
            }
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
    /// The widget described itself, so its page can open (a running problem keeps it openable:
    /// its page holds Stop / Restart).
    pub openable: bool,
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
    let startup_setting =
        Setting::new("startup", "Start with Windows", Kind::Switch).group("General");
    rows.push(Row::new(
        section,
        startup_setting,
        Value::Bool(startup),
        Value::Bool(false),
    ));
    for (key, label, fallback) in [
        ("show_on_taskbar", "Show on taskbar", true),
        ("compact", "Compact mode", false),
    ] {
        let setting = Setting::new(key, label, Kind::Switch).group("General");
        rows.push(Row::new(
            section,
            setting,
            current.get(key).cloned().unwrap_or(Value::Bool(fallback)),
            defaults.get(key).cloned().unwrap_or(Value::Bool(fallback)),
        ));
    }
    for setting in settings.iter().filter(|setting| {
        !setting.advanced && setting.key != "show_on_taskbar" && setting.key != "compact"
    }) {
        let value = taskbar_dock::describe::value_at(current, &setting.key)
            .cloned()
            .unwrap_or(Value::Null);
        let default = taskbar_dock::describe::value_at(defaults, &setting.key)
            .cloned()
            .unwrap_or(Value::Null);
        let mut row = Row::new(section, setting.clone(), value, default);
        row.shown = shown_when_matches(setting, current);
        rows.push(row);
    }
    rows
}

/// Builds every shared dock setting row and disables tint levels in fixed-tint mode.
pub fn dock_rows(current: &Value, defaults: &Value) -> Vec<Row> {
    let fixed = taskbar_dock::style::dock_settings();
    fixed
        .into_iter()
        .map(|setting| {
            let value = taskbar_dock::describe::value_at(current, &setting.key)
                .cloned()
                .unwrap_or(Value::Null);
            let default = taskbar_dock::describe::value_at(defaults, &setting.key)
                .cloned()
                .unwrap_or(Value::Null);
            let disabled =
                setting.key.starts_with("tint_dark.") || setting.key.starts_with("tint_light.");
            let mut row = Row::new("dock", setting, value, default);
            row.enabled = !disabled
                || current
                    .get("tint_fixed")
                    .is_none_or(|value| !value.is_number());
            row
        })
        .collect()
}

/// Builds Developer advanced rows in registry order, preserving each widget's section id.
pub fn advanced_rows(entries: &[crate::work::LoadedEntry], settings: &Value) -> Vec<Row> {
    entries
        .iter()
        .flat_map(|item| {
            let Ok(desc) = &item.describe else {
                return Vec::new();
            };
            desc.settings
                .iter()
                .filter(|setting| setting.advanced)
                .map(|setting| {
                    let mut setting = setting.clone();
                    setting.group = desc.title.clone();
                    let current = settings.get(&item.entry.name).cloned().unwrap_or_default();
                    let value = taskbar_dock::describe::value_at(&current, &setting.key)
                        .cloned()
                        .unwrap_or(Value::Null);
                    let default = taskbar_dock::describe::value_at(&desc.defaults, &setting.key)
                        .cloned()
                        .unwrap_or(Value::Null);
                    let mut row = Row::new(&item.entry.name, setting.clone(), value, default);
                    row.shown = shown_when_matches(&setting, &current);
                    row
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Evaluates a row's optional visibility rule against its own settings section.
pub fn shown_when_matches(setting: &Setting, section: &Value) -> bool {
    setting.shown_when.as_ref().is_none_or(|(key, expected)| {
        taskbar_dock::describe::value_at(section, key) == Some(expected)
    })
}

/// Returns the fixed dockctl command examples displayed on the Developer page.
pub fn developer_commands() -> Vec<&'static str> {
    vec![
        "dockctl weather state",
        "dockctl weather get city",
        "dockctl weather set city Amsterdam",
        "dockctl keys --dock",
        "dockctl weather startup get",
        "dockctl weather status",
        "dockctl weather start",
        "dockctl weather restart",
        "dockctl widgets list",
        "dockctl weather describe",
        "dockctl settings state",
    ]
}

/// A registry row prepared for the Developer page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredWidget {
    /// Registered section name.
    pub name: String,
    /// User-facing title, falling back to the section name.
    pub title: String,
    /// Executable path shown to the user.
    pub exe: String,
    /// Describe failure for a broken registry entry.
    pub error: Option<String>,
}

/// Builds visible registry rows without dropping entries whose describe failed.
pub fn registered_widgets(entries: &[crate::work::LoadedEntry]) -> Vec<RegisteredWidget> {
    entries
        .iter()
        .map(|item| RegisteredWidget {
            name: item.entry.name.clone(),
            title: item
                .describe
                .as_ref()
                .map(|desc| desc.title.clone())
                .unwrap_or_else(|_| item.entry.name.clone()),
            exe: item.entry.exe.to_string_lossy().into_owned(),
            error: item.describe.as_ref().err().cloned(),
        })
        .collect()
}

/// Masks raw settings using all described Secret keys and formats the result as JSON text.
pub fn masked_raw_settings(settings: &Value, entries: &[crate::work::LoadedEntry]) -> String {
    let secrets = entries
        .iter()
        .flat_map(|item| {
            item.describe
                .as_ref()
                .ok()
                .into_iter()
                .flat_map(|desc| desc.settings.iter())
                .filter(|setting| taskbar_dock::describe::is_secret_kind(&setting.kind))
                .map(|setting| format!("{}.{}", item.entry.name, setting.key))
        })
        .collect::<Vec<_>>();
    let masked = taskbar_dock::ctl::mask_with(settings, &secrets);
    serde_json::to_string_pretty(&masked).unwrap_or_default()
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
        Kind::Number { unit, integer, .. } => number_control(unit, *integer),
        Kind::Text { .. } => Control::Text,
        Kind::Color { .. } => Control::Color,
        Kind::Secret => Control::Secret,
        Kind::Choice { .. } => Control::Dropdown,
        // A slider can't show "no value", so an optional number is typed (empty box = null).
        Kind::Optional(inner) if matches!(**inner, Kind::Number { .. }) => Control::NumberBox,
        Kind::Optional(inner) => control_for(inner),
        Kind::Auto { .. } => Control::Auto,
    }
}

/// Selects the native number control from the unit and whole-number contract.
pub fn number_control(unit: &str, integer: bool) -> Control {
    if matches!(unit, "ms" | "s" | "min") || (integer && unit.is_empty()) {
        Control::NumberBox
    } else {
        Control::Slider
    }
}

/// Returns every setting key in one group, preserving the group's displayed order.
pub fn group_keys(rows: &[Row], group: &str) -> Vec<String> {
    rows.iter()
        .filter(|row| row.setting.group == group)
        .map(|row| row.key.clone())
        .collect()
}

/// Reports whether a Restore defaults action should be visible for a group.
pub fn group_modified(rows: &[Row], group: &str) -> bool {
    rows.iter()
        .any(|row| row.setting.group == group && row.key != "startup" && row.value != row.default)
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

/// The text a box shows for a saved value: strings as they are, null as empty, others as JSON.
pub fn display_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A slider's value text: as many decimals as its step has, trailing zeros dropped.
pub fn number_text(value: f64, step: f64) -> String {
    let step_text = format!("{step}");
    let decimals = step_text.split_once('.').map_or(0, |(_, d)| d.len());
    let text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
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

    /// A box shows strings bare, null as empty and other values as JSON.
    #[test]
    fn display_text_matches_what_a_box_shows() {
        assert_eq!(display_text(&json!("Utrecht")), "Utrecht");
        assert_eq!(display_text(&Value::Null), "");
        assert_eq!(display_text(&json!(52.09)), "52.09");
        assert_eq!(display_text(&json!(true)), "true");
    }

    /// Text typed while a write is pending survives that write's result; the submitted text
    /// itself is cleared.
    #[test]
    fn a_draft_typed_during_a_write_survives_its_result() {
        let text = Kind::Text {
            placeholder: String::new(),
            empty_ok: true,
        };
        let mut row = Row::new(
            "bus",
            Setting::new("dest", "Destination", text),
            json!(""),
            json!(""),
        );
        row.pending = Some(1);
        row.submitted = Some("Centraal".into());
        row.draft = Some("Centraal Station".into());
        assert!(row.apply_result(1, Some(json!("Centraal")), None));
        assert_eq!(row.value, json!("Centraal"));
        assert_eq!(row.draft.as_deref(), Some("Centraal Station"));
        row.pending = Some(2);
        row.submitted = Some("Centraal Station".into());
        assert!(row.apply_result(2, Some(json!("Centraal Station")), None));
        assert_eq!(row.draft, None);
    }

    /// A slider shows its value to its step's precision without float noise.
    #[test]
    fn number_text_uses_the_step_precision() {
        assert_eq!(number_text(12.0, 0.5), "12");
        assert_eq!(number_text(12.5, 0.5), "12.5");
        assert_eq!(number_text(1.0500000001, 0.05), "1.05");
        assert_eq!(number_text(3.0, 1.0), "3");
        assert_eq!(number_text(0.333, 0.01), "0.33");
    }

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
            ["startup", "show_on_taskbar", "compact", "one", "two"]
        );
        assert_eq!(rows[0].value, json!(true));
    }

    #[test]
    fn shown_when_matches_sibling_value_in_same_section() {
        let setting =
            Setting::new("color", "Colour", Kind::Switch).shown_when("highlight", json!("custom"));
        assert!(shown_when_matches(&setting, &json!({"highlight":"custom"})));
        assert!(!shown_when_matches(&setting, &json!({"highlight":"auto"})));
        assert!(!shown_when_matches(&setting, &json!({})));
        let legacy = Setting::new("always", "Always", Kind::Switch);
        assert!(shown_when_matches(&legacy, &json!({})));
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

    /// Dock rows cover all descriptions, preserve grouped nested ids, and disable tint levels.
    #[test]
    fn dock_rows_include_every_key_and_follow_fixed_tint() {
        let defaults = taskbar_dock::style::dock_defaults();
        let mut current = defaults.clone();
        current["tint_fixed"] = json!(0.4);
        let rows = dock_rows(&current, &defaults);
        let described = taskbar_dock::style::dock_settings();
        assert_eq!(rows.len(), described.len());
        assert_eq!(rows[0].setting.group, "Text");
        assert_eq!(
            rows.iter()
                .find(|row| row.key == "tint_dark.quiet")
                .unwrap()
                .section,
            "dock"
        );
        assert_eq!(
            rows.iter()
                .filter(|row| {
                    row.key.starts_with("tint_dark.") || row.key.starts_with("tint_light.")
                })
                .filter(|row| !row.enabled)
                .count(),
            6
        );
        current["tint_fixed"] = json!("auto");
        let enabled = dock_rows(&current, &defaults);
        assert!(enabled
            .iter()
            .filter(|row| {
                row.key.starts_with("tint_dark.") || row.key.starts_with("tint_light.")
            })
            .all(|row| row.enabled));
    }

    /// Developer models retain broken registry entries and mask every described secret.
    #[test]
    fn developer_rows_keep_advanced_broken_and_secret_data_safe() {
        let good = crate::work::LoadedEntry {
            entry: taskbar_dock::registry::Entry {
                name: "good".into(),
                exe: "good.exe".into(),
                class: "Good".into(),
            },
            describe: Ok(taskbar_dock::block::Describe {
                section: "good".into(),
                class: "Good".into(),
                title: "Good Widget".into(),
                description: "Description".into(),
                settings: vec![Setting::new("api_key", "API key", Kind::Secret).advanced()],
                defaults: json!({"api_key":""}),
                log_file: None,
            }),
        };
        let broken = crate::work::LoadedEntry {
            entry: taskbar_dock::registry::Entry {
                name: "broken".into(),
                exe: "broken.exe".into(),
                class: "Broken".into(),
            },
            describe: Err("missing describe output".into()),
        };
        let entries = [good, broken];
        let settings = json!({"good":{"api_key":"top-secret"}});
        let advanced = advanced_rows(&entries, &settings);
        assert_eq!(advanced.len(), 1);
        assert_eq!(
            format!("{}.{}", advanced[0].section, advanced[0].key),
            "good.api_key"
        );
        let registered = registered_widgets(&entries);
        assert_eq!(registered.len(), 2);
        assert_eq!(
            registered[1].error.as_deref(),
            Some("missing describe output")
        );
        let raw = masked_raw_settings(&settings, &entries);
        assert!(raw.contains("***"));
        assert!(!raw.contains("top-secret"));
    }

    #[test]
    fn number_control_mapping_covers_the_step12_values() {
        let cases = [
            ("min", false, Control::NumberBox),
            ("px", false, Control::Slider),
            ("s", false, Control::NumberBox),
            ("s", false, Control::NumberBox),
            ("", true, Control::NumberBox),
            ("", false, Control::Slider),
            ("ms", false, Control::NumberBox),
            ("", false, Control::Slider),
            ("px", false, Control::Slider),
            ("", false, Control::Slider),
        ];
        for (unit, integer, expected) in cases {
            assert_eq!(number_control(unit, integer), expected);
        }
        assert_eq!(
            control_for(&Kind::Optional(Box::new(Kind::Number {
                min: -90.0,
                max: 90.0,
                step: 0.1,
                integer: false,
                unit: String::new()
            }))),
            Control::NumberBox
        );
    }

    #[test]
    fn general_group_restore_keys_and_visibility_are_exact() {
        let rows = widget_rows("weather", &[], &json!({}), &json!({}), false);
        assert_eq!(
            group_keys(&rows, "General"),
            ["startup", "show_on_taskbar", "compact"]
        );
        assert!(!group_modified(&rows, "General"));
        let mut changed = rows;
        changed[0].value = json!(true);
        assert!(!group_modified(&changed, "General"));
        changed[2].value = json!(true);
        assert!(group_modified(&changed, "General"));
        assert_eq!(
            group_keys(&changed, "General"),
            ["startup", "show_on_taskbar", "compact"]
        );
    }
}
