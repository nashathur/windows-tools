//! Current observations from KNMI's EDR API (10-minute in-situ station data).
//! Needs a free API key from https://developer.dataplatform.knmi.nl/ (EDR API).

use crate::weather::{get_json_value, Weather};


const BASE: &str = "https://api.dataplatform.knmi.nl/edr/v1/collections/10-minute-in-situ-meteorological-observations";
/// Stations further away than this are not representative of the location.
const MAX_DISTANCE_KM: f64 = 50.0;

#[derive(Clone, Debug)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub km: f64,
}

pub struct Observation {
    pub time: i64, // unix seconds
    pub temp_c: Option<f64>,
    pub rh: Option<f64>,
    pub wind_ms: Option<f64>,
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    6371.0 * 2.0 * a.sqrt().asin()
}

/// The configured station (full WIGOS id or its last part, e.g. "06260"), else the nearest one.
pub fn find_station(agent: &ureq::Agent, key: &str, lat: f64, lon: f64, wanted: &str) -> Result<Station, String> {
    let v = get_json_value(agent, &format!("{BASE}/locations"), Some(key))?;
    let features = v["features"].as_array().ok_or("KNMI: unexpected station list")?;
    let wanted = wanted.trim();
    let mut best: Option<Station> = None;
    for f in features {
        let id = f["id"].as_str().or_else(|| f["properties"]["id"].as_str()).unwrap_or_default();
        let c = &f["geometry"]["coordinates"];
        let (Some(slon), Some(slat)) = (c[0].as_f64(), c[1].as_f64()) else { continue };
        let name = f["properties"]["name"].as_str().unwrap_or(id);
        let km = haversine_km(lat, lon, slat, slon);
        let st = Station { id: id.to_string(), name: title_case(name), km };
        if !wanted.is_empty() {
            if id == wanted || id.ends_with(&format!("-{wanted}")) {
                return Ok(st);
            }
        } else if best.as_ref().map_or(true, |b| km < b.km) {
            best = Some(st);
        }
    }
    if !wanted.is_empty() {
        return Err(format!("KNMI station {wanted} not found"));
    }
    match best {
        Some(s) if s.km <= MAX_DISTANCE_KM => Ok(s),
        Some(s) => Err(format!("nearest KNMI station is {:.0} km away", s.km)),
        None => Err("KNMI: no stations".into()),
    }
}

/// Latest non-missing temperature, humidity and wind of the last hour.
pub fn latest(agent: &ureq::Agent, key: &str, station: &Station, now: i64) -> Result<Observation, String> {
    let url = format!(
        "{BASE}/locations/{}?datetime={}/{}&parameter-name=ta,rh,ff",
        station.id, iso_utc(now - 3600), iso_utc(now)
    );
    let v = get_json_value(agent, &url, Some(key))?;
    // Either a Coverage or a CoverageCollection.
    let cov = if v["coverages"].is_array() { &v["coverages"][0] } else { &v };
    let times: Vec<i64> = cov["domain"]["axes"]["t"]["values"].as_array()
        .map(|a| a.iter().map(|t| t.as_str().and_then(parse_iso_utc).unwrap_or(0)).collect())
        .unwrap_or_default();
    let last = |p: &str| -> Option<(usize, f64)> {
        let vals = cov["ranges"][p]["values"].as_array()?;
        vals.iter().enumerate().rev().find_map(|(i, x)| x.as_f64().map(|f| (i, f)))
    };
    let ta = last("ta");
    let time = ta.and_then(|(i, _)| times.get(i).copied()).filter(|t| *t > 0)
        .ok_or_else(|| format!("KNMI: no recent data from {}", station.name))?;
    Ok(Observation { time, temp_c: ta.map(|x| x.1), rh: last("rh").map(|x| x.1), wind_ms: last("ff").map(|x| x.1) })
}

/// Replaces the model's "current" values with the station measurement.
pub fn apply(w: &mut Weather, obs: &Observation, station: &Station) {
    if let Some(t) = obs.temp_c {
        w.temp = if w.fahrenheit { t * 9.0 / 5.0 + 32.0 } else { t };
    }
    if let Some(rh) = obs.rh {
        w.humidity = rh;
    }
    if let Some(ff) = obs.wind_ms {
        w.wind = ff * if w.fahrenheit { 2.236_94 } else { 3.6 };
    }
    let local = obs.time + w.utc_offset;
    let (h, m) = ((local.rem_euclid(86_400)) / 3600, (local.rem_euclid(3600)) / 60);
    // in the Windows time format (or the shared style's), e.g. "De Bilt 8:20 PM"
    let time = taskbar_dock::style::Look::current().time(h as u32, m as u32);
    w.observed = Some(format!("{} {time}", station.name));
}

fn title_case(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// Civil-date conversions (Howard Hinnant's algorithms), UTC only.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

pub fn iso_utc(secs: i64) -> String {
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s % 3600 / 60, s % 60)
}

/// Parses "YYYY-MM-DDTHH:MM[:SS][Z]" as UTC.
pub fn parse_iso_utc(s: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let secs = if s.len() >= 19 { n(17..19)? } else { 0 };
    Some(days_from_civil(n(0..4)?, n(5..7)?, n(8..10)?) * 86_400 + n(11..13)? * 3600 + n(14..16)? * 60 + secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn iso_roundtrip() {
        let t = parse_iso_utc("2026-10-08T12:10:00Z").unwrap();
        assert_eq!(iso_utc(t), "2026-10-08T12:10:00Z");
        assert_eq!(parse_iso_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
    }
    #[test]
    fn distance() {
        // De Bilt -> Schiphol is about 36 km.
        let d = haversine_km(52.10, 5.18, 52.32, 4.79);
        assert!((d - 36.0).abs() < 3.0, "{d}");
    }
    #[test]
    fn coverage_parsing() {
        let v: Value = serde_json::from_str(r#"{"type":"CoverageCollection","coverages":[{"type":"Coverage",
            "domain":{"axes":{"t":{"values":["2026-10-08T11:50:00Z","2026-10-08T12:00:00Z"]}}},
            "ranges":{"ta":{"values":[15.1,null]},"rh":{"values":[70,71]},"ff":{"values":[4.2,null]}}}]}"#).unwrap();
        let cov = &v["coverages"][0];
        let vals = cov["ranges"]["ta"]["values"].as_array().unwrap();
        let last = vals.iter().enumerate().rev().find_map(|(i, x)| x.as_f64().map(|f| (i, f)));
        assert_eq!(last, Some((0, 15.1)));
    }
}
