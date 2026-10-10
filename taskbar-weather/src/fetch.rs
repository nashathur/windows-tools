//! Fetching the forecast and KNMI observations on a worker thread, and applying the result.

use super::*;
use std::time::SystemTime;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub(crate) struct Fetched {
    key: String,
    loc: Option<Location>,
    station: Option<(String, knmi::Station)>,
    result: Result<Weather, String>,
}

pub(crate) fn start_fetch(a: &mut App) {
    if a.fetching {
        return;
    }
    a.fetching = true;
    let (main, cfg, cached, cached_station) = (a.main as usize, a.cfg.clone(), a.loc.clone(), a.station.clone());
    std::thread::spawn(move || {
        let key = cfg.location_key();
        let loc = match cached {
            Some((k, l)) if k == key => Ok(l),
            _ => weather::resolve_location(&cfg),
        };
        let mut result = loc.clone().and_then(|l| weather::fetch(&l, cfg.fahrenheit(), &cfg.model));
        let mut station = None;
        if let (Ok(w), Ok(l)) = (&mut result, &loc) {
            station = add_observation(w, l, &cfg, cached_station);
        }
        let msg = Box::into_raw(Box::new(Fetched { key, loc: loc.ok(), station, result }));
        unsafe {
            if PostMessageW(hwnd(main as isize), WM_DATA, WPARAM(0), LPARAM(msg as isize)).is_err() {
                drop(Box::from_raw(msg));
            }
        }
    });
}

/// With a KNMI key, swaps the model's current values for the latest station measurement.
/// Returns the station used, so the (rarely changing) station lookup can be cached.
pub(crate) fn add_observation(w: &mut Weather, loc: &Location, cfg: &Config, cached: Option<(String, knmi::Station)>)
    -> Option<(String, knmi::Station)> {
    let key = cfg.knmi_api_key.trim();
    if key.is_empty() {
        return None;
    }
    let station_key = format!("{:.3},{:.3}|{}", loc.lat, loc.lon, cfg.knmi_station.trim());
    let fail = |w: &mut Weather, e: String| {
        w.obs_error = Some(if e.starts_with("KNMI") { e } else { format!("KNMI: {e}") });
    };
    let agent = match weather::agent() {
        Ok(a) => a,
        Err(e) => {
            fail(w, e);
            return None;
        }
    };
    let station = match cached {
        Some((k, s)) if k == station_key => s,
        _ => match knmi::find_station(&agent, key, loc.lat, loc.lon, &cfg.knmi_station) {
            Ok(s) => s,
            Err(e) => {
                fail(w, e);
                return None;
            }
        },
    };
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    match knmi::latest(&agent, key, &station, now) {
        Ok(obs) => knmi::apply(w, &obs, &station),
        Err(e) => fail(w, e),
    }
    Some((station_key, station))
}

/// A fetch finished: new data (or the error) for the strip, the tray icon and an open flyout.
pub(crate) fn on_data(a: &mut App, f: Box<Fetched>, ctx: &mut Ctx) {
    let Fetched { key, loc, station, result } = *f;
    a.fetching = false;
    if let Some(l) = loc {
        a.loc = Some((key, l));
    }
    if station.is_some() {
        a.station = station;
    }
    match result {
        Ok(w) => {
            a.weather = Some(w);
            a.error = None;
        }
        Err(e) => a.error = Some(e),
    }
    // retry sooner after an error
    let ms = if a.error.is_some() { 2 * 60_000 } else { a.cfg.refresh_minutes.max(5) * 60_000 };
    ctx.every(TIMER_REFRESH, ms);
    ctx.redraw_panel();
    ctx.redraw();
}
