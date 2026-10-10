//! Fetching the forecast and KNMI observations on a worker thread, and applying the result.

use super::*;

pub(crate) fn start_fetch() {
    let job = with(|a| {
        if a.fetching {
            return None;
        }
        a.fetching = true;
        Some((a.main.0 as usize, a.cfg.clone(), a.loc.clone(), a.station.clone()))
    })
    .flatten();
    let Some((main, cfg, cached, cached_station)) = job else { return };
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
            if PostMessageW(HWND(main as *mut c_void), WM_DATA, WPARAM(0), LPARAM(msg as isize)).is_err() {
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

pub(crate) fn on_data(f: Box<Fetched>) {
    let Fetched { key, loc, station, result } = *f;
    let state = with(|a| {
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
        (a.main, a.popup, a.error.is_some(), a.cfg.refresh_minutes.max(5))
    });
    let Some((main, popup, failed, minutes)) = state else { return };
    unsafe {
        // Retry sooner after an error.
        let ms = if failed { 2 * 60_000 } else { minutes * 60_000 };
        SetTimer(main, TIMER_REFRESH, ms, None);
        if let Some(p) = popup {
            let _ = InvalidateRect(p, None, false);
        }
    }
    update_tray(false);
    redraw_label();
}
