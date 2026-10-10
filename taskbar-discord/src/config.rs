use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use taskbar_dock::settings;

/// The "discord" section of the settings file shared with the other taskbar widgets,
/// %APPDATA%\TaskbarDock\settings.json. The shared look is its "dock" section (taskbar_dock::style).
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Your own Discord application (discord.com/developers/applications → OAuth2): its
    /// Client ID and Client Secret. Discord's local RPC only talks to a registered app.
    pub client_id: String,
    pub client_secret: String,
    /// Must be one of the app's OAuth2 redirects (Discord checks it when the code is exchanged).
    pub redirect_uri: String,
    /// Compact strip: one icon and the number of people in the call.
    pub compact: bool,
    /// Keep the strip on the taskbar outside a call (otherwise it appears when you join one).
    pub show_when_idle: bool,
    /// Avatars on the strip (the rest is "+N").
    pub max_avatars: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: "http://localhost".into(),
            compact: false,
            show_when_idle: false,
            max_avatars: 5,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        settings::path()
    }

    /// Loads the section and writes it back in full, so new settings show up in the file.
    pub fn load() -> Config {
        // keep a broken file as-is so the user can fix their edit
        let Some(root) = settings::read() else { return Config::default() };
        let cfg: Config = root.get("discord").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        cfg.save();
        cfg
    }

    /// Saves this section only.
    pub fn save(&self) {
        let value: Value = serde_json::to_value(self).unwrap_or_else(|_| json!({}));
        let _ = settings::save_section("discord", value);
    }

    pub fn configured(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty()
    }

    /// What the RPC connection depends on (a change reconnects).
    pub fn connection_key(&self) -> (String, String, String) {
        (self.client_id.trim().into(), self.client_secret.trim().into(), self.redirect_uri.trim().into())
    }
}
