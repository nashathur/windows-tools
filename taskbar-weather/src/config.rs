//! The `weather` settings section.

use serde::{Deserialize, Serialize};
use serde_json::json;
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
    /// Where the taskbar label goes: "auto" (right side when taskbar icons are
    /// left-aligned, else left side), "left" or "right".
    pub label_position: String,
    // `compact` (temperature over humidity) and `show_on_taskbar` are in the same section but
    // Block's (taskbar_dock::block): `save` leaves them alone.
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
            label_position: "auto".into(),
        }
    }
}

impl Config {
    /// Loads the weather section, filling only missing keys on startup.
    pub fn load() -> Config {
        let defaults = serde_json::to_value(Config::default()).unwrap_or_default();
        let _ = settings::fill_missing("weather", &defaults);
        Config::read()
    }

    /// Reads the weather section without writing it. A broken file uses defaults.
    pub fn read() -> Config {
        let Ok(root) = settings::read() else { return Config::default() };
        root.get("weather").and_then(|w| serde_json::from_value(w.clone()).ok()).unwrap_or_default()
    }

    /// Saves only the four location keys (city, latitude, longitude, location_name), in one
    /// update: the other keys keep what the file has, even if this copy of them is older.
    pub fn save_location(&self) -> std::io::Result<()> {
        settings::update(|root| {
            if !root.is_object() {
                *root = json!({});
            }
            let sec = &mut root["weather"];
            if !sec.is_object() {
                *sec = json!({});
            }
            sec["city"] = json!(self.city);
            sec["latitude"] = json!(self.latitude);
            sec["longitude"] = json!(self.longitude);
            sec["location_name"] = json!(self.location_name);
        })
    }

    pub fn fahrenheit(&self) -> bool {
        self.units.eq_ignore_ascii_case("fahrenheit") || self.units.eq_ignore_ascii_case("f")
    }

    /// Key that changes whenever the location settings change.
    pub fn location_key(&self) -> String {
        format!("{}|{:?}|{:?}", self.city.trim(), self.latitude, self.longitude)
    }
}

