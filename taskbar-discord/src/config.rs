use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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
    /// Loads the section and writes it back, so new settings show up in the file.
    pub fn load() -> Config {
        // keep a broken file as-is so the user can fix their edit
        let Some(root) = settings::read() else { return Config::default() };
        let cfg: Config = root.get("discord").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        cfg.save();
        cfg
    }

    /// Saves these settings into the discord section; other keys there (Block's `compact` and
    /// `show_on_taskbar`) and the other sections are left as they are.
    pub fn save(&self) {
        let Ok(Value::Object(mine)) = serde_json::to_value(self) else { return };
        let _ = settings::update(|root| {
            if !root.is_object() {
                *root = json!({});
            }
            let sec = &mut root["discord"];
            if !sec.is_object() {
                *sec = json!({});
            }
            for (k, v) in mine {
                sec[k.as_str()] = v;
            }
        });
    }

    pub fn configured(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty()
    }

    /// What the RPC connection depends on (a change reconnects).
    pub fn connection_key(&self) -> (String, String, String) {
        (self.client_id.trim().into(), self.client_secret.trim().into(), self.redirect_uri.trim().into())
    }
}
