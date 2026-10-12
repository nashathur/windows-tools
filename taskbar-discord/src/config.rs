//! The `discord` settings section.

use serde::{Deserialize, Serialize};
use taskbar_dock::settings;

/// The "discord" section of the settings file shared with the other taskbar widgets,
/// %APPDATA%\TaskbarDock\settings.json. The shared look is its "dock" section (taskbar_dock::style).
/// Block owns the section's `compact` and `show_on_taskbar` keys (its switches): not here.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Your own Discord application (discord.com/developers/applications → OAuth2): its
    /// Client ID and Client Secret. Discord's local RPC only talks to a registered app.
    pub client_id: String,
    pub client_secret: String,
    /// Must be one of the app's OAuth2 redirects (Discord checks it when the code is exchanged).
    pub redirect_uri: String,
    /// Avatars on the strip (the rest is "+N").
    pub max_avatars: usize,
    /// true: the avatar column is always media's `max_width` (the strip never changes width as
    /// people join or leave); false: as wide as the avatars of everyone in the call.
    pub fixed_width: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: "http://localhost".into(),
            max_avatars: 5,
            fixed_width: false,
        }
    }
}

impl Config {
    /// Loads the section, filling only missing keys on startup.
    pub fn load() -> Config {
        let defaults = serde_json::to_value(Config::default()).unwrap_or_default();
        let _ = settings::fill_missing("discord", &defaults);
        Config::read()
    }

    /// Reads the section without writing it. A broken file uses defaults.
    pub fn read() -> Config {
        let Ok(root) = settings::read() else { return Config::default() };
        root.get("discord").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    pub fn configured(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty()
    }

    /// What the RPC connection depends on (a change reconnects).
    pub fn connection_key(&self) -> (String, String, String) {
        (self.client_id.trim().into(), self.client_secret.trim().into(), self.redirect_uri.trim().into())
    }
}
