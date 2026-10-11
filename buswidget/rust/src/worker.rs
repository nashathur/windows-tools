//! State shared with the worker thread, and the worker: timetable builds and live polling.

use super::*;

pub(crate) struct Shared {
    pub(crate) cfg: data::Config,
    pub(crate) sched: Option<Arc<data::Schedule>>,
    pub(crate) live: Arc<Vec<data::Bus>>, // latest live passes for cfg's stop
    pub(crate) ok_at: Option<i64>,        // when they were fetched
    pub(crate) err: Option<String>,
    pub(crate) no_live: bool, // OVapi has no live data for this stop (404): timetable only
}

pub(crate) fn shared() -> &'static Mutex<Shared> {
    static S: OnceLock<Mutex<Shared>> = OnceLock::new();
    S.get_or_init(|| {
        let cfg = data::Config::load();
        Mutex::new(Shared { cfg, sched: None, live: Arc::default(), ok_at: None, err: None, no_live: false })
    })
}

/// What to show for the configured stop: timetable (None while it is being built) and live
/// passes (empty when older than LIVE_MAX_AGE_S, e.g. offline).
pub(crate) fn current() -> (data::Config, Option<Arc<data::Schedule>>, Arc<Vec<data::Bus>>, Option<i64>, Option<String>) {
    let g = shared().lock().unwrap();
    let sched = g.sched.clone().filter(|s| s.sig == g.cfg.sig());
    let fresh = g.ok_at.map_or(false, |t| data::now() - t < LIVE_MAX_AGE_S);
    let live = if fresh { g.live.clone() } else { Arc::default() };
    (g.cfg.clone(), sched, live, g.ok_at, g.err.clone())
}

/// Switch to another stop/line (from the picker): save it and rebuild from the local timetable.
pub(crate) fn set_config(cfg: data::Config) -> std::io::Result<()> {
    if let Err(e) = cfg.save() {
        data::log(data_dir(), &format!("saving config failed: {e}"));
        return Err(e);
    }
    apply_config(cfg);
    Ok(())
}

/// Use new settings: a different stop or line means a new timetable and fresh live data.
pub(crate) fn apply_config(cfg: data::Config) {
    let mut g = shared().lock().unwrap();
    if g.cfg == cfg {
        return;
    }
    if g.cfg.sig() != cfg.sig() {
        g.live = Arc::default();
        g.ok_at = None;
        g.no_live = false;
        CONFIG_CHANGED.store(true, Relaxed);
        FORCE_RT.store(true, Relaxed);
    }
    g.cfg = cfg;
}

/// Block's main window (set in `Content::start`): the worker posts WM_DATA there.
pub(crate) static MAIN: AtomicIsize = AtomicIsize::new(0);

/// New data for the UI: the strip, popup and tooltip are checked at once (else on the next tick).
fn notify() {
    let main = MAIN.load(Relaxed);
    if main != 0 {
        unsafe { PostMessageW(main, WM_DATA, 0, 0) };
    }
}

pub(crate) static FORCE_RT: AtomicBool = AtomicBool::new(false);
pub(crate) static FORCE_TT: AtomicBool = AtomicBool::new(false);
pub(crate) static CONFIG_CHANGED: AtomicBool = AtomicBool::new(false);

pub(crate) fn data_dir() -> &'static PathBuf {
    static D: OnceLock<PathBuf> = OnceLock::new();
    D.get_or_init(|| {
        let exe = std::env::current_exe().unwrap_or_default();
        let d = exe.parent().map(|p| p.join("data")).unwrap_or_else(|| PathBuf::from("data"));
        let _ = fs::create_dir_all(&d);
        d
    })
}

pub(crate) fn worker() {
    let dir = data_dir();
    if let Some(s) = data::load_schedule(dir) {
        shared().lock().unwrap().sched = Some(Arc::new(s));
        notify();
    }
    // back-off after a failed timetable download survives restarts
    let backoff_file = dir.join("next_build.txt");
    let mut next_build: i64 = fs::read_to_string(&backoff_file).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let (mut last_poll, mut live_pause_until) = (0i64, 0i64);
    let mut last_sig = String::new();
    loop {
        let (cfg, sched) = {
            let g = shared().lock().unwrap();
            (g.cfg.clone(), g.sched.clone())
        };
        if cfg.sig() != last_sig {
            live_pause_until = 0; // a pause for the previous stop doesn't apply to the new one
            last_sig = cfg.sig();
        }

        // timetable: weekly, or when the stop changes
        let switched = sched.as_ref().map_or(true, |s| s.sig != cfg.sig());
        let stale = switched || sched.as_ref().map_or(true, |s| data::days_since(&s.built) >= data::TIMETABLE_MAX_AGE_DAYS);
        let (changed, force) = (CONFIG_CHANGED.swap(false, Relaxed), FORCE_TT.swap(false, Relaxed));
        if changed || force || (stale && data::now() >= next_build) {
            // the weekly update may download; a stop change uses the timetable already on disk
            let refresh = force || !switched;
            match data::build_schedule(dir, &cfg, refresh) {
                Ok(s) => {
                    let mut g = shared().lock().unwrap();
                    if s.sig == g.cfg.sig() {
                        g.sched = Some(Arc::new(s));
                    }
                }
                Err(e) => {
                    // keep the old cache and keep polling live times; retry later
                    // (much later if the server says we're downloading too often)
                    let wait = if e.to_string().contains("429") { 3 * 3600 } else { 600 };
                    next_build = data::now() + wait;
                    let _ = fs::write(&backoff_file, next_build.to_string());
                    data::log(dir, &format!("timetable update failed: {e}; next try in {} min", wait / 60));
                    shared().lock().unwrap().err = Some(format!("timetable: {e}"));
                }
            }
            notify();
            continue;
        }

        // live times: one small request per POLL_S, only while a bus may be due soon
        let now = data::now();
        let soon = match sched.as_ref().filter(|s| s.sig == cfg.sig()) {
            Some(s) => s.deps.iter().any(|d| d.plan > now - 120 && d.plan - now < LIVE_WINDOW_MIN * 60),
            None => true, // no timetable yet: live data is all we have
        };
        let forced = FORCE_RT.swap(false, Relaxed);
        if now >= live_pause_until && (forced || (soon && now - last_poll >= POLL_S)) {
            last_poll = now;
            match data::fetch_live(&cfg) {
                Ok(live) => {
                    let mut g = shared().lock().unwrap();
                    if g.cfg.sig() == cfg.sig() {
                        g.live = Arc::new(live);
                        g.ok_at = Some(data::now());
                        g.err = None;
                        g.no_live = false;
                    }
                }
                Err(e) if e.to_string().contains("404") => {
                    // OVapi has no live data for this platform: timetable only; look again hourly
                    if !shared().lock().unwrap().no_live {
                        data::log(dir, &format!("no live data for stop {} (404); timetable times only", cfg.stop_code));
                    }
                    let mut g = shared().lock().unwrap();
                    if g.cfg.sig() == cfg.sig() {
                        g.no_live = true;
                    }
                    live_pause_until = data::now() + 3600;
                }
                Err(e) => {
                    // no quick retries: the next regular poll is soon enough
                    data::log(dir, &format!("live update failed: {e}"));
                    shared().lock().unwrap().err = Some(e.to_string());
                    if e.to_string().contains("429") {
                        live_pause_until = data::now() + 900; // server says too many requests
                    }
                }
            }
            notify();
        }
        thread::sleep(Duration::from_millis(500));
    }
}
