//! The voice call as Discord's RPC reports it: the channel, who is in it, who is speaking, and
//! our own mute/deafen. Plain data and the rules that apply RPC payloads to it (tested here);
//! the pipe and the commands are in rpc.rs.

use serde_json::Value;

/// Where the connection to Discord stands.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Status {
    /// No client_id / client_secret in the settings yet.
    #[default]
    NotConfigured,
    /// Discord isn't running (no RPC pipe).
    NoDiscord,
    Connecting,
    /// Discord shows its "Authorize" popup; waiting for the user.
    Authorizing,
    /// Signed out (declined, or the token stopped working): "Sign in to Discord" in the menu.
    NeedsSignIn(String),
    Error(String),
    Ready,
}

impl Status {
    /// Something the user has to act on: the strip shows even outside a call.
    pub fn needs_user(&self) -> bool {
        matches!(self, Status::NotConfigured | Status::Authorizing | Status::NeedsSignIn(_) | Status::Error(_))
    }

    pub fn text(&self) -> String {
        match self {
            Status::NotConfigured => "Not set up".into(),
            Status::NoDiscord => "Discord isn't running".into(),
            Status::Connecting => "Connecting…".into(),
            Status::Authorizing => "Authorize in Discord".into(),
            Status::NeedsSignIn(why) => format!("Sign in needed: {why}"),
            Status::Error(e) => e.clone(),
            Status::Ready => "Connected".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Member {
    pub id: String,
    /// Server nickname, else global display name, else username.
    pub name: String,
    /// Avatar hash (None: Discord's default avatar).
    pub avatar: Option<String>,
    pub speaking: bool,
    /// Muted by themselves or by the server.
    pub muted: bool,
    pub deafened: bool,
    /// Muted locally by us (right-click → Mute in Discord).
    pub local_mute: bool,
}

impl Member {
    /// The avatar's CDN URL (PNG, 64 px), or Discord's default avatar for this user.
    pub fn avatar_url(&self) -> String {
        match &self.avatar {
            Some(h) => format!("https://cdn.discordapp.com/avatars/{}/{h}.png?size=64", self.id),
            None => {
                let n = self.id.parse::<u64>().map_or(0, |id| (id >> 22) % 6);
                format!("https://cdn.discordapp.com/embed/avatars/{n}.png")
            }
        }
    }

    /// The cache key for the avatar image (changes when the user changes their avatar).
    pub fn avatar_key(&self) -> String {
        format!("{}_{}", self.id, self.avatar.as_deref().unwrap_or("default"))
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub guild_id: Option<String>,
    /// The server's name (None: a DM or group call, or not looked up yet).
    pub guild: Option<String>,
    /// The server's logo as a PNG URL (None: a DM or group call, no logo, or not looked up yet).
    pub guild_icon: Option<String>,
    /// In Discord's order: by name.
    pub members: Vec<Member>,
}

/// Everything the widget shows. The RPC worker keeps one and sends a copy when it changes.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Voice {
    pub status: Status,
    pub channel: Option<Channel>,
    /// Our own user id (from AUTHENTICATE).
    pub me: Option<String>,
    pub self_mute: bool,
    pub self_deaf: bool,
}

impl Voice {
    pub fn in_call(&self) -> bool {
        self.channel.is_some()
    }

    pub fn anyone_speaking(&self) -> bool {
        self.channel.as_ref().is_some_and(|c| c.members.iter().any(|m| m.speaking))
    }

    pub fn count(&self) -> usize {
        self.channel.as_ref().map_or(0, |c| c.members.len())
    }

    /// GET_SELECTED_VOICE_CHANNEL's answer: the channel with everyone in it, or null.
    pub fn set_channel(&mut self, data: &Value) {
        if data.is_null() || data.get("id").is_none() {
            self.channel = None;
            return;
        }
        let guild_id = str_of(data, "guild_id");
        let same = self.channel.as_ref().filter(|c| c.guild_id == guild_id);
        let (old_guild, old_icon) = (same.and_then(|c| c.guild.clone()), same.and_then(|c| c.guild_icon.clone()));
        let mut members: Vec<Member> = data
            .get("voice_states")
            .and_then(Value::as_array)
            .map(|v| v.iter().filter_map(member_of).collect())
            .unwrap_or_default();
        sort(&mut members);
        self.channel = Some(Channel {
            id: str_of(data, "id").unwrap_or_default(),
            name: str_of(data, "name").unwrap_or_default(),
            guild_id,
            guild: old_guild,
            guild_icon: old_icon,
            members,
        });
    }

    /// VOICE_STATE_CREATE / VOICE_STATE_UPDATE: someone joined or changed (mute, nick, …).
    pub fn upsert(&mut self, data: &Value) {
        let (Some(c), Some(mut m)) = (self.channel.as_mut(), member_of(data)) else { return };
        match c.members.iter_mut().find(|x| x.id == m.id) {
            Some(old) => {
                m.speaking = old.speaking;
                *old = m;
            }
            None => c.members.push(m),
        }
        sort(&mut c.members);
    }

    /// VOICE_STATE_DELETE: someone left.
    pub fn remove(&mut self, data: &Value) {
        let id = data.pointer("/user/id").and_then(Value::as_str);
        if let (Some(c), Some(id)) = (self.channel.as_mut(), id) {
            c.members.retain(|m| m.id != id);
        }
    }

    /// SPEAKING_START / SPEAKING_STOP.
    pub fn speaking(&mut self, data: &Value, on: bool) {
        let Some(id) = str_of(data, "user_id") else { return };
        if let Some(m) = self.channel.as_mut().and_then(|c| c.members.iter_mut().find(|m| m.id == id)) {
            m.speaking = on;
        }
    }

    /// GET_VOICE_SETTINGS / VOICE_SETTINGS_UPDATE: our own mute and deafen.
    pub fn settings(&mut self, data: &Value) {
        if let Some(v) = data.get("mute").and_then(Value::as_bool) {
            self.self_mute = v;
        }
        if let Some(v) = data.get("deaf").and_then(Value::as_bool) {
            self.self_deaf = v;
        }
    }

    pub fn set_guild_icon(&mut self, guild_id: &str, icon_url: &str) {
        if let Some(c) = self.channel.as_mut().filter(|c| c.guild_id.as_deref() == Some(guild_id)) {
            c.guild_icon = Some(guild_icon_png(icon_url));
        }
    }

    pub fn set_guild(&mut self, guild_id: &str, name: &str) {
        if let Some(c) = self.channel.as_mut().filter(|c| c.guild_id.as_deref() == Some(guild_id)) {
            c.guild = Some(name.to_string());
        }
    }
}

/// GET_GUILD's `icon_url` (JPG, WebP or GIF, any size) as the 64 px PNG the avatar decoder reads.
pub fn guild_icon_png(url: &str) -> String {
    let base = url.split('?').next().unwrap_or(url);
    let stem = base.rsplit_once('.').filter(|(_, ext)| ext.len() <= 4 && !ext.contains('/')).map_or(base, |(a, _)| a);
    format!("{stem}.png?size=64")
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

/// One entry of `voice_states` (also the payload of VOICE_STATE_* events).
fn member_of(v: &Value) -> Option<Member> {
    let user = v.get("user")?;
    let id = str_of(user, "id")?;
    let vs = v.get("voice_state").cloned().unwrap_or(Value::Null);
    let flag = |k: &str| vs.get(k).and_then(Value::as_bool).unwrap_or(false);
    let name = str_of(v, "nick")
        .or_else(|| str_of(user, "global_name"))
        .or_else(|| str_of(user, "username"))
        .unwrap_or_else(|| id.clone());
    Some(Member {
        name,
        avatar: str_of(user, "avatar"),
        speaking: false,
        muted: flag("mute") || flag("self_mute") || flag("suppress"),
        deafened: flag("deaf") || flag("self_deaf"),
        local_mute: v.get("mute").and_then(Value::as_bool).unwrap_or(false),
        id,
    })
}

fn sort(members: &mut [Member]) {
    members.sort_by_key(|m| m.name.to_lowercase());
}

/// Who gets the strip's few avatar places. When everyone fits, everyone has one (muted or not),
/// in the call's order. Otherwise people who can talk (not muted or deafened) compete for them:
/// whoever speaks takes an empty place, else the place of the one quiet longest. The places keep
/// their order, so the avatars don't reshuffle each time someone speaks; places nobody has won
/// yet go to the others who can talk, in the call's order.
#[derive(Clone, Debug, Default)]
pub struct Slots {
    /// (member id, the update they last spoke in; 0: not yet)
    held: Vec<(String, u64)>,
    tick: u64,
}

impl Slots {
    /// Called with each new state of the call.
    pub fn update(&mut self, members: &[Member], max: usize) {
        let max = max.max(1);
        self.tick += 1;
        let tick = self.tick;
        if members.len() <= max {
            // everyone fits: no "+N" for the muted
            let old = std::mem::take(&mut self.held);
            let last = |m: &Member| if m.speaking { tick } else { old.iter().find(|(id, _)| *id == m.id).map_or(0, |h| h.1) };
            self.held = members.iter().map(|m| (m.id.clone(), last(m))).collect();
            return;
        }
        let can_talk = |m: &Member| !m.muted && !m.deafened;
        let find = |id: &str| members.iter().find(|m| m.id == id);
        self.held.retain(|(id, _)| find(id).is_some_and(can_talk));
        self.held.truncate(max);
        for (id, last) in &mut self.held {
            if find(id).is_some_and(|m| m.speaking) {
                *last = tick;
            }
        }
        for m in members.iter().filter(|m| can_talk(m) && m.speaking) {
            if self.held.iter().any(|(id, _)| *id == m.id) {
                continue;
            }
            if self.held.len() < max {
                self.held.push((m.id.clone(), tick));
            } else if let Some(i) = (0..self.held.len()).filter(|&i| self.held[i].1 < tick).min_by_key(|&i| self.held[i].1) {
                self.held[i] = (m.id.clone(), tick);
            }
        }
        for m in members.iter().filter(|m| can_talk(m)) {
            if self.held.len() >= max {
                break;
            }
            if !self.held.iter().any(|(id, _)| *id == m.id) {
                self.held.push((m.id.clone(), 0));
            }
        }
    }

    /// The people in the places, in place order.
    pub fn people(&self, members: &[Member]) -> Vec<Member> {
        self.held.iter().filter_map(|(id, _)| members.iter().find(|m| m.id == *id).cloned()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(id: &str, name: &str, self_mute: bool) -> Value {
        json!({
            "nick": name, "mute": false, "volume": 100,
            "voice_state": { "mute": false, "deaf": false, "self_mute": self_mute, "self_deaf": false, "suppress": false },
            "user": { "id": id, "username": format!("user{id}"), "avatar": null, "bot": false }
        })
    }

    fn joined() -> Voice {
        let mut v = Voice::default();
        v.set_channel(&json!({
            "id": "10", "name": "General", "guild_id": "1", "type": 2,
            "voice_states": [state("2", "zed", false), state("3", "Amy", true)]
        }));
        v
    }

    #[test]
    fn channel_with_members_sorted_by_name() {
        let v = joined();
        let c = v.channel.as_ref().unwrap();
        assert_eq!(c.name, "General");
        assert_eq!(c.members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Amy", "zed"]);
        assert!(c.members[0].muted && !c.members[1].muted);
        assert_eq!(v.count(), 2);
    }

    #[test]
    fn null_channel_leaves_the_call() {
        let mut v = joined();
        v.set_channel(&Value::Null);
        assert!(!v.in_call());
    }

    #[test]
    fn join_update_leave() {
        let mut v = joined();
        v.upsert(&state("4", "Bob", false));
        assert_eq!(v.count(), 3);
        v.speaking(&json!({ "user_id": "4" }), true);
        // an update (now muted) keeps the speaking state
        v.upsert(&state("4", "Bob", true));
        let bob = v.channel.as_ref().unwrap().members.iter().find(|m| m.id == "4").unwrap();
        assert!(bob.muted && bob.speaking);
        v.remove(&json!({ "user": { "id": "4" } }));
        assert_eq!(v.count(), 2);
    }

    #[test]
    fn speaking_start_and_stop() {
        let mut v = joined();
        assert!(!v.anyone_speaking());
        v.speaking(&json!({ "user_id": "2", "channel_id": "10" }), true);
        assert!(v.anyone_speaking());
        v.speaking(&json!({ "user_id": "2" }), false);
        assert!(!v.anyone_speaking());
        // someone not in the list: ignored
        v.speaking(&json!({ "user_id": "99" }), true);
        assert!(!v.anyone_speaking());
    }

    #[test]
    fn name_falls_back_to_global_name_then_username() {
        let mut v = joined();
        v.upsert(&json!({ "user": { "id": "5", "username": "u5", "global_name": "Five" }, "voice_state": {} }));
        v.upsert(&json!({ "user": { "id": "6", "username": "u6" }, "nick": "" }));
        let names: Vec<_> = v.channel.unwrap().members.into_iter().map(|m| m.name).collect();
        assert!(names.contains(&"Five".to_string()) && names.contains(&"u6".to_string()));
    }

    #[test]
    fn own_settings() {
        let mut v = Voice::default();
        v.settings(&json!({ "mute": true, "deaf": false, "input": {} }));
        assert!(v.self_mute && !v.self_deaf);
        v.settings(&json!({ "deaf": true }));
        assert!(v.self_mute && v.self_deaf);
    }

    #[test]
    fn guild_name_kept_while_in_the_same_server() {
        let mut v = joined();
        v.set_guild("1", "My server");
        v.set_channel(&json!({ "id": "11", "name": "Gaming", "guild_id": "1", "voice_states": [] }));
        assert_eq!(v.channel.as_ref().unwrap().guild.as_deref(), Some("My server"));
        v.set_channel(&json!({ "id": "12", "name": "Other", "guild_id": "7", "voice_states": [] }));
        assert_eq!(v.channel.unwrap().guild, None);
    }

    #[test]
    fn avatar_urls() {
        let mut m = Member { id: "198010416612376576".into(), ..Default::default() };
        assert_eq!(m.avatar_url(), format!("https://cdn.discordapp.com/embed/avatars/{}.png", (198010416612376576u64 >> 22) % 6));
        m.avatar = Some("abc".into());
        assert_eq!(m.avatar_url(), "https://cdn.discordapp.com/avatars/198010416612376576/abc.png?size=64");
        assert_eq!(m.avatar_key(), "198010416612376576_abc");
    }

    #[test]
    fn guild_icon_becomes_a_small_png() {
        assert_eq!(guild_icon_png("https://cdn.discordapp.com/icons/1/abc.jpg"), "https://cdn.discordapp.com/icons/1/abc.png?size=64");
        assert_eq!(guild_icon_png("https://cdn.discordapp.com/icons/1/a_abc.gif?size=128"), "https://cdn.discordapp.com/icons/1/a_abc.png?size=64");
    }

    fn people(n: usize) -> Vec<Member> {
        (0..n).map(|i| Member { id: i.to_string(), ..Default::default() }).collect()
    }

    fn ids(s: &Slots, ms: &[Member]) -> Vec<String> {
        s.people(ms).into_iter().map(|m| m.id).collect()
    }

    #[test]
    fn slots_skip_the_muted_and_go_to_speakers() {
        let mut ms = people(7);
        ms[1].muted = true;
        ms[3].deafened = true;
        let mut s = Slots::default();
        s.update(&ms, 3);
        assert_eq!(ids(&s, &ms), ["0", "2", "4"]);
        // 6 speaks: takes the place of the one quiet longest (0, the first never to have spoken)
        ms[6].speaking = true;
        s.update(&ms, 3);
        assert_eq!(ids(&s, &ms), ["6", "2", "4"]);
        // 5 speaks while 6 still does: 2 goes, 6 stays
        ms[5].speaking = true;
        s.update(&ms, 3);
        assert_eq!(ids(&s, &ms), ["6", "5", "4"]);
        // a muted speaker never gets in; someone muting leaves their place
        ms[6].speaking = false;
        ms[5].speaking = false;
        ms[1].speaking = true;
        ms[4].muted = true;
        s.update(&ms, 3);
        assert_eq!(ids(&s, &ms), ["6", "5", "0"]);
        // all speaking and the places full: nobody is pushed out
        for m in ms.iter_mut() {
            m.speaking = true;
        }
        ms[1].muted = false;
        s.update(&ms, 3);
        assert_eq!(ids(&s, &ms), ["6", "5", "0"]);
    }

    #[test]
    fn slots_show_everyone_when_everyone_fits() {
        let mut ms = people(5);
        ms[2].muted = true;
        ms[4].deafened = true;
        let mut s = Slots::default();
        s.update(&ms, 5);
        assert_eq!(ids(&s, &ms), ["0", "1", "2", "3", "4"], "five places, five people: all of them");
        // a sixth joins: the places go to those who can talk again
        let mut six = ms.clone();
        six.extend(people(6).into_iter().skip(5));
        s.update(&six, 5);
        assert_eq!(ids(&s, &six), ["0", "1", "3", "5"]);
        // back to five: everyone again
        s.update(&ms, 5);
        assert_eq!(ids(&s, &ms), ["0", "1", "2", "3", "4"]);
    }
}
