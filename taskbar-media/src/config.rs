//! The media widget's settings: the "media" section of the settings file shared with the other
//! taskbar widgets, %APPDATA%\TaskbarDock\settings.json (see the taskbar-dock library). The
//! shared look and behaviour (gap, font, timings, "follow Windows" switches) is its "dock"
//! section, read through taskbar_dock::style. Block owns the section's `compact` and
//! `show_on_taskbar` keys, and Start with Windows is the shared `startup` list.

use serde::{Deserialize, Serialize};
use taskbar_dock::settings;
use taskbar_dock::describe::{Kind, Setting};

/// Returns the settings exposed by the media widget.
pub fn settings() -> Vec<Setting> {
    vec![
        Setting::number("max_width", "Maximum strip width", 60., 400., 1.)
            .unit("px")
            .group("Strip"),
        Setting::new("fixed_width", "Fixed width", Kind::Switch).group("Strip"),
        Setting::number("click_panel_width", "Panel width", 280., 600., 1.)
            .unit("px")
            .group("Panel"),
        Setting::number("hide_after_pause_s", "Hide after pause", 0., 3600., 5.)
            .integer()
            .unit("s")
            .group("Playback"),
        Setting::number("skip_seconds", "Skip interval", 1., 60., 1.)
            .integer()
            .unit("s")
            .group("Playback"),
        Setting::new(
            "tint",
            "Tint",
            Kind::Choice {
                options: vec![
                    (serde_json::json!("brand"), "Brand".into()),
                    (serde_json::json!("cover"), "Cover".into()),
                    (serde_json::json!("off"), "Off".into()),
                ],
                other: false,
            },
        )
        .group("Colour"),
        Setting::number("tint_strength", "Tint strength", 0., 3., 0.05).group("Colour"),
        Setting::new(
            "live_color",
            "Live colour",
            Kind::Color {
                hash: true,
                empty_ok: false,
            },
        )
        .group("Colour"),
    ]
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Config {
    /// Widest the title/artist text gets on the strip (px at 100%); longer is cut with "…".
    pub max_width: f32,
    /// true: the text column is always `max_width` (the strip never changes width between
    /// tracks); false: as wide as the text.
    pub fixed_width: bool,
    /// Fixed width of the click panel (left click), px at 100%.
    pub click_panel_width: f32,
    /// Nothing playing: the paused session stays shown this long, then the strip hides.
    pub hide_after_pause_s: u32,
    /// The −/+ buttons for apps without previous/next (single videos).
    pub skip_seconds: u32,
    /// Strip and panel tint: "brand", "cover" or "off".
    pub tint: String,
    pub tint_strength: f32,
    /// The progress ring for a live stream.
    pub live_color: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_width: 136.0,
            fixed_width: false,
            click_panel_width: 360.0,
            hide_after_pause_s: 120,
            skip_seconds: 15,
            tint: "brand".into(),
            tint_strength: 1.0,
            live_color: format!("#{:06X}", taskbar_dock::theme::brand("media.live", true).rgb24()),
        }
    }
}

impl Config {
    /// Loads the media section, filling only missing keys on startup.
    pub fn load() -> Config {
        let defaults = serde_json::to_value(Config::default()).unwrap_or_default();
        let _ = settings::fill_missing("media", &defaults);
        Config::read()
    }

    /// Reads the media section without writing it. A broken file uses defaults.
    pub fn read() -> Config {
        let Ok(root) = settings::read() else { return Config::default() };
        root.get("media").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    /// `live_color` as RGB; the default if it isn't "#RRGGBB".
    pub fn live_rgb(&self) -> (u32, u32, u32) {
        parse_hex(&self.live_color).unwrap_or(taskbar_dock::theme::brand("media.live", true).tuple())
    }
}

/// "#RRGGBB" (or "RRGGBB") → (r, g, b).
pub fn parse_hex(s: &str) -> Option<(u32, u32, u32)> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some((v >> 16, (v >> 8) & 0xff, v & 0xff))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_keys_get_defaults() {
        let c: Config = serde_json::from_value(json!({ "max_width": 120.0, "compact": true })).unwrap();
        assert_eq!(c.max_width, 120.0);
        assert_eq!(c.hide_after_pause_s, 120);
        assert_eq!(c.live_color, "#FF4E45");
    }

    #[test]
    fn hex_colours() {
        assert_eq!(parse_hex("#FF4E45"), Some((0xff, 0x4e, 0x45)));
        assert_eq!(parse_hex("1ed760"), Some((0x1e, 0xd7, 0x60)));
        assert_eq!(parse_hex("red"), None);
    }
}
