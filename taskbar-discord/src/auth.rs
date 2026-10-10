//! Signing in: the OAuth token for Discord's RPC, exchanged and refreshed over HTTPS, and kept
//! encrypted for this Windows user (DPAPI) in %APPDATA%\TaskbarDock\discord-token.bin.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

/// What the widget asks for: connect over RPC, read voice state, and set our own mute/deafen.
pub const SCOPES: [&str; 3] = ["rpc", "rpc.voice.read", "rpc.voice.write"];
const TOKEN_URL: &str = "https://discord.com/api/oauth2/token";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Token {
    /// The app it belongs to (a different client_id needs a new sign-in).
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
}

impl Token {
    /// Expires within a day: refresh before using it.
    pub fn stale(&self) -> bool {
        self.expires_at < now() + 24 * 3600
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn path() -> std::path::PathBuf {
    taskbar_dock::settings::dir().join("discord-token.bin")
}

pub fn load(client_id: &str) -> Option<Token> {
    let bytes = unprotect(&std::fs::read(path()).ok()?)?;
    let t: Token = serde_json::from_slice(&bytes).ok()?;
    (t.client_id == client_id).then_some(t)
}

pub fn save(t: &Token) {
    let Ok(json) = serde_json::to_vec(t) else { return };
    if let Some(enc) = protect(&json) {
        let tmp = path().with_extension("tmp");
        if std::fs::write(&tmp, enc).is_ok() {
            let _ = std::fs::rename(tmp, path());
        }
    }
}

pub fn forget() {
    let _ = std::fs::remove_file(path());
}

fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("taskbar-discord/", env!("CARGO_PKG_VERSION")))
        .build())
}

/// The code from RPC AUTHORIZE → a token.
pub fn exchange(client_id: &str, secret: &str, redirect: &str, code: &str) -> Result<Token, String> {
    request(client_id, &[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect),
        ("client_id", client_id), ("client_secret", secret)])
}

pub fn refresh(client_id: &str, secret: &str, t: &Token) -> Result<Token, String> {
    request(client_id, &[("grant_type", "refresh_token"), ("refresh_token", &t.refresh_token),
        ("client_id", client_id), ("client_secret", secret)])
}

fn request(client_id: &str, form: &[(&str, &str)]) -> Result<Token, String> {
    let resp = agent()?.post(TOKEN_URL).send_form(form);
    let v: Value = match resp {
        Ok(r) => r.into_json().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(code, r)) => {
            // Discord says why ("invalid_grant", "invalid_client", …); never log the request
            let body: Value = r.into_json().unwrap_or(Value::Null);
            let why = body.get("error_description").or(body.get("error")).and_then(Value::as_str).unwrap_or("");
            return Err(format!("token request failed: HTTP {code} {why}").trim().to_string());
        }
        Err(e) => return Err(format!("token request failed: {e}")),
    };
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Ok(Token {
        client_id: client_id.to_string(),
        access_token: s("access_token").ok_or("no access_token in Discord's answer")?,
        refresh_token: s("refresh_token").unwrap_or_default(),
        expires_at: now() + v.get("expires_in").and_then(Value::as_u64).unwrap_or(7 * 24 * 3600),
    })
}

fn protect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, true)
}

fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, false)
}

fn dpapi(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        let ok = if encrypt {
            CryptProtectData(&input, None, None, None, None, 0, &mut out)
        } else {
            CryptUnprotectData(&input, None, None, None, None, 0, &mut out)
        };
        ok.ok()?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(out.pbData as *mut _));
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_round_trip() {
        let enc = protect(b"secret token").unwrap();
        assert_ne!(enc, b"secret token");
        assert_eq!(unprotect(&enc).unwrap(), b"secret token");
    }

    #[test]
    fn stale_within_a_day() {
        let t = |s| Token { client_id: "1".into(), access_token: "a".into(), refresh_token: "r".into(), expires_at: now() + s };
        assert!(t(3600).stale());
        assert!(!t(3 * 24 * 3600).stale());
    }
}
