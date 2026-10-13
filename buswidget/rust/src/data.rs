//! Live times (OVapi, one small request per stop) and the timetable (national GTFS, reduced to
//! one stop, downloaded weekly) that fills in departures beyond the live window.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Europe::Amsterdam;
use taskbar_dock::settings;
use zip::ZipArchive;
use taskbar_dock::describe::{Kind, Setting};

/// Returns the settings exposed by the bus widget.
pub fn settings() -> Vec<Setting> {
    vec![
        Setting::new(
            "label",
            "Taskbar label",
            Kind::Text {
                placeholder: String::new(),
                empty_ok: false,
            },
        )
        .group("Taskbar"),
        Setting::new(
            "dest",
            "Destination",
            Kind::Text {
                placeholder: String::new(),
                empty_ok: true,
            },
        )
        .help("Empty: the trip's own destination")
        .group("Taskbar"),
        Setting::new(
            "badge_color",
            "Line badge colour",
            Kind::Color {
                hash: false,
                empty_ok: true,
            },
        )
        .group("Line badge"),
        Setting::new(
            "badge_text",
            "Badge text colour",
            Kind::Color {
                hash: false,
                empty_ok: true,
            },
        )
        .group("Line badge"),
        Setting::new(
            "agency",
            "Agency",
            Kind::Text {
                placeholder: String::new(),
                empty_ok: false,
            },
        )
        .help("Set by Change stop… in the menu")
        .advanced(),
        Setting::new(
            "line",
            "Line",
            Kind::Text {
                placeholder: String::new(),
                empty_ok: false,
            },
        )
        .help("Set by Change stop… in the menu")
        .advanced(),
        Setting::new(
            "stop_code",
            "Stop code",
            Kind::Text {
                placeholder: String::new(),
                empty_ok: false,
            },
        )
        .help("Set by Change stop… in the menu")
        .advanced(),
    ]
}

const STATIC_URL: &str = "http://gtfs.ovapi.nl/nl/gtfs-nl.zip";
const TPC_URL: &str = "http://v0.ovapi.nl/tpc/"; // + stop code: live passes at one stop (~6 KB)
const ZIP: &str = "gtfs-nl.zip"; // kept between runs so changing stop needs no new download
pub const TIMETABLE_MAX_AGE_DAYS: i64 = 6; // the cache covers 8 days; refresh weekly

/// Which stop and line to show: the "bus" section of the settings file shared with the other
/// widgets, %APPDATA%\TaskbarDock\settings.json (editable by hand; Change stop… writes it too).
/// The shared look and behaviour (gap, font, timings, …) is the "dock" section, read through
/// taskbar_dock::style.
#[derive(Clone, PartialEq)]
pub struct Config {
    pub agency: String,    // GTFS agency_id, e.g. UOV
    pub line: String,      // route_short_name, e.g. 3
    pub stop_code: String, // stop_code (timing point code) of the platform
    pub label: String,     // small text on the taskbar
    pub dest: String,      // destination shown in the popup; empty = the trip's headsign
    pub badge_color: String, // "RRGGBB" line badge colour; empty = the line colour from the timetable
    pub badge_text: String,  // "RRGGBB" badge number colour; empty = black or white, whichever reads better
    // `compact` and `show_on_taskbar` in the same section are Block's (taskbar_dock::block)
}

const FIELDS: [&str; 7] = ["agency", "line", "stop_code", "label", "dest", "badge_color", "badge_text"];

impl Config {
    fn get(&self, k: &str) -> &str {
        match k {
            "agency" => &self.agency,
            "line" => &self.line,
            "stop_code" => &self.stop_code,
            "label" => &self.label,
            "dest" => &self.dest,
            "badge_color" => &self.badge_color,
            _ => &self.badge_text,
        }
    }

    fn set(&mut self, k: &str, v: String) {
        match k {
            "agency" => self.agency = v,
            "line" => self.line = v,
            "stop_code" => self.stop_code = v,
            "label" => self.label = v,
            "dest" => self.dest = v,
            "badge_color" => self.badge_color = v,
            "badge_text" => self.badge_text = v,
            _ => {}
        }
    }

    /// The stop picker's choice: writes its seven keys in one update, leaving other keys alone.
    pub fn save_stop(&self) -> io::Result<()> {
        let fields = self.to_json();
        settings::update(|root| {
            if !root.is_object() {
                *root = serde_json::json!({});
            }
            let sec = &mut root["bus"];
            if !sec.is_object() {
                *sec = serde_json::json!({});
            }
            for (key, value) in fields.as_object().unwrap() {
                sec[key] = value.clone();
            }
        })
    }

    /// Returns this configuration as the widget's settings defaults.
    pub fn to_json(&self) -> serde_json::Value {
        let mut fields = serde_json::Map::new();
        for key in FIELDS {
            fields.insert(key.to_string(), serde_json::json!(self.get(key)));
        }
        serde_json::Value::Object(fields)
    }

    /// Loads the bus section, filling only missing keys on startup.
    pub fn load() -> Self {
        let defaults = Self::default().to_json();
        let _ = settings::fill_missing("bus", &defaults);
        Self::read()
    }

    /// Reads the bus section without writing it.
    pub fn read() -> Self {
        let Ok(root) = settings::read() else { return Self::default() };
        let mut c = Self::default();
        if let Some(bus) = root.get("bus").and_then(|b| b.as_object()) {
            for k in FIELDS {
                if let Some(v) = bus.get(k).and_then(|v| v.as_str()) {
                    c.set(k, v.to_string());
                }
            }
        }
        c
    }
    /// What the cached schedule depends on.
    pub fn sig(&self) -> String {
        format!("{}/{}/{}", self.agency, self.line, self.stop_code)
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            agency: "UOV".into(),
            line: "3".into(),
            stop_code: "59150850".into(), // Rhônedreef, platform towards Willem Dreeslaan / Utrecht CS
            label: "Rhônedreef → CS".into(),
            dest: "Utrecht CS".into(), // line 3 is a loop: its headsign "Zuilen" would mislead here
            badge_color: String::new(),
            badge_text: String::new(),
        }
    }
}

/// Badge colours per line from data\line_colors.txt: "operator  line  RRGGBB  RRGGBB" per row.
#[derive(Clone)]
pub struct LineColor {
    operator: String,
    line: String,
    pub bg: String,
    pub fg: String,
}

pub fn load_line_colors(dir: &Path) -> Vec<LineColor> {
    let text = fs::read_to_string(dir.join("line_colors.txt")).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let p: Vec<&str> = l.split_whitespace().collect();
            (p.len() == 4).then(|| LineColor {
                operator: p[0].into(),
                line: p[1].into(),
                bg: p[2].into(),
                fg: p[3].into(),
            })
        })
        .collect()
}

/// The colour row for this line; the operator name ("U-OV", "Arriva") is matched loosely
/// against the GTFS agency id ("UOV", "ARR").
pub fn find_line_color<'a>(table: &'a [LineColor], agency: &str, line: &str) -> Option<&'a LineColor> {
    let norm = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
    let a = norm(agency);
    table.iter().find(|c| {
        let o = norm(&c.operator);
        c.line.eq_ignore_ascii_case(line) && !a.is_empty() && !o.is_empty() && (o.starts_with(&a) || a.starts_with(&o))
    })
}

pub struct Dep {
    pub key: String,  // "trip_id|service date"
    pub plan: i64,    // planned departure, unix seconds
    pub seq: u32,     // stop_sequence of our stop in this trip
    pub head: String, // trip headsign
}

pub struct Schedule {
    pub built: String,
    pub sig: String, // Config::sig() it was built for
    pub stops: Vec<String>,
    pub color: String,      // route_color "RRGGBB" from GTFS ("" if the operator gives none)
    pub text_color: String, // route_text_color
    pub deps: Vec<Dep>,
}

/// One departure, from live data or the timetable.
#[derive(Clone)]
pub struct Bus {
    pub t: i64,       // expected departure, unix seconds
    pub plan: i64,    // planned departure
    pub live: bool,   // the bus is on its way and reporting (times are real-time)
    pub cancel: bool, // cancelled
    pub passed: bool, // already left this stop (kept only to know what live data covers)
    pub head: String, // destination
}

fn err<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn today() -> String {
    Utc::now().with_timezone(&Amsterdam).format("%Y-%m-%d").to_string()
}

/// Days since a "YYYY-MM-DD" date (large if it can't be read).
pub fn days_since(date: &str) -> i64 {
    let today = Utc::now().with_timezone(&Amsterdam).date_naive();
    NaiveDate::parse_from_str(date, "%Y-%m-%d").map_or(i64::MAX, |d| (today - d).num_days())
}

pub fn clock(t: i64, fmt: &str) -> String {
    Amsterdam.timestamp_opt(t, 0).single().map(|d| d.format(fmt).to_string()).unwrap_or_default()
}

pub fn log(_dir: &Path, msg: &str) {
    taskbar_dock::log::append(&taskbar_dock::log::path("buswidget"), msg);
}

/// GTFS time: seconds after "noon minus 12h" local time on the service date.
fn plan_epoch(date: &str, sec: i64) -> Option<i64> {
    let d = NaiveDate::parse_from_str(date, "%Y%m%d").ok()?;
    let noon = Amsterdam.from_local_datetime(&d.and_hms_opt(12, 0, 0)?).earliest()?;
    Some(noon.timestamp() - 12 * 3600 + sec)
}

fn num(b: &[u8]) -> Option<i64> {
    std::str::from_utf8(b).ok()?.trim().parse().ok()
}

fn hms(b: &[u8]) -> Option<i64> {
    let mut it = b.split(|&c| c == b':').map(num);
    Some(it.next()?? * 3600 + it.next()?? * 60 + it.next()??)
}

/// Stream one CSV file from the zip, handing the chosen columns of each row to `f`.
fn table<const N: usize>(
    z: &mut ZipArchive<File>,
    name: &str,
    cols: [&str; N],
    mut f: impl FnMut(&[&[u8]; N]),
) -> io::Result<()> {
    let entry = z.by_name(name).map_err(err)?;
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(BufReader::with_capacity(1 << 16, entry));
    let hdr = rdr.byte_headers().map_err(err)?.clone();
    let mut idx = [0usize; N];
    for (k, c) in cols.iter().enumerate() {
        // "?col" = optional column, read as "" when the file doesn't have it
        let (optional, c) = c.strip_prefix('?').map_or((false, *c), |c| (true, c));
        let pos = hdr.iter().position(|h| h.strip_prefix(b"\xef\xbb\xbf").unwrap_or(h) == c.as_bytes());
        idx[k] = match pos {
            Some(i) => i,
            None if optional => usize::MAX,
            None => return Err(err(format!("{name}: no column {c}"))),
        };
    }
    let mut rec = csv::ByteRecord::new();
    while rdr.read_byte_record(&mut rec).map_err(err)? {
        let vals: [&[u8]; N] = std::array::from_fn(|k| rec.get(idx[k]).unwrap_or(b""));
        f(&vals);
    }
    Ok(())
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().to_string()
}

/// Opens the local timetable without attempting a download.
pub fn open_timetable_readonly(dir: &Path) -> io::Result<ZipArchive<File>> {
    let path = dir.join(ZIP);
    if !path.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "timetable not downloaded yet (it downloads once a day; `menu click \"Update timetable\"` starts it)"));
    }
    ZipArchive::new(File::open(path)?).map_err(err)
}

/// Get the national GTFS zip: download when `refresh` and the local copy is older than 20 h
/// (or there is none); otherwise use the local copy.
fn open_timetable(dir: &Path, refresh: bool) -> io::Result<ZipArchive<File>> {
    let zip_path = dir.join(ZIP);
    let age = fs::metadata(&zip_path).and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok());
    if age.is_none() || (refresh && age.unwrap() > Duration::from_secs(20 * 3600)) {
        log(dir, "downloading timetable");
        let part = dir.join(format!("{ZIP}.part"));
        let resp = ureq::get(STATIC_URL).timeout(Duration::from_secs(1800)).call().map_err(err)?;
        let mut f = BufWriter::new(File::create(&part)?);
        io::copy(&mut resp.into_reader(), &mut f)?;
        f.flush()?;
        drop(f);
        fs::rename(&part, &zip_path)?;
    }
    ZipArchive::new(File::open(&zip_path)?).map_err(err)
}

/// Reduce the national GTFS to this stop/line for the next 8 days.
/// `refresh`: daily update (may download); otherwise use the local timetable if there is one.
pub fn build_schedule(dir: &Path, cfg: &Config, refresh: bool) -> io::Result<Schedule> {
    let mut z = open_timetable(dir, refresh)?;

    let mut stops: HashSet<Vec<u8>> = HashSet::new();
    table(&mut z, "stops.txt", ["stop_id", "stop_code"], |r| {
        if r[1] == cfg.stop_code.as_bytes() {
            stops.insert(r[0].to_vec());
        }
    })?;
    if stops.is_empty() {
        return Err(err(format!("stop code {} not in timetable", cfg.stop_code)));
    }
    let mut routes: HashSet<Vec<u8>> = HashSet::new();
    let (mut color, mut text_color) = (String::new(), String::new());
    let cols = ["route_id", "agency_id", "route_short_name", "?route_color", "?route_text_color"];
    table(&mut z, "routes.txt", cols, |r| {
        if r[1] == cfg.agency.as_bytes() && r[2] == cfg.line.as_bytes() {
            routes.insert(r[0].to_vec());
            if color.is_empty() && !r[3].is_empty() {
                (color, text_color) = (s(r[3]), s(r[4]));
            }
        }
    })?;
    let mut trips: HashMap<Vec<u8>, (Vec<u8>, String)> = HashMap::new(); // trip -> service, headsign
    table(&mut z, "trips.txt", ["trip_id", "route_id", "service_id", "?trip_headsign"], |r| {
        if routes.contains(r[1]) {
            trips.insert(r[0].to_vec(), (r[2].to_vec(), s(r[3])));
        }
    })?;
    let mut hits: Vec<(Vec<u8>, i64, u32)> = Vec::new(); // trip, seconds, stop_sequence
    let cols = ["trip_id", "stop_id", "stop_sequence", "arrival_time", "departure_time"];
    table(&mut z, "stop_times.txt", cols, |r| {
        if stops.contains(r[1]) && trips.contains_key(r[0]) {
            let t = if r[4].is_empty() { r[3] } else { r[4] };
            if let (Some(sec), Some(seq)) = (hms(t), num(r[2])) {
                hits.push((r[0].to_vec(), sec, seq as u32));
            }
        }
    })?;
    let services: HashSet<Vec<u8>> = hits.iter().filter_map(|h| trips.get(&h.0).map(|t| t.0.clone())).collect();
    let d0 = Utc::now().with_timezone(&Amsterdam).date_naive();
    let keep: HashSet<String> = (-1..8)
        .map(|d| (d0 + chrono::Duration::days(d)).format("%Y%m%d").to_string())
        .collect();
    let mut dates: HashMap<Vec<u8>, Vec<String>> = HashMap::new();
    table(&mut z, "calendar_dates.txt", ["service_id", "date", "exception_type"], |r| {
        if r[2] == b"1" && services.contains(r[0]) {
            if let Ok(d) = std::str::from_utf8(r[1]) {
                if keep.contains(d) {
                    dates.entry(r[0].to_vec()).or_default().push(d.to_string());
                }
            }
        }
    })?;

    let mut deps = Vec::new();
    for (trip, sec, seq) in &hits {
        let (service, head) = &trips[trip];
        for d in dates.get(service).map(|v| v.as_slice()).unwrap_or(&[]) {
            if let Some(plan) = plan_epoch(d, *sec) {
                let key = format!("{}|{}", String::from_utf8_lossy(trip), d);
                deps.push(Dep { key, plan, seq: *seq, head: head.clone() });
            }
        }
    }
    let s = Schedule {
        built: today(),
        sig: cfg.sig(),
        stops: stops.iter().map(|s| String::from_utf8_lossy(s).into_owned()).collect(),
        color,
        text_color,
        deps,
    };
    save_schedule(dir, &s)?;
    log(dir, &format!(
        "timetable built for {}: {} departures, stops {:?}, line colour {:?} text {:?}",
        s.sig, s.deps.len(), s.stops, s.color, s.text_color
    ));
    Ok(s)
}

fn save_schedule(dir: &Path, s: &Schedule) -> io::Result<()> {
    let part = dir.join("schedule.tsv.part");
    let mut f = BufWriter::new(File::create(&part)?);
    writeln!(f, "v2\t{}\t{}\t{}\t{}\t{}", s.built, s.sig, s.stops.join(","), s.color, s.text_color)?;
    for d in &s.deps {
        writeln!(f, "{}\t{}\t{}\t{}", d.key, d.plan, d.seq, d.head)?;
    }
    f.flush()?;
    drop(f);
    fs::rename(part, dir.join("schedule.tsv"))
}

pub fn load_schedule(dir: &Path) -> Option<Schedule> {
    let mut lines = BufReader::new(File::open(dir.join("schedule.tsv")).ok()?).lines();
    let head = lines.next()?.ok()?;
    let h: Vec<&str> = head.split('\t').collect();
    let s = if h[0] == "v2" && h.len() == 6 {
        Schedule {
            built: h[1].into(),
            sig: h[2].into(),
            stops: h[3].split(',').map(String::from).collect(),
            color: h[4].into(),
            text_color: h[5].into(),
            deps: Vec::new(),
        }
    } else {
        // older cache (always the default stop): still use it, but mark it stale so it gets rebuilt
        Schedule {
            built: String::new(),
            sig: Config::default().sig(),
            stops: h.get(1)?.split(',').map(String::from).collect(),
            color: h.get(2).unwrap_or(&"").to_string(),
            text_color: h.get(3).unwrap_or(&"").to_string(),
            deps: Vec::new(),
        }
    };
    let mut deps = Vec::new();
    for l in lines {
        let l = l.ok()?;
        let p: Vec<&str> = l.split('\t').collect();
        if p.len() >= 3 {
            let head = p.get(3).unwrap_or(&"").to_string();
            deps.push(Dep { key: p[0].into(), plan: p[1].parse().ok()?, seq: p[2].parse().ok()?, head });
        }
    }
    Some(Schedule { deps, ..s })
}

// ---- stop picker ------------------------------------------------------------

pub struct StopHit {
    pub code: String,
    pub name: String,
    pub folded: String, // lower case, no accents: for searching
}

/// Lower case and strip common accents, so "rhone" finds "Rhônedreef".
pub fn fold(t: &str) -> String {
    t.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            'ÿ' | 'ý' => 'y',
            _ => c,
        })
        .collect()
}

/// All boarding platforms with a stop code, from the local timetable.
pub fn all_stops(dir: &Path) -> io::Result<Vec<StopHit>> {
    let mut z = open_timetable_readonly(dir)?;
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    table(&mut z, "stops.txt", ["stop_code", "stop_name", "?location_type"], |r| {
        if !r[0].is_empty() && (r[2].is_empty() || r[2] == b"0") && seen.insert(r[0].to_vec()) {
            let name = s(r[1]);
            v.push(StopHit { code: s(r[0]), folded: fold(&name), name });
        }
    })?;
    v.sort_by(|a, b| a.name.cmp(&b.name).then(a.code.cmp(&b.code)));
    Ok(v)
}

#[derive(Clone)]
pub struct LineOpt {
    pub agency: String,
    pub line: String,
    pub headsign: String,
    pub next: String, // next stop after this platform ("" = trip ends here)
    pub trips: usize,
    pub color: String,
    pub text_color: String,
}

/// Lines and directions serving one platform, with the stop each goes to next.
pub fn lines_at_stop(dir: &Path, stop_code: &str) -> io::Result<Vec<LineOpt>> {
    let mut z = open_timetable_readonly(dir)?;
    let mut ids: HashSet<Vec<u8>> = HashSet::new();
    table(&mut z, "stops.txt", ["stop_id", "stop_code"], |r| {
        if r[1] == stop_code.as_bytes() {
            ids.insert(r[0].to_vec());
        }
    })?;
    // stop_times is ordered by trip, then sequence: the row after ours is the next stop
    let mut next_of: HashMap<Vec<u8>, Vec<u8>> = HashMap::new(); // trip -> next stop_id ("" = none)
    let mut prev_trip: Vec<u8> = Vec::new();
    let mut prev_ours = false;
    table(&mut z, "stop_times.txt", ["trip_id", "stop_id"], |r| {
        let same = r[0] == prev_trip.as_slice();
        if prev_ours && same {
            next_of.insert(r[0].to_vec(), r[1].to_vec());
        }
        if !same {
            prev_trip.clear();
            prev_trip.extend_from_slice(r[0]);
        }
        prev_ours = ids.contains(r[1]);
        if prev_ours {
            next_of.insert(r[0].to_vec(), Vec::new());
        }
    })?;
    let mut trip_info: HashMap<Vec<u8>, (Vec<u8>, String)> = HashMap::new(); // trip -> route, headsign
    table(&mut z, "trips.txt", ["trip_id", "route_id", "?trip_headsign"], |r| {
        if next_of.contains_key(r[0]) {
            trip_info.insert(r[0].to_vec(), (r[1].to_vec(), s(r[2])));
        }
    })?;
    let wanted: HashSet<&Vec<u8>> = trip_info.values().map(|t| &t.0).collect();
    let mut routes: HashMap<Vec<u8>, (String, String, String, String)> = HashMap::new(); // route -> agency, line, badge colours
    table(&mut z, "routes.txt", ["route_id", "agency_id", "route_short_name", "?route_color", "?route_text_color"], |r| {
        if wanted.contains(&r[0].to_vec()) {
            routes.insert(r[0].to_vec(), (s(r[1]), s(r[2]), s(r[3]), s(r[4])));
        }
    })?;
    let next_ids: HashSet<&Vec<u8>> = next_of.values().filter(|v| !v.is_empty()).collect();
    let mut names: HashMap<Vec<u8>, String> = HashMap::new();
    table(&mut z, "stops.txt", ["stop_id", "stop_name"], |r| {
        if next_ids.contains(&r[0].to_vec()) {
            names.insert(r[0].to_vec(), s(r[1]));
        }
    })?;

    let mut count: HashMap<(String, String, String, String, String, String), usize> = HashMap::new();
    for (trip, next) in &next_of {
        let Some((route, head)) = trip_info.get(trip) else { continue };
        let Some((agency, line, color, text_color)) = routes.get(route) else { continue };
        let next = names.get(next).cloned().unwrap_or_default();
        *count.entry((agency.clone(), line.clone(), head.clone(), next, color.clone(), text_color.clone())).or_default() += 1;
    }
    let mut v: Vec<LineOpt> = count
        .into_iter()
        .map(|((agency, line, headsign, next, color, text_color), trips)| LineOpt { agency, line, headsign, next, trips, color, text_color })
        .collect();
    // numeric line order (3 before 28), busiest direction first; the rest only makes the order
    // the same on every run (the counts come out of a HashMap), so `choose --to <n>` is stable
    let key = |l: &LineOpt| (l.line.parse::<u32>().unwrap_or(u32::MAX), l.line.clone(), usize::MAX - l.trips,
        l.agency.clone(), l.headsign.clone(), l.next.clone());
    v.sort_by_key(key);
    Ok(v)
}

// ---- live times: OVapi per-stop passes ----------------------------------------

/// OVapi local time "2026-10-08T13:19:00" (Dutch time, no offset) to unix seconds.
fn local_ts(s: &str) -> Option<i64> {
    let n = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()?;
    Amsterdam.from_local_datetime(&n).earliest().map(|d| d.timestamp())
}

/// One request (~6 KB) for the configured stop; returns this line's passes, sorted.
pub fn fetch_live(cfg: &Config) -> io::Result<Vec<Bus>> {
    let url = format!("{TPC_URL}{}", cfg.stop_code);
    let resp = ureq::get(&url).timeout(Duration::from_secs(20)).call().map_err(err)?;
    let mut body = String::new();
    resp.into_reader().take(4 << 20).read_to_string(&mut body)?;
    parse_live(&body, cfg)
}

fn parse_live(body: &str, cfg: &Config) -> io::Result<Vec<Bus>> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(err)?;
    let mut out = Vec::new();
    let passes = v.get(&cfg.stop_code).and_then(|s| s.get("Passes")).and_then(|p| p.as_object());
    for p in passes.into_iter().flat_map(|m| m.values()) {
        let g = |k: &str| p.get(k).and_then(|x| x.as_str()).unwrap_or("");
        if g("LinePublicNumber") != cfg.line {
            continue;
        }
        // KV78 status: PLANNED (no live data yet), DRIVING, OFFROUTE, ARRIVED, PASSED, CANCEL, UNKNOWN
        let status = g("TripStopStatus");
        let (Some(plan), Some(t)) = (local_ts(g("TargetDepartureTime")), local_ts(g("ExpectedDepartureTime")))
        else {
            continue;
        };
        out.push(Bus {
            t,
            plan,
            live: matches!(status, "DRIVING" | "OFFROUTE" | "ARRIVED"),
            cancel: status == "CANCEL",
            passed: status == "PASSED",
            head: g("DestinationName50").to_string(),
        });
    }
    out.sort_by_key(|b| b.t);
    Ok(out)
}

/// Departures still to come: live data where it reaches, the timetable after that.
pub fn upcoming(sched: Option<&Schedule>, live: &[Bus], now: i64) -> Vec<Bus> {
    let mut v: Vec<Bus> = live.iter().filter(|b| !b.passed && b.t - now >= -30).cloned().collect();
    // live data covers buses up to its last planned time; trust it there (a missing bus has passed)
    let horizon = live.iter().map(|b| b.plan).max();
    if let Some(s) = sched {
        for d in &s.deps {
            let ahead = d.plan - now;
            if ahead < -30 || ahead > 6 * 3600 || horizon.map_or(false, |h| d.plan <= h + 30) {
                continue;
            }
            v.push(Bus { t: d.plan, plan: d.plan, live: false, cancel: false, passed: false, head: d.head.clone() });
        }
    }
    v.sort_by_key(|b| b.t);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_json_has_all_fields_and_default_stop() {
        let defaults = Config::default().to_json();
        let fields = defaults.as_object().unwrap();
        assert_eq!(fields.len(), FIELDS.len());
        assert!(FIELDS.iter().all(|key| fields.contains_key(*key)));
        assert_eq!(defaults["agency"], "UOV");
        assert_eq!(defaults["line"], "3");
        assert_eq!(defaults["stop_code"], "59150850");
    }

    #[test]
    fn readonly_timetable_reports_missing_file_without_creating_one() {
        let dir = std::env::temp_dir().join(format!("bus-no-timetable-{}-{}", std::process::id(), now()));
        let e = match open_timetable_readonly(&dir) { Ok(_) => panic!("missing timetable opened"), Err(e) => e };
        assert_eq!(e.to_string(), "timetable not downloaded yet (it downloads once a day; `menu click \"Update timetable\"` starts it)");
        assert!(!dir.join(ZIP).exists());
    }

    // a saved OVapi response for Rhônedreef (59150850): tests never call the API
    #[test]
    fn parses_saved_response() {
        let body = fs::read_to_string("tests/tpc_59150850.json").expect("sample response");
        let cfg = Config::default();
        let buses = parse_live(&body, &cfg).unwrap();
        assert!(!buses.is_empty());
        for b in &buses {
            assert_eq!(b.head, "Utrecht CS");
            assert!(b.t >= b.plan - 600, "expected time far before planned");
        }
        let live = buses.iter().filter(|b| b.live).count();
        eprintln!("{} passes, {} live", buses.len(), live);
        for b in &buses {
            eprintln!(
                "plan {} expected {} live={} passed={} cancel={}",
                clock(b.plan, "%H:%M:%S"), clock(b.t, "%H:%M:%S"), b.live, b.passed, b.cancel
            );
        }
    }

    #[test]
    fn timetable_fills_in_after_live_horizon() {
        let live = vec![
            Bus { t: 1000, plan: 1000, live: true, cancel: false, passed: false, head: "A".into() },
            Bus { t: 1900, plan: 1900, live: false, cancel: false, passed: false, head: "A".into() },
        ];
        let dep = |plan| Dep { key: String::new(), plan, seq: 1, head: "B".into() };
        let s = Schedule {
            built: String::new(),
            sig: String::new(),
            stops: vec![],
            color: String::new(),
            text_color: String::new(),
            deps: vec![dep(1000), dep(1450), dep(1900), dep(2800)],
        };
        let v = upcoming(Some(&s), &live, 900);
        let plans: Vec<i64> = v.iter().map(|b| b.plan).collect();
        // 1450 is inside the live window but missing there (already passed): not re-added
        assert_eq!(plans, vec![1000, 1900, 2800]);
        assert_eq!(v[2].head, "B");
    }
}
