//! Discord's local RPC on a worker thread: the named pipe `\\.\pipe\discord-ipc-N`, sign-in
//! (AUTHORIZE → token → AUTHENTICATE), the voice events, and our commands (mute, deafen,
//! leave). The worker keeps a `Voice` and hands the UI a copy whenever it changes.
//!
//! Frames: op (u32 LE), length (u32 LE), JSON. Ops: 0 handshake, 1 frame, 2 close, 3 ping, 4 pong.
//! The pipe handle is synchronous, so one thread does everything: it peeks for incoming bytes
//! and otherwise sleeps briefly (a blocking read would also block our writes).

use crate::auth::{self, Token};
use crate::voice::{Status, Voice};
use serde_json::{json, Value};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Pipes::PeekNamedPipe;

/// What the UI asks the worker to do.
pub enum Cmd {
    Mute(bool),
    Deaf(bool),
    Leave,
    /// Show Discord's Authorize popup again (menu: Sign in to Discord).
    SignIn,
    /// The settings changed (client id, secret, redirect), or the menu's Reconnect.
    Reconnect(Key),
}

/// client_id, client_secret, redirect_uri.
pub type Key = (String, String, String);

pub type Post = Box<dyn Fn(Voice) + Send>;

const EVENTS_IN_CHANNEL: [&str; 5] = ["VOICE_STATE_CREATE", "VOICE_STATE_UPDATE", "VOICE_STATE_DELETE", "SPEAKING_START", "SPEAKING_STOP"];

pub fn start(key: Key, post: Post) -> Sender<Cmd> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || match std::env::var("TASKBAR_DISCORD_TEST") {
        Ok(n) => fake(n.parse().unwrap_or(4), &rx, &post),
        Err(_) => run(key, &rx, &post),
    });
    tx
}

// ---------------------------------------------------------------- frames

pub fn encode(op: u32, body: &Value) -> Vec<u8> {
    let json = body.to_string().into_bytes();
    let mut out = Vec::with_capacity(8 + json.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    out
}

pub fn read_frame(r: &mut impl Read) -> io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let op = u32::from_le_bytes(head[0..4].try_into().unwrap());
    let len = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
    if len > 16 << 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let v = serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok((op, v))
}

fn connect() -> Option<File> {
    (0..10).find_map(|i| File::options().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")).ok())
}

/// Bytes waiting in the pipe; Err when Discord closed it.
fn available(pipe: &File) -> io::Result<u32> {
    let mut n = 0u32;
    unsafe { PeekNamedPipe(HANDLE(pipe.as_raw_handle()), None, 0, None, Some(&mut n), None) }
        .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e.message()))?;
    Ok(n)
}

// ---------------------------------------------------------------- the worker

fn run(mut key: Key, rx: &Receiver<Cmd>, post: &Post) {
    let mut last = Voice::default();
    let mut send = |v: &Voice| {
        if *v != last {
            last = v.clone();
            post(v.clone());
        }
    };
    // the Authorize popup shows by itself only the first time (no token yet); after a decline
    // only from the menu, so it never nags
    let mut prompt = true;
    let mut logged = String::new();
    loop {
        let configured = !key.0.is_empty() && !key.1.is_empty();
        let status = if configured { Status::NoDiscord } else { Status::NotConfigured };
        if configured {
            if let Some(pipe) = connect() {
                if logged != "connected" {
                    crate::log("connected to Discord");
                    logged = "connected".into();
                }
                let mut s = Session { pipe, key: key.clone(), nonce: 0, voice: Voice { status: Status::Connecting, ..Default::default() },
                    channel_subs: None, tried_refresh: false, token: None };
                send(&s.voice);
                let end = s.run(rx, &mut prompt, &mut send);
                match end {
                    End::Reconnect(k) => {
                        key = k;
                        continue;
                    }
                    End::Closed(why) => {
                        if logged != why {
                            crate::log(&format!("Discord connection ended: {why}"));
                            logged = why.clone();
                        }
                        if !why.is_empty() && s.voice.status != Status::Ready {
                            // e.g. an invalid client id: show it rather than "not running"
                            send(&Voice { status: Status::Error(why), ..Default::default() });
                            if let Some(k) = wait(rx, Duration::from_secs(15), &mut prompt) {
                                key = k;
                            }
                            continue;
                        }
                    }
                }
            } else if logged != "no discord" {
                crate::log("Discord isn't running (no RPC pipe); retrying every 3 s");
                logged = "no discord".into();
            }
        }
        send(&Voice { status, ..Default::default() });
        // not configured: nothing to do until the settings change
        let pause = if configured { Duration::from_secs(3) } else { Duration::from_secs(3600) };
        if let Some(k) = wait(rx, pause, &mut prompt) {
            key = k;
        }
    }
}

/// Waits up to `d` for the next try, handling commands meanwhile; Some(new key) to reconnect now.
fn wait(rx: &Receiver<Cmd>, d: Duration, prompt: &mut bool) -> Option<Key> {
    let until = Instant::now() + d;
    loop {
        match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(Cmd::Reconnect(k)) => return Some(k),
            Ok(Cmd::SignIn) => *prompt = true,
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(d),
        }
    }
}

enum End {
    Reconnect(Key),
    /// Discord went away or closed the connection (with its reason, if it gave one).
    Closed(String),
}

struct Session {
    pipe: File,
    key: Key,
    nonce: u64,
    voice: Voice,
    /// The channel whose events we're subscribed to.
    channel_subs: Option<String>,
    tried_refresh: bool,
    token: Option<Token>,
}

impl Session {
    fn run(&mut self, rx: &Receiver<Cmd>, prompt: &mut bool, send: &mut impl FnMut(&Voice)) -> End {
        if let Err(e) = self.pipe.write_all(&encode(0, &json!({ "v": 1, "client_id": self.key.0 }))) {
            return End::Closed(format!("handshake failed: {e}"));
        }
        loop {
            loop {
                match rx.try_recv() {
                    Ok(Cmd::Reconnect(k)) => return End::Reconnect(k),
                    Ok(cmd) => self.command(cmd, prompt),
                    Err(_) => break,
                }
            }
            match available(&self.pipe) {
                Err(_) => return End::Closed(String::new()),
                Ok(n) if n >= 8 => match read_frame(&mut self.pipe) {
                    Ok((1, msg)) => self.message(&msg, prompt),
                    Ok((2, msg)) => {
                        let why = msg.get("message").and_then(Value::as_str).unwrap_or("closed by Discord");
                        return End::Closed(why.to_string());
                    }
                    Ok((3, msg)) => self.write(4, &msg),
                    Ok(_) => {}
                    Err(_) => return End::Closed(String::new()),
                },
                Ok(_) => std::thread::sleep(Duration::from_millis(15)),
            }
            send(&self.voice);
        }
    }

    fn write(&mut self, op: u32, v: &Value) {
        let _ = self.pipe.write_all(&encode(op, v));
    }

    fn cmd(&mut self, cmd: &str, args: Value, evt: Option<&str>) {
        self.nonce += 1;
        let mut msg = json!({ "cmd": cmd, "args": args, "nonce": self.nonce.to_string() });
        if let Some(e) = evt {
            msg["evt"] = json!(e);
        }
        self.write(1, &msg);
    }

    fn command(&mut self, cmd: Cmd, prompt: &mut bool) {
        let ready = self.voice.status == Status::Ready;
        match cmd {
            Cmd::Mute(on) if ready => self.cmd("SET_VOICE_SETTINGS", json!({ "mute": on }), None),
            Cmd::Deaf(on) if ready => self.cmd("SET_VOICE_SETTINGS", json!({ "deaf": on }), None),
            Cmd::Leave if ready => self.cmd("SELECT_VOICE_CHANNEL", json!({ "channel_id": null }), None),
            Cmd::SignIn => {
                auth::forget();
                self.token = None;
                *prompt = true;
                if self.voice.status != Status::Connecting {
                    self.sign_in(prompt);
                }
            }
            _ => {}
        }
    }

    /// After READY: authenticate with the saved token (refreshed if it's about to expire), or
    /// ask for authorization.
    fn sign_in(&mut self, prompt: &mut bool) {
        let (id, secret) = (self.key.0.clone(), self.key.1.clone());
        let mut token = self.token.take().or_else(|| auth::load(&id));
        if let Some(t) = token.as_ref().filter(|t| t.stale() && !t.refresh_token.is_empty()) {
            self.tried_refresh = true;
            token = match auth::refresh(&id, &secret, t) {
                Ok(n) => {
                    auth::save(&n);
                    Some(n)
                }
                Err(e) => {
                    crate::log(&format!("token refresh failed: {e}"));
                    None
                }
            };
        }
        match token {
            Some(t) => {
                self.cmd("AUTHENTICATE", json!({ "access_token": t.access_token }), None);
                self.token = Some(t);
            }
            None if *prompt => {
                *prompt = false;
                self.voice.status = Status::Authorizing;
                self.cmd("AUTHORIZE", json!({ "client_id": id, "scopes": auth::SCOPES }), None);
            }
            None => self.voice.status = Status::NeedsSignIn("right-click → Sign in to Discord".into()),
        }
    }

    fn message(&mut self, msg: &Value, prompt: &mut bool) {
        let cmd = msg.get("cmd").and_then(Value::as_str).unwrap_or("");
        let evt = msg.get("evt").and_then(Value::as_str).unwrap_or("");
        let data = msg.get("data").cloned().unwrap_or(Value::Null);
        let error = (evt == "ERROR").then(|| data.get("message").and_then(Value::as_str).unwrap_or("error").to_string());
        match (cmd, evt) {
            ("DISPATCH", "READY") => self.sign_in(prompt),
            ("AUTHORIZE", _) => match error {
                Some(e) => {
                    crate::log(&format!("authorization not given: {e}"));
                    self.voice.status = Status::NeedsSignIn(e);
                }
                None => {
                    let code = data.get("code").and_then(Value::as_str).unwrap_or("");
                    let (id, secret, redirect) = self.key.clone();
                    match auth::exchange(&id, &secret, &redirect, code) {
                        Ok(t) => {
                            auth::save(&t);
                            crate::log("signed in to Discord");
                            self.cmd("AUTHENTICATE", json!({ "access_token": t.access_token }), None);
                            self.token = Some(t);
                        }
                        Err(e) => {
                            crate::log(&e);
                            self.voice.status = Status::Error(e);
                        }
                    }
                }
            },
            ("AUTHENTICATE", _) => match error {
                Some(e) => {
                    crate::log(&format!("AUTHENTICATE failed: {e}"));
                    let t = self.token.take();
                    if let Some(t) = t.filter(|t| !self.tried_refresh && !t.refresh_token.is_empty()) {
                        // e.g. revoked or expired early: one refresh, then ask again
                        self.tried_refresh = true;
                        if let Ok(n) = auth::refresh(&self.key.0, &self.key.1, &t) {
                            auth::save(&n);
                            self.token = Some(n);
                            return self.sign_in(prompt);
                        }
                    }
                    auth::forget();
                    self.sign_in(prompt);
                    if let Status::NeedsSignIn(_) = self.voice.status {
                        self.voice.status = Status::NeedsSignIn(e);
                    }
                }
                None => {
                    self.voice.me = data.pointer("/user/id").and_then(Value::as_str).map(str::to_string);
                    self.voice.status = Status::Ready;
                    self.cmd("SUBSCRIBE", json!({}), Some("VOICE_CHANNEL_SELECT"));
                    self.cmd("SUBSCRIBE", json!({}), Some("VOICE_SETTINGS_UPDATE"));
                    self.cmd("GET_VOICE_SETTINGS", json!({}), None);
                    self.cmd("GET_SELECTED_VOICE_CHANNEL", json!({}), None);
                }
            },
            _ if error.is_some() => crate::log(&format!("{cmd} failed: {}", error.unwrap_or_default())),
            ("GET_SELECTED_VOICE_CHANNEL", _) => {
                self.voice.set_channel(&data);
                self.follow_channel();
                if let Some(g) = self.voice.channel.as_ref().filter(|c| c.guild.is_none()).and_then(|c| c.guild_id.clone()) {
                    self.cmd("GET_GUILD", json!({ "guild_id": g, "timeout": 5 }), None);
                }
            }
            ("GET_GUILD", _) => {
                if let (Some(id), Some(name)) = (data.get("id").and_then(Value::as_str), data.get("name").and_then(Value::as_str)) {
                    self.voice.set_guild(id, name);
                    if let Some(icon) = data.get("icon_url").and_then(Value::as_str).filter(|u| !u.is_empty()) {
                        self.voice.set_guild_icon(id, icon);
                    }
                }
            }
            ("GET_VOICE_SETTINGS", _) | ("SET_VOICE_SETTINGS", _) => self.voice.settings(&data),
            ("DISPATCH", "VOICE_SETTINGS_UPDATE") => self.voice.settings(&data),
            ("DISPATCH", "VOICE_CHANNEL_SELECT") => {
                if data.get("channel_id").map_or(true, Value::is_null) {
                    self.voice.set_channel(&Value::Null);
                    self.follow_channel();
                } else {
                    self.cmd("GET_SELECTED_VOICE_CHANNEL", json!({}), None);
                }
            }
            ("DISPATCH", "VOICE_STATE_CREATE") | ("DISPATCH", "VOICE_STATE_UPDATE") => self.voice.upsert(&data),
            ("DISPATCH", "VOICE_STATE_DELETE") => self.voice.remove(&data),
            ("DISPATCH", "SPEAKING_START") => self.voice.speaking(&data, true),
            ("DISPATCH", "SPEAKING_STOP") => self.voice.speaking(&data, false),
            _ => {}
        }
    }

    /// Subscribes to the events of the channel we're in (and unsubscribes from the last one).
    fn follow_channel(&mut self) {
        let now = self.voice.channel.as_ref().map(|c| c.id.clone());
        if now == self.channel_subs {
            return;
        }
        if let Some(old) = self.channel_subs.take() {
            for e in EVENTS_IN_CHANNEL {
                self.cmd("UNSUBSCRIBE", json!({ "channel_id": old }), Some(e));
            }
        }
        if let Some(id) = now {
            for e in EVENTS_IN_CHANNEL {
                self.cmd("SUBSCRIBE", json!({ "channel_id": id }), Some(e));
            }
            self.channel_subs = Some(id);
        }
    }
}

// ---------------------------------------------------------------- test aid

/// TASKBAR_DISCORD_TEST=<n>: a made-up call with n people, someone speaking in turn, so the
/// strip and panel can be checked without Discord. Mute/deafen/leave work on it.
fn fake(n: usize, rx: &Receiver<Cmd>, post: &Post) {
    let names = ["Amy", "Bob", "Chandra", "Dewi", "Eko", "Fatima", "Gus", "Hana"];
    let mut v = Voice { status: Status::Ready, me: Some("1".into()), ..Default::default() };
    let join = |v: &mut Voice| {
        let states: Vec<Value> = (0..n.max(1))
            .map(|i| json!({ "nick": names[i % names.len()], "voice_state": { "self_mute": i == 2 },
                             "user": { "id": (i + 1).to_string(), "username": "x" } }))
            .collect();
        v.set_channel(&json!({ "id": "10", "name": "General", "guild_id": "1", "voice_states": states }));
        v.set_guild("1", "Test server");
    };
    join(&mut v);
    let mut tick = 0usize;
    let mut left_at: Option<Instant> = None;
    loop {
        post(v.clone());
        match rx.recv_timeout(Duration::from_millis(900)) {
            Ok(Cmd::Mute(on)) => v.self_mute = on,
            Ok(Cmd::Deaf(on)) => v.self_deaf = on,
            Ok(Cmd::Leave) => {
                v.channel = None;
                left_at = Some(Instant::now());
            }
            Ok(_) => {}
            Err(_) => {
                tick += 1;
                if let Some(c) = v.channel.as_mut() {
                    let k = c.members.len().max(1);
                    for (i, m) in c.members.iter_mut().enumerate() {
                        m.speaking = i == tick % k || (tick % 5 == 0 && i == (tick + 1) % k);
                    }
                }
                if left_at.is_some_and(|t| t.elapsed() > Duration::from_secs(6)) {
                    left_at = None;
                    join(&mut v);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let body = json!({ "cmd": "SUBSCRIBE", "evt": "SPEAKING_START", "args": { "channel_id": "1" }, "nonce": "1" });
        let bytes = encode(1, &body);
        assert_eq!(&bytes[0..4], &1u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, bytes.len() - 8);
        let (op, v) = read_frame(&mut &bytes[..]).unwrap();
        assert_eq!((op, v), (1, body));
    }

    #[test]
    fn two_frames_back_to_back() {
        let mut bytes = encode(1, &json!({ "a": 1 }));
        bytes.extend(encode(3, &json!({ "b": 2 })));
        let mut r = &bytes[..];
        assert_eq!(read_frame(&mut r).unwrap().0, 1);
        assert_eq!(read_frame(&mut r).unwrap(), (3, json!({ "b": 2 })));
    }

    #[test]
    fn truncated_frame_is_an_error() {
        let bytes = encode(1, &json!({ "a": 1 }));
        assert!(read_frame(&mut &bytes[..bytes.len() - 1]).is_err());
    }
}
