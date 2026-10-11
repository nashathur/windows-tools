//! The weather widget's dockctl `do` commands.

use super::*;
use serde_json::{json, Value};
use std::time::{Duration, Instant, SystemTime};
use taskbar_dock::block::{CommandError, CommandReply, Ctx, ReplyToken};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

const PLACE_LIST_TTL: Duration = Duration::from_secs(10 * 60);

/// A geocoding result waiting for the UI thread to update the place list and answer dockctl.
pub(crate) struct PlacesResult {
    query: String,
    token: ReplyToken,
    result: Result<Vec<Place>, String>,
}

/// One parsed form of a `place` command argument.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PlaceArg {
    Auto,
    Coordinates { lat: f64, lon: f64 },
    Index(usize),
}

/// Lists the weather commands and their short usage strings.
pub(crate) fn commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("now", "now: place, temperature, conditions, last update"),
        ("places", "places <query>: up to 5 places matching the name (Open-Meteo geocoding)"),
        ("place", "place <n | lat,lon | auto>: use place n from the last places reply, a coordinate, or detect by IP"),
    ]
}

/// Runs one weather command; network work goes to a thread, so the UI thread never waits.
pub(crate) fn command(a: &mut App, name: &str, args: &[String], token: ReplyToken, _ctx: &mut Ctx) -> CommandReply {
    match name {
        "now" if args.is_empty() => done(token, Ok(now_reply(a))),
        "now" => done(token, Err(usage("now takes no arguments"))),
        "places" => {
            let query = args.join(" ");
            if query.trim().is_empty() {
                return done(token, Err(usage("places requires a query")));
            }
            let main = a.main as usize;
            std::thread::spawn(move || {
                let result = weather::geocode(&query, 5);
                let boxed = Box::into_raw(Box::new(PlacesResult { query, token, result }));
                unsafe {
                    if PostMessageW(hwnd(main as isize), WM_PLACES, WPARAM(0), LPARAM(boxed as isize)).is_err() {
                        drop(Box::from_raw(boxed));
                    }
                }
            });
            CommandReply::Pending
        }
        "place" => place(a, args, token),
        _ => done(token, Err(usage("unknown command"))),
    }
}

/// Applies a geocoding result on the UI thread and finishes its command token.
pub(crate) fn on_places(a: &mut App, result: Box<PlacesResult>) {
    let PlacesResult { query, token, result } = *result;
    match result {
        Ok(list) if list.is_empty() => token.finish(Err(CommandError {
            usage: false,
            message: format!("no place matches \"{query}\""),
        })),
        Ok(list) => {
            a.places = Some(Places { at: Instant::now(), list: list.clone() });
            let places: Vec<Value> = list.iter().enumerate().map(|(i, place)| json!({
                "n": i + 1,
                "name": place.name,
                "region": place.region,
                "country": place.country,
                "latitude": place.lat,
                "longitude": place.lon,
                "label": place.label(),
            })).collect();
            token.finish(Ok(json!({"ok": true, "query": query, "places": places})));
        }
        Err(e) => token.finish(Err(CommandError { usage: false, message: e })),
    }
}

/// Validates and saves one requested location before starting its forecast fetch.
fn place(a: &mut App, args: &[String], token: ReplyToken) -> CommandReply {
    if args.len() != 1 {
        return done(token, Err(usage("place requires exactly one argument")));
    }
    let parsed = match parse_place_arg(&args[0]) {
        Ok(parsed) => parsed,
        Err(e) => return done(token, Err(usage(e))),
    };
    let (city, latitude, longitude, location_name) = match parsed {
        PlaceArg::Auto => (String::new(), None, None, String::new()),
        PlaceArg::Coordinates { lat, lon } => (String::new(), Some(lat), Some(lon), String::new()),
        PlaceArg::Index(n) => {
            let now = Instant::now();
            let Some(places) = a.places.as_ref().filter(|places| places_recent(places.at, now)) else {
                return done(token, Err(CommandError { usage: false, message: "no recent places list: run places again".into() }));
            };
            if n == 0 || n > places.list.len() {
                return done(token, Err(usage(format!("place number must be in 1..={}", places.list.len()))));
            }
            let selected = &places.list[n - 1];
            (String::new(), Some(selected.lat), Some(selected.lon), selected.label())
        }
    };

    let mut cfg = a.cfg.clone();
    cfg.city = city;
    cfg.latitude = latitude;
    cfg.longitude = longitude;
    cfg.location_name = location_name;
    if let Err(e) = cfg.save_location() {
        return done(token, Err(CommandError { usage: false, message: format!("saving settings failed: {e}") }));
    }

    a.cfg = cfg;
    if let Some(old) = a.place_reply.take() {
        old.token.finish(Err(CommandError { usage: false, message: "superseded by a newer place command".into() }));
    }
    let saved = json!({
        "city": a.cfg.city,
        "latitude": a.cfg.latitude,
        "longitude": a.cfg.longitude,
        "location_name": a.cfg.location_name,
    });
    fetch::restart(a);
    a.place_reply = Some(PlaceReply { generation: a.generation, location: a.cfg.location_key(), token, saved });
    CommandReply::Pending
}

/// Parses `auto`, a numbered result, or latitude and longitude.
fn parse_place_arg(arg: &str) -> Result<PlaceArg, String> {
    if arg == "auto" {
        return Ok(PlaceArg::Auto);
    }
    if let Ok(n) = arg.parse::<usize>() {
        return Ok(PlaceArg::Index(n));
    }
    if arg.contains(',') {
        let mut parts = arg.split(',');
        let lat = parts.next().unwrap_or_default().trim().parse::<f64>().ok();
        let lon = parts.next().unwrap_or_default().trim().parse::<f64>().ok();
        if parts.next().is_none() {
            if let (Some(lat), Some(lon)) = (lat, lon) {
                if lat.is_finite() && (-90.0..=90.0).contains(&lat) && lon.is_finite() && (-180.0..=180.0).contains(&lon) {
                    return Ok(PlaceArg::Coordinates { lat, lon });
                }
            }
        }
        return Err("coordinates must be latitude,longitude within -90..=90 and -180..=180".into());
    }
    Err("expected a positive place number, latitude,longitude, or auto".into())
}

/// Reports whether a search result is no more than ten minutes old.
fn places_recent(at: Instant, now: Instant) -> bool {
    now.checked_duration_since(at).is_some_and(|age| age <= PLACE_LIST_TTL)
}

/// Builds the complete `now` response.
fn now_reply(a: &App) -> Value {
    let mut fields = now_fields(a);
    fields["ok"] = json!(true);
    fields
}

/// Builds the weather fields shared by `now` and a successful `place` response.
pub(super) fn now_fields(a: &App) -> Value {
    let place = a.loc.as_ref().map(|(_, loc)| loc.name.as_str())
        .or_else(|| (!a.cfg.location_name.is_empty()).then_some(a.cfg.location_name.as_str()));
    // weather from the place before a change isn't this place's
    let weather = a.weather.as_ref().filter(|_| a.weather_for == a.cfg.location_key());
    json!({
        "place": place,
        "temperature": temperature_value(weather),
        "unit": if a.cfg.fahrenheit() { "°F" } else { "°C" },
        "conditions": weather.map(|w| describe(w.code, w.is_day).1),
        "updated": weather.map(|w| w.updated.as_str()),
        "age_s": weather.and(a.fetched_at).and_then(|at| SystemTime::now().duration_since(at).ok()).map(|age| age.as_secs()),
        "fetching": a.fetching,
        "error": a.error,
    })
}

/// Converts a finite temperature to JSON, replacing non-finite values with null.
fn finite_value(value: f64) -> Value {
    if value.is_finite() { json!(value) } else { Value::Null }
}

/// Returns the current temperature as JSON, or null without a valid observation.
fn temperature_value(weather: Option<&Weather>) -> Value {
    weather.map(|w| finite_value(w.temp)).unwrap_or(Value::Null)
}

/// Builds an error dockctl should report as a usage mistake.
fn usage(message: impl Into<String>) -> CommandError {
    CommandError { usage: true, message: message.into() }
}

/// Returns an inline command result. The unused token is dropped here: Block ignores the reply
/// a dropped token posts for a command that already answered inline.
fn done(token: ReplyToken, result: Result<Value, CommandError>) -> CommandReply {
    drop(token);
    CommandReply::Done(result)
}

/// A fetch finished: answers the waiting `place` command. A refresh while it waited restarts the
/// fetch for the same place (a later generation still answers it); a change of place fails it.
pub(crate) fn place_fetched(a: &mut App, generation: u64, fetch_error: Option<String>) {
    if !a.place_reply.as_ref().is_some_and(|p| p.generation <= generation) {
        return;
    }
    let Some(pending) = a.place_reply.take() else { return };
    if pending.location != a.cfg.location_key() {
        let message = "the place was saved, but the location changed again before its weather came in";
        pending.token.finish(Err(CommandError { usage: false, message: message.into() }));
        return;
    }
    let reply = match fetch_error {
        Some(e) => json!({ "ok": true, "saved": pending.saved, "fetch_error": e }),
        None => json!({ "ok": true, "saved": pending.saved, "weather": now_fields(a) }),
    };
    pending.token.finish(Ok(reply));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_place_arguments() {
        assert_eq!(parse_place_arg("auto"), Ok(PlaceArg::Auto));
        assert_eq!(parse_place_arg("2"), Ok(PlaceArg::Index(2)));
        assert_eq!(parse_place_arg("-6.25,106.8"), Ok(PlaceArg::Coordinates { lat: -6.25, lon: 106.8 }));
        assert_eq!(parse_place_arg("0"), Ok(PlaceArg::Index(0)));
        for invalid in ["", "Jakarta", "91,0", "0,181", "NaN,0", "1,2,3", "x,2"] {
            assert!(parse_place_arg(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn place_list_expires_after_ten_minutes() {
        let at = Instant::now();
        assert!(places_recent(at, at + PLACE_LIST_TTL));
        assert!(!places_recent(at, at + PLACE_LIST_TTL + Duration::from_nanos(1)));
        assert!(!places_recent(at, at - Duration::from_secs(1)));
    }

    #[test]
    fn now_temperature_turns_nan_into_null() {
        assert_eq!(temperature_value(None), Value::Null);
        // `finite_value` is the exact conversion used by the now-reply temperature field.
        assert_eq!(temperature_value(Some(&weather_with_temp(f64::NAN))), Value::Null);
        assert_eq!(temperature_value(Some(&weather_with_temp(12.5))), json!(12.5));
    }

    fn weather_with_temp(temp: f64) -> Weather {
        Weather {
            location: String::new(), updated: String::new(), temp, feels: 0.0, humidity: 0.0,
            wind: 0.0, code: 0, is_day: true, fahrenheit: false, hourly: Vec::new(), daily: Vec::new(),
            utc_offset: 0, model: String::new(), observed: None, obs_error: None,
        }
    }
}
