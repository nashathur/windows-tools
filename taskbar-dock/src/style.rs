//! The shared look and behaviour: the `dock` section of settings.json.
//!
//! ```json
//! "dock": {
//!   "gap": 3,                          px between docked widgets and to the tray (at 100%)
//!   "font": "Segoe UI Variable Text",  same family as the taskbar clock
//!   "font_size": 12,                   px at 100%, same as the clock
//!   "text_size": 1.0,                  1.0 = match the clock; "system" = Windows text size
//!   "control_radius": 4,               hover highlight, badges (Windows 11 controls: 4 px)
//!   "time_format": "system",           "system" (Windows region format), "12h" or "24h"
//!   "animations": "system",            "system" (Windows animation effects), true or false
//!   "hover_open_ms": "system",         rest this long to open a panel; "system" = Windows
//!   "hover_close_ms": 300,             a hover-opened panel closes this long after leaving
//!   "fade_ms": 150, "zoom_ms": 120,
//!   "glide_ms": 300,                   slides and the compact switch (as the taskbar's buttons)
//!   "drag_zoom": 1.15,                 content size while dragged
//!   "panel_margin": 12,                px a panel keeps from the taskbar and the screen edges
//!   "tint_dark":  { "quiet": 0.18, "active": 0.333, "hover": 0.49 },   brand colour behind a
//!   "tint_light": { "quiet": 0.149, "active": 0.275, "hover": 0.412 }, strip (discord, media):
//!                                      paused / muted, playing / unmuted, under the mouse
//!   "tint_fixed": "auto"               "auto": the levels above by state; a number 0..1: that
//!                                      strength always (0 = no tint, 1 = solid colour)
//! }
//! ```
//! Missing keys are filled in with these defaults, so the file always shows every option.
//! Panels get Windows' own 8 px rounding from the system (DWM), so that isn't a setting.

use crate::settings;
use crate::system::{self, System};
use crate::theme::{tint_levels, TintLevels};
use serde_json::{json, Map, Value};

/// A setting that can follow Windows ("system") or be fixed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Follow<T> {
    System,
    Fixed(T),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TimeFormat {
    System,
    H12,
    H24,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Style {
    pub gap: f32,
    pub font: String,
    pub font_size: f32,
    pub text_size: Follow<f32>,
    pub control_radius: f32,
    pub time_format: TimeFormat,
    pub animations: Follow<bool>,
    pub hover_open_ms: Follow<u32>,
    pub hover_close_ms: u32,
    pub fade_ms: f32,
    pub glide_ms: f32,
    pub zoom_ms: f32,
    pub drag_zoom: f32,
    pub panel_margin: f32,
    /// Brand tint strengths on a dark / light taskbar, and one strength for every state instead.
    pub tint_dark: TintLevels,
    pub tint_light: TintLevels,
    pub tint_fixed: Option<f32>,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            gap: 3.0,
            font: "Segoe UI Variable Text".into(),
            font_size: 12.0,
            // The taskbar clock doesn't follow Windows' text size, so matching it means 1.0.
            text_size: Follow::Fixed(1.0),
            control_radius: 4.0,
            time_format: TimeFormat::System,
            animations: Follow::System,
            hover_open_ms: Follow::System,
            hover_close_ms: 300,
            fade_ms: 150.0,
            glide_ms: crate::anim::DOCK_MS,
            zoom_ms: 120.0,
            drag_zoom: 1.15,
            // Windows 11's own flyouts (calendar, quick settings) sit 12 px from the taskbar
            panel_margin: 12.0,
            tint_dark: tint_levels(false),
            tint_light: tint_levels(true),
            tint_fixed: None,
        }
    }
}

fn levels_json(t: TintLevels) -> Value {
    json!({ "quiet": num(t.quiet), "active": num(t.active), "hover": num(t.hover) })
}

/// A `{ "quiet", "active", "hover" }` object (each 0..1; a missing or broken one keeps `d`'s).
fn levels_from(v: Option<&Value>, d: TintLevels) -> TintLevels {
    let get = |k: &str, def: f32| v.and_then(|v| v.get(k)).and_then(|x| x.as_f64()).map_or(def, |x| (x as f32).clamp(0.0, 1.0));
    TintLevels { quiet: get("quiet", d.quiet), active: get("active", d.active), hover: get("hover", d.hover) }
}

/// Baselines of the taskbar clock's two lines (time, date), measured on screen: 4 px above and
/// 12 px below the middle of the taskbar at 100%. Two-line widgets put their lines there.
pub const CLOCK_BASELINES: (f32, f32) = (-4.0, 12.0);

/// Size of the tray's icons (network, volume, …) at 100%: a 16 px box centred on the middle
/// of the taskbar (measured on screen: rows 16–31 of a 48 px taskbar). Widget icons and badges
/// use the same box so they line up with the tray (see draw::icon_box).
pub const TRAY_ICON: f32 = 16.0;

/// f32 → JSON number without float noise (1.15, not 1.149999976158142).
fn num(v: f32) -> Value {
    json!(((v as f64) * 1000.0).round() / 1000.0)
}

fn follow_json<T: Into<Value> + Copy>(f: Follow<T>) -> Value {
    match f {
        Follow::System => json!("system"),
        Follow::Fixed(v) => {
            let v: Value = v.into();
            v.as_f64().filter(|_| v.is_f64()).map_or(v, |x| json!((x * 1000.0).round() / 1000.0))
        }
    }
}

impl Style {
    fn to_json(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("gap".into(), num(self.gap));
        m.insert("font".into(), json!(self.font));
        m.insert("font_size".into(), num(self.font_size));
        m.insert("text_size".into(), follow_json(self.text_size));
        m.insert("control_radius".into(), num(self.control_radius));
        m.insert(
            "time_format".into(),
            json!(match self.time_format {
                TimeFormat::System => "system",
                TimeFormat::H12 => "12h",
                TimeFormat::H24 => "24h",
            }),
        );
        m.insert("animations".into(), follow_json(self.animations));
        m.insert("hover_open_ms".into(), follow_json(self.hover_open_ms));
        m.insert("hover_close_ms".into(), json!(self.hover_close_ms));
        m.insert("fade_ms".into(), num(self.fade_ms));
        m.insert("glide_ms".into(), num(self.glide_ms));
        m.insert("zoom_ms".into(), num(self.zoom_ms));
        m.insert("drag_zoom".into(), num(self.drag_zoom));
        m.insert("panel_margin".into(), num(self.panel_margin));
        m.insert("tint_dark".into(), levels_json(self.tint_dark));
        m.insert("tint_light".into(), levels_json(self.tint_light));
        m.insert("tint_fixed".into(), self.tint_fixed.map_or(json!("auto"), num));
        m
    }

    fn from_json(dock: &Value) -> Style {
        let d = Style::default();
        let num = |k: &str, def: f32, lo: f32, hi: f32| dock.get(k).and_then(|v| v.as_f64()).map_or(def, |v| (v as f32).clamp(lo, hi));
        let is_system = |k: &str| dock.get(k).and_then(|v| v.as_str()).map_or(false, |s| s.eq_ignore_ascii_case("system"));
        Style {
            gap: num("gap", d.gap, 0.0, 64.0),
            font: dock.get("font").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map_or(d.font, String::from),
            font_size: num("font_size", d.font_size, 6.0, 48.0),
            text_size: if is_system("text_size") {
                Follow::System
            } else {
                Follow::Fixed(num("text_size", 1.0, 0.5, 3.0))
            },
            control_radius: num("control_radius", d.control_radius, 0.0, 16.0),
            time_format: match dock.get("time_format").and_then(|v| v.as_str()).unwrap_or("system").to_ascii_lowercase().as_str() {
                "12h" | "12" => TimeFormat::H12,
                "24h" | "24" => TimeFormat::H24,
                _ => TimeFormat::System,
            },
            animations: match dock.get("animations").and_then(|v| v.as_bool()) {
                Some(b) => Follow::Fixed(b),
                None => Follow::System,
            },
            hover_open_ms: if dock.get("hover_open_ms").and_then(|v| v.as_u64()).is_some() {
                Follow::Fixed(num("hover_open_ms", 400.0, 0.0, 5000.0) as u32)
            } else {
                Follow::System
            },
            hover_close_ms: num("hover_close_ms", d.hover_close_ms as f32, 0.0, 5000.0) as u32,
            fade_ms: num("fade_ms", d.fade_ms, 0.0, 2000.0),
            glide_ms: num("glide_ms", d.glide_ms, 0.0, 2000.0),
            zoom_ms: num("zoom_ms", d.zoom_ms, 0.0, 2000.0),
            drag_zoom: num("drag_zoom", d.drag_zoom, 1.0, 2.0),
            panel_margin: num("panel_margin", d.panel_margin, 0.0, 64.0),
            tint_dark: levels_from(dock.get("tint_dark"), d.tint_dark),
            tint_light: levels_from(dock.get("tint_light"), d.tint_light),
            tint_fixed: dock.get("tint_fixed").and_then(|v| v.as_f64()).map(|v| (v as f32).clamp(0.0, 1.0)),
        }
    }

    /// Loads the `dock` section, adding any missing keys (with defaults) to the file so every
    /// option is visible there. Leaves an invalid file alone and returns the defaults.
    pub fn load() -> Style {
        let Some(root) = settings::read() else { return Style::default() };
        let dock = root.get("dock").cloned().unwrap_or_else(|| json!({}));
        let style = Style::from_json(&dock);
        // fill in missing keys, keeping the user's values (and any keys we don't know)
        let mut full = dock.as_object().cloned().unwrap_or_default();
        let mut added = false;
        for (k, v) in style.to_json() {
            if !full.contains_key(&k) {
                full.insert(k, v);
                added = true;
            }
        }
        if added {
            let _ = settings::update(|r| r["dock"] = Value::Object(full));
        }
        style
    }

    /// The concrete values to use now, with "system" settings taken from Windows.
    pub fn resolve(&self, sys: &System) -> Look {
        let animations = match self.animations {
            Follow::System => sys.animations,
            Follow::Fixed(b) => b,
        };
        let anim = |ms: f32| if animations { ms } else { 0.0 };
        let (pattern, am, pm) = match self.time_format {
            TimeFormat::System => (sys.time_pattern.clone(), sys.am.clone(), sys.pm.clone()),
            TimeFormat::H12 => ("h:mm tt".into(), sys.am.clone(), sys.pm.clone()),
            TimeFormat::H24 => ("HH:mm".into(), String::new(), String::new()),
        };
        Look {
            gap: self.gap,
            font: self.font.clone(),
            font_size: self.font_size
                * match self.text_size {
                    Follow::System => sys.text_scale,
                    Follow::Fixed(f) => f,
                },
            control_radius: self.control_radius,
            light: sys.light,
            animations,
            hover_open_ms: match self.hover_open_ms {
                Follow::System => sys.hover_ms,
                Follow::Fixed(ms) => ms,
            },
            hover_close_ms: self.hover_close_ms,
            fade_ms: anim(self.fade_ms),
            glide_ms: anim(self.glide_ms),
            zoom_ms: anim(self.zoom_ms),
            drag_zoom: if animations { self.drag_zoom } else { 1.0 },
            panel_margin: self.panel_margin,
            tint: match self.tint_fixed {
                Some(v) => TintLevels { quiet: v, active: v, hover: v },
                None if sys.light => self.tint_light,
                None => self.tint_dark,
            },
            time_pattern: pattern,
            am,
            pm,
        }
    }
}

/// Everything a widget needs to look and behave like the others, resolved for right now.
/// Sizes are in px at 100%; multiply by the display scale ([`system::scale_for`]).
#[derive(Clone, PartialEq, Debug)]
pub struct Look {
    pub gap: f32,
    pub font: String,
    /// Body text size (taskbar lines), text size setting applied.
    pub font_size: f32,
    pub control_radius: f32,
    /// Taskbar in light mode: dark text, darker hover.
    pub light: bool,
    pub animations: bool,
    pub hover_open_ms: u32,
    pub hover_close_ms: u32,
    /// Durations are 0 when animations are off (things then jump straight to the end).
    pub fade_ms: f32,
    pub glide_ms: f32,
    pub zoom_ms: f32,
    pub drag_zoom: f32,
    /// Gap between a panel and the taskbar / screen edges.
    pub panel_margin: f32,
    /// Brand tint strengths for this taskbar (light or dark; all one with `tint_fixed`).
    pub tint: TintLevels,
    pub time_pattern: String,
    pub am: String,
    pub pm: String,
}

impl Look {
    /// Current look: the `dock` settings resolved against Windows' current settings.
    pub fn current() -> Look {
        Style::load().resolve(&system::read())
    }

    /// A time of day in the chosen format, e.g. "7:45 PM" or "19:45".
    pub fn time(&self, hour: u32, minute: u32) -> String {
        system::format_time(&self.time_pattern, &self.am, &self.pm, hour, minute, None)
    }

    /// A whole hour, compact like Windows' own weather: "9 PM" in 12-hour formats, "21:00" in
    /// 24-hour ones.
    pub fn hour(&self, hour: u32) -> String {
        if self.time_pattern.contains('t') {
            // drop the minutes (and their separator) from the pattern: "h:mm tt" → "h tt"
            let p = self.time_pattern.replace(":mm", "").replace(".mm", "").replace(":m", "");
            system::format_time(&p, &self.am, &self.pm, hour, 0, None)
        } else {
            self.time(hour, 0)
        }
    }

    /// Same with seconds, e.g. "7:45:12 PM".
    pub fn time_with_seconds(&self, hour: u32, minute: u32, second: u32) -> String {
        system::format_time(&self.time_pattern, &self.am, &self.pm, hour, minute, Some(second))
    }

    /// Body text colour (RGB) on the taskbar, and the dimmer one for a first/caption line.
    pub fn text_rgb(&self) -> ((u32, u32, u32), (u32, u32, u32)) {
        if self.light {
            ((0x1a, 0x1a, 0x1a), (0x5a, 0x5a, 0x5a))
        } else {
            ((0xff, 0xff, 0xff), (0xc8, 0xc8, 0xc8))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(dock: Value, light: bool) -> Look {
        let mut sys = system::read();
        sys.light = light;
        Style::from_json(&dock).resolve(&sys)
    }

    #[test]
    fn tint_levels_default_and_follow_the_taskbar() {
        assert_eq!(look(json!({}), false).tint, tint_levels(false));
        assert_eq!(look(json!({}), true).tint, tint_levels(true));
        // written to the file in full, "auto" for the fixed level
        let m = Style::default().to_json();
        assert_eq!(m["tint_fixed"], json!("auto"));
        assert_eq!(m["tint_dark"]["active"], json!(0.333));
        assert_eq!(Style::from_json(&Value::Object(m)).tint_dark.hover, (tint_levels(false).hover * 1000.0).round() / 1000.0);
    }

    #[test]
    fn tint_levels_are_configurable() {
        let dock = json!({ "tint_dark": { "quiet": 0.1, "active": 0.5, "hover": 0.9 }, "tint_light": { "active": 0.4 } });
        assert_eq!(look(dock.clone(), false).tint, TintLevels { quiet: 0.1, active: 0.5, hover: 0.9 });
        // a missing key keeps its default; out of range is clamped; nonsense is ignored
        let l = look(dock, true).tint;
        assert_eq!((l.quiet, l.active, l.hover), (tint_levels(true).quiet, 0.4, tint_levels(true).hover));
        let odd = look(json!({ "tint_dark": { "quiet": -1, "active": 7, "hover": "x" } }), false).tint;
        assert_eq!((odd.quiet, odd.active, odd.hover), (0.0, 1.0, tint_levels(false).hover));
    }

    #[test]
    fn a_fixed_tint_is_one_strength_for_every_state() {
        for (v, want) in [(json!(0), 0.0), (json!(1), 1.0), (json!(0.25), 0.25), (json!(3), 1.0)] {
            let t = look(json!({ "tint_fixed": v }), false).tint;
            assert_eq!((t.quiet, t.active, t.hover), (want, want, want));
        }
        assert_eq!(look(json!({ "tint_fixed": "auto" }), false).tint, tint_levels(false));
    }
}
