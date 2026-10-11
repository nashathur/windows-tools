//! The media widget's settings: the "media" section of the settings file shared with the other
//! taskbar widgets, %APPDATA%\TaskbarDock\settings.json (see the taskbar-dock library). The
//! shared look and behaviour (gap, font, timings, "follow Windows" switches) is its "dock"
//! section, read through taskbar_dock::style. Block owns the section's `compact` and
//! `show_on_taskbar` keys, and Start with Windows is the shared `startup` list.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use taskbar_dock::settings;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Config {
    /// Widest the title/artist text gets on the strip (px at 100%); longer is cut with "…".
    pub max_width: f32,
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
            click_panel_width: 360.0,
            hide_after_pause_s: 120,
            skip_seconds: 15,
            tint: "brand".into(),
            tint_strength: 1.0,
            live_color: format!("#{:06X}", taskbar_dock::theme::brand("media.live", true).rgb24()),
        }
    }
}

/// Keys of older versions, removed from the file once (plan step 8, media on Block): the menu
/// panel is Block's shared one, the strip's fade is Block's (`glide_ms`), Start with Windows is
/// the `startup` list.
const GONE: [&str; 3] = ["menu_panel_width", "fade_ms", "start_with_windows"];

impl Config {
    /// Loads the media section and writes it back, which adds settings introduced by newer
    /// versions. A broken file is left as-is.
    pub fn load() -> Config {
        let Some(root) = settings::read() else { return Config::default() };
        let cfg: Config = root.get("media").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        cfg.save();
        cfg
    }

    /// Saves these settings into the media section; other keys there (Block's `compact` and
    /// `show_on_taskbar`) and the other sections are left as they are.
    pub fn save(&self) {
        let Ok(Value::Object(mine)) = serde_json::to_value(self) else { return };
        let _ = settings::update(|root| {
            if !root.is_object() {
                *root = json!({});
            }
            let sec = &mut root["media"];
            if !sec.is_object() {
                *sec = json!({});
            }
            for (k, v) in mine {
                sec[k.as_str()] = v;
            }
            if let Some(sec) = sec.as_object_mut() {
                for k in GONE {
                    sec.remove(k);
                }
            }
        });
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
