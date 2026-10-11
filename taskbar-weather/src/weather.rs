//! Open-Meteo: geocoding, IP location (ip-api.com), the forecast request and its parsing into
//! `Weather`; also the shared HTTP agent.

use crate::config::Config;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Location {
    pub lat: f64,
    pub lon: f64,
    pub name: String,
}

pub struct Hour {
    pub time: String, // "HH:MM"
    pub temp: f64,
    pub code: i32,
    pub precip: Option<f64>,
}

pub struct Day {
    pub label: String, // "Today", "Fri", ...
    pub code: i32,
    pub tmax: f64,
    pub tmin: f64,
    pub precip: Option<f64>,
}

pub struct Weather {
    pub location: String,
    pub updated: String, // "HH:MM" local time at the location
    pub temp: f64,
    pub feels: f64,
    pub humidity: f64,
    pub wind: f64,
    pub code: i32,
    pub is_day: bool,
    pub fahrenheit: bool,
    pub hourly: Vec<Hour>,
    pub daily: Vec<Day>,
    /// Seconds to add to UTC for local time at the location.
    pub utc_offset: i64,
    /// Open-Meteo model the forecast came from ("best_match", "knmi_seamless", ...).
    pub model: String,
    /// "De Bilt 13:50" when the current values were measured by a KNMI station.
    pub observed: Option<String>,
    /// Why station measurements are missing, if they were requested.
    pub obs_error: Option<String>,
}

impl Weather {
    pub fn unit(&self) -> &'static str {
        if self.fahrenheit { "°F" } else { "°C" }
    }
    pub fn wind_unit(&self) -> &'static str {
        if self.fahrenheit { "mph" } else { "km/h" }
    }
    pub fn today_precip(&self) -> Option<f64> {
        self.daily.first().and_then(|d| d.precip)
    }
}

pub fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("taskbar-weather/", env!("CARGO_PKG_VERSION")))
        .build())
}

fn request(agent: &ureq::Agent, url: &str, auth: Option<&str>) -> Result<ureq::Response, String> {
    let mut req = agent.get(url);
    if let Some(key) = auth {
        req = req.set("Authorization", key);
    }
    req.call().map_err(|e| match e {
        ureq::Error::Status(401 | 403, _) if auth.is_some() => "API key rejected".to_string(),
        ureq::Error::Status(429, _) => "rate limited".to_string(),
        ureq::Error::Status(code, r) => {
            // Open-Meteo explains bad requests (e.g. an unknown model) in {"reason": ...}.
            let reason = r.into_json::<serde_json::Value>().ok()
                .and_then(|v| v["reason"].as_str().map(str::to_string));
            reason.unwrap_or_else(|| format!("HTTP {code}"))
        }
        ureq::Error::Transport(t) => format!("network error: {}", t.kind()),
    })
}

fn get<T: for<'de> Deserialize<'de>>(agent: &ureq::Agent, url: &str) -> Result<T, String> {
    request(agent, url, None)?.into_json::<T>().map_err(|e| format!("bad response: {e}"))
}

pub fn get_json_value(agent: &ureq::Agent, url: &str, auth: Option<&str>) -> Result<serde_json::Value, String> {
    request(agent, url, auth)?.into_json().map_err(|e| format!("bad response: {e}"))
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Deserialize)]
struct Geo {
    results: Option<Vec<GeoResult>>,
}

#[derive(Deserialize)]
struct GeoResult {
    name: String,
    #[serde(default)]
    admin1: Option<String>,
    latitude: f64,
    longitude: f64,
    country: Option<String>,
}

/// A place returned by Open-Meteo's geocoding service.
#[derive(Clone, Debug)]
pub struct Place {
    pub name: String,
    pub region: Option<String>,
    pub country: Option<String>,
    pub lat: f64,
    pub lon: f64,
}

impl Place {
    /// Builds the display name used by the weather forecast.
    pub fn label(&self) -> String {
        match &self.country {
            Some(country) if !country.is_empty() => format!("{}, {}", self.name, country),
            _ => self.name.clone(),
        }
    }
}

/// Searches Open-Meteo for places matching a name.
pub fn geocode(query: &str, count: u32) -> Result<Vec<Place>, String> {
    let agent = agent()?;
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?count={count}&format=json&name={}",
        encode(query)
    );
    let geo: Geo = get(&agent, &url)?;
    Ok(geo.results.unwrap_or_default().into_iter().map(|r| Place {
        name: r.name,
        region: r.admin1,
        country: r.country,
        lat: r.latitude,
        lon: r.longitude,
    }).collect())
}

/// Coordinates from config, else geocode the city, else approximate location from the IP address.
pub fn resolve_location(cfg: &Config) -> Result<Location, String> {
    if let (Some(lat), Some(lon)) = (cfg.latitude, cfg.longitude) {
        let name = if cfg.location_name.is_empty() { format!("{lat:.2}, {lon:.2}") } else { cfg.location_name.clone() };
        return Ok(Location { lat, lon, name });
    }
    let city = cfg.city.trim();
    if !city.is_empty() {
        let place = geocode(city, 1)?.into_iter().next().ok_or_else(|| format!("city \"{city}\" not found"))?;
        return Ok(Location { lat: place.lat, lon: place.lon, name: place.label() });
    }
    let agent = agent()?;
    #[derive(Deserialize)]
    struct Ip { status: String, lat: Option<f64>, lon: Option<f64>, city: Option<String>, country: Option<String> }
    let ip: Ip = get(&agent, "http://ip-api.com/json/?fields=status,lat,lon,city,country")?;
    match (ip.status.as_str(), ip.lat, ip.lon) {
        ("success", Some(lat), Some(lon)) => {
            let name = [ip.city, ip.country].into_iter().flatten().collect::<Vec<_>>().join(", ");
            Ok(Location { lat, lon, name })
        }
        _ => Err("could not detect location; set \"city\" in settings".into()),
    }
}

#[derive(Deserialize)]
struct Forecast {
    #[serde(default)]
    utc_offset_seconds: i64,
    current: Current,
    hourly: Hourly,
    daily: Daily,
}
#[derive(Deserialize)]
struct Current {
    time: String,
    temperature_2m: Option<f64>,
    apparent_temperature: Option<f64>,
    relative_humidity_2m: Option<f64>,
    wind_speed_10m: Option<f64>,
    weather_code: Option<f64>,
    is_day: Option<f64>,
}
#[derive(Deserialize)]
struct Hourly {
    time: Vec<String>,
    temperature_2m: Vec<Option<f64>>,
    weather_code: Vec<Option<f64>>,
    precipitation_probability: Vec<Option<f64>>,
}
#[derive(Deserialize)]
struct Daily {
    time: Vec<String>,
    weather_code: Vec<Option<f64>>,
    temperature_2m_max: Vec<Option<f64>>,
    temperature_2m_min: Vec<Option<f64>>,
    precipitation_probability_max: Vec<Option<f64>>,
}

pub fn fetch(loc: &Location, fahrenheit: bool, model: &str) -> Result<Weather, String> {
    let agent = agent()?;
    let units = if fahrenheit { "&temperature_unit=fahrenheit&wind_speed_unit=mph" } else { "" };
    let model = model.trim();
    let models = match model {
        "" | "best_match" => String::new(),
        m => format!("&models={}", encode(m)),
    };
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}\
         &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m,is_day\
         &hourly=temperature_2m,weather_code,precipitation_probability\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max\
         &forecast_days=5&timezone=auto{units}{models}",
        loc.lat, loc.lon
    );
    let f: Forecast = get(&agent, &url)?;
    let c = &f.current;

    let hourly = f.hourly.time.iter().enumerate()
        .filter(|(_, t)| t.as_str() > c.time.as_str())
        .take(6)
        .map(|(i, t)| Hour {
            time: t.get(11..16).unwrap_or(t).to_string(),
            temp: f.hourly.temperature_2m.get(i).copied().flatten().unwrap_or(f64::NAN),
            code: f.hourly.weather_code.get(i).copied().flatten().unwrap_or(-1.0) as i32,
            precip: f.hourly.precipitation_probability.get(i).copied().flatten(),
        })
        .collect();

    let daily = f.daily.time.iter().enumerate()
        .map(|(i, date)| Day {
            label: if i == 0 { "Today".into() } else { weekday_name(date).into() },
            code: f.daily.weather_code.get(i).copied().flatten().unwrap_or(-1.0) as i32,
            tmax: f.daily.temperature_2m_max.get(i).copied().flatten().unwrap_or(f64::NAN),
            tmin: f.daily.temperature_2m_min.get(i).copied().flatten().unwrap_or(f64::NAN),
            precip: f.daily.precipitation_probability_max.get(i).copied().flatten(),
        })
        .collect();

    Ok(Weather {
        location: loc.name.clone(),
        updated: c.time.get(11..16).unwrap_or("").to_string(),
        temp: c.temperature_2m.unwrap_or(f64::NAN),
        feels: c.apparent_temperature.unwrap_or(f64::NAN),
        humidity: c.relative_humidity_2m.unwrap_or(f64::NAN),
        wind: c.wind_speed_10m.unwrap_or(f64::NAN),
        code: c.weather_code.unwrap_or(-1.0) as i32,
        is_day: c.is_day.unwrap_or(1.0) >= 0.5,
        fahrenheit,
        hourly,
        daily,
        utc_offset: f.utc_offset_seconds,
        model: if model.is_empty() { "best_match".into() } else { model.to_string() },
        observed: None,
        obs_error: None,
    })
}

/// "2026-10-08" -> "Thu"
fn weekday_name(date: &str) -> &'static str {
    let mut parts = date.split('-').filter_map(|p| p.parse::<i64>().ok());
    let (Some(mut y), Some(m), Some(d)) = (parts.next(), parts.next(), parts.next()) else { return "" };
    const T: [i64; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    if m < 3 { y -= 1; }
    let w = (y + y / 4 - y / 100 + y / 400 + T[((m - 1) % 12) as usize] + d).rem_euclid(7);
    ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][w as usize]
}

/// WMO weather code -> (emoji glyph, description).
pub fn describe(code: i32, is_day: bool) -> (&'static str, &'static str) {
    match code {
        0 => (if is_day { "☀" } else { "☾" }, "Clear"),
        1 => (if is_day { "🌤" } else { "☾" }, "Mostly clear"),
        2 => ("⛅", "Partly cloudy"),
        3 => ("☁", "Cloudy"),
        45 | 48 => ("🌫", "Fog"),
        51 | 53 | 55 => ("🌦", "Drizzle"),
        56 | 57 => ("🌧", "Freezing drizzle"),
        61 => ("🌧", "Light rain"),
        63 => ("🌧", "Rain"),
        65 => ("🌧", "Heavy rain"),
        66 | 67 => ("🌧", "Freezing rain"),
        71 | 73 | 75 | 77 => ("🌨", "Snow"),
        80 => ("🌦", "Light showers"),
        81 => ("🌦", "Showers"),
        82 => ("🌧", "Heavy showers"),
        85 | 86 => ("🌨", "Snow showers"),
        95 => ("⛈", "Thunderstorm"),
        96 | 99 => ("⛈", "Thunderstorm, hail"),
        _ => ("·", "Unknown"),
    }
}

/// Formats a temperature rounded to whole degrees ("31°"), or "--" when missing.
pub fn deg(t: f64) -> String {
    if t.is_nan() { "--".into() } else { format!("{}°", t.round() as i64) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weekdays() {
        assert_eq!(weekday_name("2026-10-08"), "Thu");
        assert_eq!(weekday_name("2024-02-29"), "Thu");
        assert_eq!(weekday_name("2000-01-01"), "Sat");
    }
    #[test]
    fn encoding() {
        assert_eq!(encode("São Paulo"), "S%C3%A3o%20Paulo");
    }

    #[test]
    fn place_label_includes_country_when_present() {
        let place = Place { name: "Bandung".into(), region: Some("West Java".into()), country: Some("Indonesia".into()), lat: -6.9, lon: 107.6 };
        assert_eq!(place.label(), "Bandung, Indonesia");
        let without_country = Place { country: None, ..place };
        assert_eq!(without_country.label(), "Bandung");
    }
}
