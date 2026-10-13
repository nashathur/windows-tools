use serde_json::{json, Value};
use taskbar_dock::{
    describe::{Kind, Setting},
    settings,
};
#[derive(Clone, Debug)]
pub struct Config {
    pub speed: String,
    pub range: String,
    pub warn_pct: f64,
    pub crit_pct: f64,
    pub gpu_temp_warn: f64,
    pub cpu_temp_warn: f64,
    pub cpu_temp_crit: f64,
    pub include_virtual: bool,
    /// The widest the strip gets, px at 100% (default: media's widest strip).
    pub max_width: f64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            speed: "normal".into(),
            range: "1m".into(),
            warn_pct: 80.,
            crit_pct: 95.,
            gpu_temp_warn: 80.,
            cpu_temp_warn: 80.,
            cpu_temp_crit: 90.,
            include_virtual: false,
            max_width: 254.,
        }
    }
}
impl Config {
    pub fn load() -> Self {
        let mut c = Self::default();
        if let Some(v) = settings::section("sysmon") {
            c.speed = v["speed"].as_str().unwrap_or("normal").to_owned();
            if !["high", "normal", "low"].contains(&c.speed.as_str()) {
                c.speed = "normal".into()
            }
            c.range = v["range"].as_str().unwrap_or("1m").into();
            c.warn_pct = num(&v, "warn_pct", 80.);
            c.crit_pct = num(&v, "crit_pct", 95.);
            c.gpu_temp_warn = num(&v, "gpu_temp_warn", 80.);
            c.cpu_temp_warn = num(&v, "cpu_temp_warn", 80.);
            c.cpu_temp_crit = num(&v, "cpu_temp_crit", 90.);
            c.include_virtual = v["include_virtual"].as_bool().unwrap_or(false);
            c.max_width = num(&v, "max_width", 254.);
        }
        c
    }
    pub fn save(&self, k: &str, v: Value) {
        let _ = settings::set_key("sysmon", k, v);
    }
    pub fn defaults() -> Value {
        json!({"speed":"normal","range":"1m","warn_pct":80,"crit_pct":95,"gpu_temp_warn":80,"cpu_temp_warn":80,"cpu_temp_crit":90,"include_virtual":false,"max_width":254})
    }
}
fn num(v: &Value, k: &str, d: f64) -> f64 {
    v[k].as_f64().unwrap_or(d)
}
pub fn descriptions() -> Vec<Setting> {
    vec![
        Setting::new(
            "speed",
            "Update speed",
            Kind::Choice {
                options: vec![
                    (json!("high"), "High".into()),
                    (json!("normal"), "Normal".into()),
                    (json!("low"), "Low".into()),
                ],
                other: false,
            },
        )
        .group("Monitoring"),
        Setting::new(
            "range",
            "Graph range",
            Kind::Choice {
                options: vec![
                    (json!("1m"), "1 minute".into()),
                    (json!("5m"), "5 minutes".into()),
                    (json!("1h"), "1 hour".into()),
                ],
                other: false,
            },
        )
        .group("Monitoring"),
        Setting::number("warn_pct", "Usage warning", 0., 100., 1.).group("Thresholds"),
        Setting::number("crit_pct", "Usage critical", 0., 100., 1.).group("Thresholds"),
        Setting::number("gpu_temp_warn", "GPU temperature warning", 0., 120., 1.)
            .group("Thresholds"),
        Setting::number("cpu_temp_warn", "CPU temperature warning", 0., 120., 1.)
            .group("Thresholds"),
        Setting::number("cpu_temp_crit", "CPU temperature critical", 0., 120., 1.)
            .group("Thresholds"),
        Setting::new("include_virtual", "Include virtual adapters", Kind::Switch).group("Network"),
        Setting::number("max_width", "Maximum strip width", 120., 400., 1.).group("Display"),
    ]
}
