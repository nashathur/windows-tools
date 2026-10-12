//! Setting descriptions shared by widgets, dockctl and the settings window.

use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
/// A user-facing description of one setting.
pub struct Setting {
    /// The settings key; dots address nested leaves.
    pub key: String,
    /// The short label shown beside the control.
    pub label: String,
    /// Optional one-line help shown below the label.
    pub help: String,
    /// Optional heading used to group related controls.
    pub group: String,
    /// The setting's type and validation rules.
    pub kind: Kind,
    /// Whether this setting belongs in the Developer tab.
    pub advanced: bool,
}

#[derive(Clone, Debug, PartialEq)]
/// The input type and validation rules for a setting.
pub enum Kind {
    /// A true/false value.
    Switch,
    /// A number constrained to the given range.
    Number {
        min: f64,
        max: f64,
        step: f64,
        integer: bool,
        unit: String,
    },
    /// Free text with a required or optional value.
    Text { placeholder: String, empty_ok: bool },
    /// One of the listed values, optionally allowing another string.
    Choice {
        options: Vec<(Value, String)>,
        other: bool,
    },
    /// A six-digit hexadecimal colour.
    Color { hash: bool, empty_ok: bool },
    /// A string value that must be masked when displayed.
    Secret,
    /// A value that may be null.
    Optional(Box<Kind>),
    /// A special word selecting a system value, or an explicit value.
    Auto {
        word: String,
        label: String,
        inner: Box<Kind>,
    },
}

/// Returns whether a setting kind stores a secret, including wrapped kinds.
pub fn is_secret_kind(kind: &Kind) -> bool {
    match kind {
        Kind::Secret => true,
        Kind::Optional(inner) | Kind::Auto { inner, .. } => is_secret_kind(inner),
        _ => false,
    }
}

impl Setting {
    /// Creates a setting with empty help and group labels.
    pub fn new(key: impl Into<String>, label: impl Into<String>, kind: Kind) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            help: String::new(),
            group: String::new(),
            kind,
            advanced: false,
        }
    }
    /// Creates a floating-point number setting.
    pub fn number(
        key: impl Into<String>,
        label: impl Into<String>,
        min: f64,
        max: f64,
        step: f64,
    ) -> Self {
        Self::new(
            key,
            label,
            Kind::Number {
                min,
                max,
                step,
                integer: false,
                unit: String::new(),
            },
        )
    }
    /// Adds explanatory help text.
    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = help.into();
        self
    }
    /// Adds a group heading.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = group.into();
        self
    }
    /// Marks this setting as advanced.
    pub fn advanced(mut self) -> Self {
        self.advanced = true;
        self
    }
    /// Requires a numeric value to be an integer.
    pub fn integer(mut self) -> Self {
        if let Kind::Number { integer, .. } = &mut self.kind {
            *integer = true;
        }
        self
    }
    /// Sets the unit shown beside a numeric value.
    pub fn unit(mut self, unit: impl Into<String>) -> Self {
        if let Kind::Number { unit: target, .. } = &mut self.kind {
            *target = unit.into();
        }
        self
    }
}

/// Checks and canonicalizes a value against a setting.
pub fn check(s: &Setting, v: &Value) -> Result<Value, String> {
    check_kind(&s.kind, v)
}

/// Checks a value against a kind, including wrapped kinds.
fn check_kind(kind: &Kind, v: &Value) -> Result<Value, String> {
    match kind {
        Kind::Switch => {
            if v.is_boolean() {
                Ok(v.clone())
            } else {
                Err("must be on or off".into())
            }
        }
        Kind::Number {
            min, max, integer, ..
        } => {
            let n = v.as_f64().ok_or_else(|| "not a number".to_string())?;
            if n < *min || n > *max {
                return Err(format!(
                    "must be between {} and {}",
                    short(*min),
                    short(*max)
                ));
            }
            if *integer {
                if n.fract() != 0.0 {
                    return Err("must be a whole number".into());
                }
                if n >= 0.0 {
                    Ok(json!(n as u64))
                } else {
                    Ok(json!(n as i64))
                }
            } else {
                Ok(v.clone())
            }
        }
        Kind::Text { empty_ok, .. } => {
            let x = v.as_str().ok_or_else(|| "must be text".to_string())?;
            if x.is_empty() && !empty_ok {
                Err("must not be empty".into())
            } else {
                Ok(v.clone())
            }
        }
        Kind::Secret => {
            if v.is_string() {
                Ok(v.clone())
            } else {
                Err("must be text".into())
            }
        }
        Kind::Color { hash, empty_ok } => {
            let x = v.as_str().ok_or_else(|| "must be a colour".to_string())?;
            if x.is_empty() && *empty_ok {
                return Ok(v.clone());
            }
            let digits = if *hash {
                x.strip_prefix('#').unwrap_or(x)
            } else {
                x
            };
            if digits.len() != 6
                || !digits.bytes().all(|b| b.is_ascii_hexdigit())
                || (*hash && (!x.starts_with('#') || x.len() != 7))
                || (!*hash && x.starts_with('#'))
            {
                return Err(if *hash {
                    "must be 6 hex digits like #1E90FF"
                } else {
                    "must be 6 hex digits like 1E90FF"
                }
                .into());
            }
            Ok(v.clone())
        }
        Kind::Choice { options, other } => {
            if options.iter().any(|(x, _)| x == v) {
                return Ok(v.clone());
            }
            if *other && v.as_str().is_some_and(|s| !s.is_empty()) {
                return Ok(v.clone());
            }
            let choices = options
                .iter()
                .map(|(value, _)| value.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!("not one of: {choices}"))
        }
        Kind::Optional(inner) => {
            if v.is_null() {
                Ok(Value::Null)
            } else {
                check_kind(inner, v)
            }
        }
        Kind::Auto { word, inner, .. } => {
            if v.as_str().is_some_and(|x| x.eq_ignore_ascii_case(word)) {
                Ok(json!(word))
            } else {
                check_kind(inner, v)
            }
        }
    }
}

/// Formats integral range bounds without a decimal suffix.
fn short(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{n:.0}")
    } else {
        n.to_string()
    }
}

/// Parses user-entered text and validates the resulting value.
pub fn parse(s: &Setting, input: &str) -> Result<Value, String> {
    let v = parse_kind(&s.kind, input)?;
    check(s, &v)
}

/// Parses text according to a setting kind before validation.
fn parse_kind(kind: &Kind, input: &str) -> Result<Value, String> {
    match kind {
        Kind::Text { .. } | Kind::Secret | Kind::Color { .. } => {
            Ok(json!(json_string_or_text(input)))
        }
        Kind::Number { .. } => serde_json::from_str::<Value>(input)
            .ok()
            .filter(Value::is_number)
            .ok_or_else(|| "not a number".into()),
        Kind::Switch => {
            if let Ok(v) = serde_json::from_str::<Value>(input) {
                if v.is_boolean() {
                    return Ok(v);
                }
            }
            match input.to_ascii_lowercase().as_str() {
                "on" | "yes" => Ok(json!(true)),
                "off" | "no" => Ok(json!(false)),
                _ => Err("must be on or off".into()),
            }
        }
        Kind::Choice { options, other } => {
            if let Some((v, _)) = options.iter().find(|(v, _)| {
                v.as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(input))
            }) {
                return Ok(v.clone());
            }
            if let Some((v, _)) = options
                .iter()
                .find(|(_, label)| label.eq_ignore_ascii_case(input))
            {
                return Ok(v.clone());
            }
            if let Ok(v) = serde_json::from_str::<Value>(input) {
                return Ok(v);
            }
            if *other && !input.is_empty() {
                return Ok(json!(input));
            }
            Err(format!(
                "not one of: {}",
                options
                    .iter()
                    .map(|(_, l)| l.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
        Kind::Optional(inner) => {
            if input.trim().is_empty() || input.trim().eq_ignore_ascii_case("null") {
                Ok(Value::Null)
            } else {
                parse_kind(inner, input)
            }
        }
        Kind::Auto { word, inner, .. } => {
            if input.eq_ignore_ascii_case(word) {
                Ok(json!(word))
            } else {
                parse_kind(inner, input)
            }
        }
    }
}

/// Unquotes valid JSON strings and otherwise preserves the entered text.
fn json_string_or_text(input: &str) -> String {
    if input.starts_with('"') {
        serde_json::from_str::<String>(input).unwrap_or_else(|_| input.to_string())
    } else {
        input.to_string()
    }
}

/// Serializes setting descriptions to JSON.
pub fn to_json(list: &[Setting]) -> Value {
    Value::Array(list.iter().map(setting_json).collect())
}
/// Converts one setting description to JSON.
fn setting_json(s: &Setting) -> Value {
    json!({
        "key": s.key,
        "label": s.label,
        "help": s.help,
        "group": s.group,
        "advanced": s.advanced,
        "kind": kind_json(&s.kind),
    })
}
/// Converts one setting kind to JSON.
fn kind_json(k: &Kind) -> Value {
    match k {
        Kind::Switch => json!({"type":"switch"}),
        Kind::Number {
            min,
            max,
            step,
            integer,
            unit,
        } => json!({
            "type": "number",
            "min": min,
            "max": max,
            "step": step,
            "integer": integer,
            "unit": unit,
        }),
        Kind::Text {
            placeholder,
            empty_ok,
        } => json!({
            "type": "text",
            "placeholder": placeholder,
            "empty_ok": empty_ok,
        }),
        Kind::Choice { options, other } => {
            json!({
                "type": "choice",
                "options": options.iter().map(|(value, label)| json!([value, label])).collect::<Vec<_>>(),
                "other": other,
            })
        }
        Kind::Color { hash, empty_ok } => json!({
            "type": "color",
            "hash": hash,
            "empty_ok": empty_ok,
        }),
        Kind::Secret => json!({"type":"secret"}),
        Kind::Optional(inner) => json!({
            "type": "optional",
            "inner": kind_json(inner),
        }),
        Kind::Auto { word, label, inner } => {
            json!({
                "type": "auto",
                "word": word,
                "label": label,
                "inner": kind_json(inner),
            })
        }
    }
}
/// Parses setting descriptions from JSON.
pub fn from_json(v: &Value) -> Result<Vec<Setting>, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| "settings must be an array".to_string())?;
    arr.iter()
        .map(|x| {
            Ok(Setting {
                key: field(x, "key")?,
                label: field(x, "label")?,
                help: field(x, "help")?,
                group: field(x, "group")?,
                advanced: x
                    .get("advanced")
                    .and_then(Value::as_bool)
                    .ok_or("advanced must be a boolean")?,
                kind: parse_kind_json(x.get("kind").ok_or("missing kind")?)?,
            })
        })
        .collect()
}
/// Reads a required string field from a JSON object.
fn field(v: &Value, k: &str) -> Result<String, String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{k} must be text"))
}
/// Parses one setting kind from JSON.
fn parse_kind_json(v: &Value) -> Result<Kind, String> {
    let t = field(v, "type")?;
    Ok(match t.as_str() {
        "switch" => Kind::Switch,
        "number" => Kind::Number {
            min: v["min"].as_f64().ok_or("number min missing")?,
            max: v["max"].as_f64().ok_or("number max missing")?,
            step: v["step"].as_f64().ok_or("number step missing")?,
            integer: v["integer"].as_bool().ok_or("number integer missing")?,
            unit: field(v, "unit")?,
        },
        "text" => Kind::Text {
            placeholder: field(v, "placeholder")?,
            empty_ok: v["empty_ok"].as_bool().ok_or("empty_ok missing")?,
        },
        "choice" => Kind::Choice {
            options: v["options"]
                .as_array()
                .ok_or("choice options missing")?
                .iter()
                .map(|p| {
                    Ok((
                        p.get(0).cloned().ok_or("choice value missing")?,
                        p.get(1)
                            .and_then(Value::as_str)
                            .ok_or("choice label missing")?
                            .to_string(),
                    ))
                })
                .collect::<Result<_, String>>()?,
            other: v["other"].as_bool().ok_or("choice other missing")?,
        },
        "color" => Kind::Color {
            hash: v["hash"].as_bool().ok_or("color hash missing")?,
            empty_ok: v["empty_ok"].as_bool().ok_or("empty_ok missing")?,
        },
        "secret" => Kind::Secret,
        "optional" => Kind::Optional(Box::new(parse_kind_json(&v["inner"])?)),
        "auto" => Kind::Auto {
            word: field(v, "word")?,
            label: field(v, "label")?,
            inner: Box::new(parse_kind_json(&v["inner"])?),
        },
        _ => return Err(format!("unknown setting kind {t:?}")),
    })
}

/// Looks up a setting value by a dotted key path.
pub fn value_at<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.').try_fold(root, |v, k| v.get(k))
}

/// Confirms that a widget description round-trips and exactly matches its defaults.
pub fn validate(describe: &crate::block::Describe) -> Result<(), String> {
    let round_trip = crate::block::Describe::from_json(&describe.to_json())?;
    if round_trip != *describe {
        return Err("description changed during JSON round-trip".into());
    }

    let setting_keys: std::collections::HashSet<_> = describe
        .settings
        .iter()
        .map(|setting| setting.key.as_str())
        .collect();
    for setting in &describe.settings {
        let value = value_at(&describe.defaults, &setting.key)
            .ok_or_else(|| format!("default missing for {}", setting.key))?;
        if check(setting, value)? != *value {
            return Err(format!(
                "default changed during validation for {}",
                setting.key
            ));
        }
    }

    /// Collects leaf paths in a defaults object.
    fn leaves(value: &Value, prefix: String, output: &mut Vec<String>) {
        if let Some(object) = value.as_object() {
            for (key, value) in object {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                leaves(value, path, output);
            }
        } else {
            output.push(prefix);
        }
    }

    let mut default_keys = Vec::new();
    leaves(&describe.defaults, String::new(), &mut default_keys);
    for key in default_keys {
        if !setting_keys.contains(key.as_str()) {
            return Err(format!("default has no setting description: {key}"));
        }
    }
    Ok(())
}

/// Writes a value at a dotted key path, creating objects for intermediate parts.
pub fn set_at(root: &mut Value, key: &str, value: Value) {
    let mut parts = key.split('.').peekable();
    let mut cur = root;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            cur[part] = value;
            break;
        }
        if !cur.get(part).is_some_and(Value::is_object) {
            cur[part] = json!({});
        }
        cur = &mut cur[part];
    }
}

/// The full path of an exe, resolved like `fs::canonicalize` but without its `\\?\` prefix
/// (the form stored in settings.json and shown to the user).
pub fn canonical_exe(exe: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let canonical =
        std::fs::canonicalize(exe).map_err(|error| format!("{}: {error}", exe.display()))?;
    let full_path = canonical.to_string_lossy();
    Ok(std::path::PathBuf::from(
        full_path.strip_prefix("\\\\?\\").unwrap_or(&full_path),
    ))
}

/// Gets a validated description from the executable cache.
pub fn cached(exe: &std::path::Path) -> Result<crate::block::Describe, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    let canonical = canonical_exe(exe)?;
    let display = canonical.to_string_lossy().to_string();
    let metadata = std::fs::metadata(&canonical).map_err(|error| error.to_string())?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs());
    let mut hash = 0xcbf29ce484222325u64;
    for byte in display.to_lowercase().bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let stem = canonical
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("widget");
    let appdata = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let cache = appdata
        .join("TaskbarDock")
        .join("describe")
        .join(format!("{stem}-{hash:016x}.json"));
    let key = json!({
        "exe": display,
        "size": metadata.len(),
        "mtime": modified,
        "version": crate::block::DESCRIBE_VERSION,
    });

    if let Ok(bytes) = std::fs::read(&cache) {
        if let Ok(envelope) = serde_json::from_slice::<Value>(&bytes) {
            if envelope.get("key") == Some(&key) {
                if let Ok(describe) = crate::block::Describe::from_json(&envelope["describe"]) {
                    return Ok(describe);
                }
            }
        }
    }

    let temporary = std::env::temp_dir().join(format!(
        "taskbar-dock-describe-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut command = std::process::Command::new(&canonical);
    command
        .arg("--describe")
        .arg(&temporary)
        .current_dir(canonical.parent().unwrap_or(std::path::Path::new(".")));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("couldn't run --describe: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child
            .try_wait()
            .map_err(|error| format!("waiting for --describe failed: {error}"))?
        {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&temporary);
                return Err("didn't answer --describe within 5 s".into());
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    if !status.success() {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "--describe exited with code {}",
            status
                .code()
                .map_or("unknown".into(), |code| code.to_string())
        ));
    }

    let result = (|| {
        let bytes = std::fs::read(&temporary)
            .map_err(|error| format!("couldn't read --describe output: {error}"))?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid --describe JSON: {error}"))?;
        let describe = crate::block::Describe::from_json(&value)?;
        std::fs::create_dir_all(cache.parent().unwrap()).map_err(|error| error.to_string())?;
        let envelope = json!({ "key": key, "describe": value });
        let contents =
            serde_json::to_string_pretty(&envelope).map_err(|error| error.to_string())?;
        crate::settings::write_atomic(&cache, &(contents + "\n"))
            .map_err(|error| error.to_string())?;
        Ok(describe)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    /// Checks every kind's JSON, parsing and default validation.
    fn every_kind_round_trips_checks_and_parses() {
        let s = Setting::new(
            "n",
            "Number",
            Kind::Number {
                min: 6.,
                max: 48.,
                step: 1.,
                integer: true,
                unit: "px".into(),
            },
        );
        assert_eq!(from_json(&to_json(&[s.clone()])).unwrap(), vec![s.clone()]);
        assert_eq!(parse(&s, "7"), Ok(json!(7)));
        assert_eq!(parse(&s, "5"), Err("must be between 6 and 48".into()));
        let t = Setting::new(
            "id",
            "ID",
            Kind::Text {
                placeholder: String::new(),
                empty_ok: false,
            },
        );
        assert_eq!(parse(&t, "59150850"), Ok(json!("59150850")));
        assert_eq!(parse(&t, r#""foo""#), Ok(json!("foo")));
        let cases = vec![
            (
                Setting::new("sw", "Switch", Kind::Switch),
                json!(true),
                "yes",
            ),
            (
                Setting::new("secret", "Secret", Kind::Secret),
                json!("token"),
                "token",
            ),
            (
                Setting::new(
                    "color",
                    "Colour",
                    Kind::Color {
                        hash: true,
                        empty_ok: false,
                    },
                ),
                json!("#1E90FF"),
                "#1E90FF",
            ),
            (
                Setting::new(
                    "choice",
                    "Choice",
                    Kind::Choice {
                        options: vec![(json!("system"), "Follow".into())],
                        other: false,
                    },
                ),
                json!("system"),
                "Follow",
            ),
            (
                Setting::new(
                    "optional",
                    "Optional",
                    Kind::Optional(Box::new(Kind::Number {
                        min: 0.,
                        max: 2.,
                        step: 0.5,
                        integer: false,
                        unit: String::new(),
                    })),
                ),
                Value::Null,
                "",
            ),
            (
                Setting::new(
                    "auto",
                    "Auto",
                    Kind::Auto {
                        word: "auto".into(),
                        label: "Automatic".into(),
                        inner: Box::new(Kind::Number {
                            min: 0.,
                            max: 2.,
                            step: 0.5,
                            integer: false,
                            unit: String::new(),
                        }),
                    },
                ),
                json!("auto"),
                "auto",
            ),
        ];
        for (setting, default, input) in cases {
            assert_eq!(check(&setting, &default).unwrap(), default);
            assert_eq!(parse(&setting, input).unwrap(), default);
            assert_eq!(
                from_json(&to_json(&[setting.clone()])).unwrap(),
                vec![setting]
            );
        }
    }

    #[test]
    fn secret_predicate_follows_wrappers() {
        assert!(is_secret_kind(&Kind::Secret));
        assert!(is_secret_kind(&Kind::Optional(Box::new(Kind::Secret))));
        assert!(is_secret_kind(&Kind::Auto {
            word: "system".into(),
            label: "Follow Windows".into(),
            inner: Box::new(Kind::Secret),
        }));
        assert!(!is_secret_kind(&Kind::Optional(Box::new(Kind::Text {
            placeholder: String::new(),
            empty_ok: true,
        }))));
    }

    #[test]
    fn choice_parsing_matches_string_values_before_labels() {
        let position = Setting::new(
            "label_position",
            "Label position",
            Kind::Choice {
                options: vec![
                    (json!("left"), "Left".into()),
                    (json!("auto"), "Automatic".into()),
                ],
                other: false,
            },
        );
        assert_eq!(parse(&position, "AUTO"), Ok(json!("auto")));
        assert_eq!(parse(&position, "Automatic"), Ok(json!("auto")));

        let animations = Setting::new(
            "animations",
            "Animations",
            Kind::Choice {
                options: vec![
                    (json!("system"), "Follow Windows".into()),
                    (json!(true), "On".into()),
                    (json!(false), "Off".into()),
                ],
                other: false,
            },
        );
        assert_eq!(parse(&animations, "system"), Ok(json!("system")));
        assert_eq!(parse(&animations, "On"), Ok(json!(true)));
        assert_eq!(parse(&animations, "true"), Ok(json!(true)));
    }

    #[test]
    /// Checks dock descriptions against every dock default leaf.
    fn dock_descriptions_and_defaults_match_both_ways() {
        let defaults = crate::style::dock_defaults();
        let settings = crate::style::dock_settings();
        let names: std::collections::HashSet<_> = settings.iter().map(|s| s.key.as_str()).collect();
        for s in &settings {
            let value =
                value_at(&defaults, &s.key).unwrap_or_else(|| panic!("missing dock key {}", s.key));
            assert_eq!(check(s, value).unwrap(), *value, "{}", s.key);
        }
        /// Collects the leaf paths in dock defaults.
        fn leaves(v: &Value, prefix: String, out: &mut Vec<String>) {
            if let Some(m) = v.as_object() {
                for (k, v) in m {
                    leaves(
                        v,
                        if prefix.is_empty() {
                            k.clone()
                        } else {
                            format!("{prefix}.{k}")
                        },
                        out,
                    );
                }
            } else {
                out.push(prefix);
            }
        }
        let mut all = Vec::new();
        leaves(&defaults, String::new(), &mut all);
        for key in all {
            assert!(
                names.contains(key.as_str()),
                "missing description for {key}"
            );
        }
    }
}
