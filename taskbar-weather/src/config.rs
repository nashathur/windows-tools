use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use taskbar_dock::settings;

/// Weather settings: the "weather" section of the settings file shared with the other taskbar
/// widgets, %APPDATA%\TaskbarDock\settings.json (see the taskbar-dock library). The shared look
/// and behaviour (gap, font, timings, "follow Windows" switches) is its "dock" section, read
/// through taskbar_dock::style.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Config {
    /// City to look up with Open-Meteo geocoding, e.g. "Jakarta". Empty = detect by IP.
    pub city: String,
    /// Exact coordinates; when both are set they win over `city`.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Display name used together with coordinates.
    pub location_name: String,
    /// "celsius" or "fahrenheit".
    pub units: String,
    pub refresh_minutes: u32,
    /// Open-Meteo forecast model: "best_match", "knmi_seamless" (KNMI HARMONIE, then ECMWF),
    /// "ecmwf_ifs025", "icon_seamless", ... See https://open-meteo.com/en/docs.
    pub model: String,
    /// KNMI Data Platform EDR API key. When set, current temperature, humidity and wind
    /// are measured values from the nearest KNMI station (Netherlands only).
    pub knmi_api_key: String,
    /// KNMI station id, e.g. "06260" (De Bilt). Empty = nearest station within 50 km.
    pub knmi_station: String,
    /// Show the label embedded in the taskbar (in addition to the tray icon).
    pub taskbar_label: bool,
    /// Where the taskbar label goes: "auto" (right side when taskbar icons are
    /// left-aligned, else left side), "left" or "right".
    pub label_position: String,
    // `compact` (temperature over humidity) is in the same section but Block's
    // (taskbar_dock::block): `save` leaves it alone.
}

impl Default for Config {
    fn default() -> Self {
        Config {
            city: String::new(),
            latitude: None,
            longitude: None,
            location_name: String::new(),
            units: "celsius".into(),
            refresh_minutes: 15,
            model: "best_match".into(),
            knmi_api_key: String::new(),
            knmi_station: String::new(),
            taskbar_label: true,
            label_position: "auto".into(),
        }
    }
}

/// The old per-app file, migrated into the shared one on first run.
fn old_path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into());
    base.join("TaskbarWeather").join("config.json")
}

impl Config {
    /// Loads the weather section. On first run it moves the old
    /// %APPDATA%\TaskbarWeather\config.json in (kept as config.json.migrated) or writes defaults.
    pub fn load() -> Config {
        // Keep a broken file as-is so the user can fix their edit.
        let Some(root) = settings::read() else { return Config::default() };
        let cfg: Config = match root.get("weather") {
            Some(w) => serde_json::from_value(w.clone()).unwrap_or_default(),
            None => {
                // first run with the shared file: take over the old weather config if there is one
                let old: Option<Value> = std::fs::read_to_string(old_path()).ok().and_then(|t| serde_json::from_str(&t).ok());
                let cfg = old.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
                if old_path().exists() {
                    let _ = std::fs::rename(old_path(), old_path().with_extension("json.migrated"));
                }
                cfg
            }
        };
        // write the section back in full: adds settings introduced by newer versions (no write
        // if nothing changed)
        cfg.save();
        cfg
    }

    /// Saves these settings into the weather section; other keys there (Block's `compact`) and
    /// the other sections are left as they are. No write if nothing changed.
    pub fn save(&self) {
        let Ok(Value::Object(mine)) = serde_json::to_value(self) else { return };
        let _ = settings::update(|root| {
            if !root.is_object() {
                *root = json!({});
            }
            let sec = &mut root["weather"];
            if !sec.is_object() {
                *sec = json!({});
            }
            for (k, v) in mine {
                sec[k.as_str()] = v;
            }
        });
    }

    pub fn fahrenheit(&self) -> bool {
        self.units.eq_ignore_ascii_case("fahrenheit") || self.units.eq_ignore_ascii_case("f")
    }

    /// Key that changes whenever the location settings change.
    pub fn location_key(&self) -> String {
        format!("{}|{:?}|{:?}", self.city.trim(), self.latitude, self.longitude)
    }
}
