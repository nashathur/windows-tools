//! dockctl commands for the media widget and their platform-independent parsing.

use crate::media::{self, Session};
use crate::timeline::Progress;
use crate::{audio, apps, realtime, App};
use crate::audio::Volume;
use serde_json::{json, Value};
use taskbar_dock::block::{CommandError, CommandReply, ReplyToken};

/// The media commands shown by dockctl's command list.
pub(crate) fn commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("now", "now [--app <name>]: show the current track and controls"),
        ("sessions", "sessions: list media sessions (one per app)"),
        ("play", "play [--app <name>]: start playback"),
        ("pause", "pause [--app <name>]: pause playback"),
        ("toggle", "toggle [--app <name>]: toggle playback"),
        ("next", "next [--app <name>]: skip to the next track"),
        ("prev", "prev [--app <name>]: skip to the previous track"),
        ("skip", "skip <±seconds> [--app <name>]: jump forward or back"),
        ("seek", "seek <seconds> [--app <name>]: seek from the start of the track"),
        ("volume", "volume <0-100> [--app <name>]: set the app volume"),
        ("mute", "mute on|off [--app <name>]: set the app mute state"),
    ]
}

/// A parsed media command. `app` can appear among the command arguments.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    Now { app: Option<String> },
    Sessions,
    Play { app: Option<String> },
    Pause { app: Option<String> },
    Toggle { app: Option<String> },
    Next { app: Option<String> },
    Prev { app: Option<String> },
    Skip { by: f64, app: Option<String> },
    Seek { to: f64, app: Option<String> },
    Volume { level: u8, app: Option<String> },
    Mute { muted: bool, app: Option<String> },
}

impl Command {
    /// The optional app selector, if this command carries one.
    pub(crate) fn app(&self) -> Option<&str> {
        match self {
            Self::Now { app } | Self::Play { app } | Self::Pause { app } | Self::Toggle { app }
            | Self::Next { app } | Self::Prev { app } | Self::Skip { app, .. }
            | Self::Seek { app, .. } | Self::Volume { app, .. } | Self::Mute { app, .. } => app.as_deref(),
            Self::Sessions => None,
        }
    }

    /// The playback action, if this command asks the media worker to act.
    pub(crate) fn action(&self) -> Option<media::Action> {
        match self {
            Self::Play { .. } => Some(media::Action::Play),
            Self::Pause { .. } => Some(media::Action::Pause),
            Self::Toggle { .. } => Some(media::Action::Toggle),
            Self::Next { .. } => Some(media::Action::Next),
            Self::Prev { .. } => Some(media::Action::Prev),
            Self::Skip { by, .. } => Some(media::Action::Skip(*by)),
            Self::Seek { to, .. } => Some(media::Action::Seek(*to)),
            Self::Now { .. } | Self::Sessions | Self::Volume { .. } | Self::Mute { .. } => None,
        }
    }
}

/// Parses arguments and removes `--app <name>` wherever it appears. Errors are short: Block adds
/// the command's usage line itself.
pub(crate) fn parse(name: &str, args: &[String]) -> Result<Command, String> {
    let bad = |message: &str| message.to_string();
    if name == "sessions" && !args.is_empty() {
        return Err(bad("sessions takes no arguments"));
    }
    let mut positional = Vec::new();
    let mut app = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--app" {
            if i + 1 == args.len() || args[i + 1].is_empty() {
                return Err(bad("--app needs a name"));
            }
            if app.is_some() {
                return Err(bad("--app given twice"));
            }
            app = Some(args[i + 1].clone());
            i += 2;
        } else {
            positional.push(args[i].as_str());
            i += 1;
        }
    }
    let no_args = positional.is_empty();
    Ok(match name {
        "now" if no_args => Command::Now { app },
        "sessions" if no_args => Command::Sessions,
        "play" if no_args => Command::Play { app },
        "pause" if no_args => Command::Pause { app },
        "toggle" if no_args => Command::Toggle { app },
        "next" if no_args => Command::Next { app },
        "prev" if no_args => Command::Prev { app },
        "skip" if positional.len() == 1 => {
            let by = positional[0].parse::<f64>().ok().filter(|n| n.is_finite() && *n != 0.0)
                .ok_or_else(|| bad("skip takes a non-zero number of seconds"))?;
            Command::Skip { by, app }
        }
        "seek" if positional.len() == 1 => {
            let to = positional[0].parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0)
                .ok_or_else(|| bad("seek takes a number of seconds >= 0"))?;
            Command::Seek { to, app }
        }
        "volume" if positional.len() == 1 => {
            let level = positional[0].parse::<u8>().ok().filter(|n| *n <= 100)
                .ok_or_else(|| bad("volume takes a whole number 0-100"))?;
            Command::Volume { level, app }
        }
        "mute" if positional.len() == 1 && matches!(positional[0], "on" | "off") => Command::Mute { muted: positional[0] == "on", app },
        "mute" => return Err(bad("mute takes on or off")),
        "now" | "play" | "pause" | "toggle" | "next" | "prev" | "sessions" => {
            return Err(bad(&format!("{name} takes no arguments")));
        }
        _ => return Err(bad(match name {
            "skip" => "skip takes a non-zero number of seconds",
            "seek" => "seek takes a number of seconds >= 0",
            "volume" => "volume takes a whole number 0-100",
            _ => "unknown command",
        })),
    })
}

/// Resolves an app id or a case-insensitive display-name prefix.
pub(crate) fn resolve<'a>(sessions: &'a [Session], shown: Option<&str>, app: Option<&str>) -> Result<&'a Session, String> {
    if sessions.is_empty() {
        return Err("no media sessions".into());
    }
    if let Some(query) = app {
        if let Some(session) = sessions.iter().find(|s| s.id == query) {
            return Ok(session);
        }
        let prefix = query.to_ascii_lowercase();
        let found: Vec<_> = sessions.iter().filter(|s| apps::display_name(&s.id).to_ascii_lowercase().starts_with(&prefix)).collect();
        return match found.as_slice() {
            [session] => Ok(*session),
            [] => Err(format!("no media session matches \"{query}\"; sessions: {}", list(sessions))),
            _ => Err(format!("\"{query}\" matches several sessions: {}", list(sessions))),
        };
    }
    match shown.and_then(|id| sessions.iter().find(|s| s.id == id)) {
        Some(session) => Ok(session),
        None if sessions.is_empty() => Err("no media sessions".into()),
        None => Err(format!("no session is shown; pick one with --app: {}", list(sessions))),
    }
}

fn list(sessions: &[Session]) -> String {
    sessions.iter().map(|s| format!("{} ({})", apps::display_name(&s.id), s.id)).collect::<Vec<_>>().join(", ")
}

/// Runs one dockctl media command using the latest UI-thread snapshot.
pub(crate) fn run(a: &mut App, name: &str, args: &[String], token: ReplyToken) -> CommandReply {
    let parsed = match parse(name, args) {
        Ok(command) => command,
        Err(message) => return CommandReply::Done(Err(CommandError { usage: true, message })),
    };
    if matches!(parsed, Command::Sessions) {
        return CommandReply::Done(Ok(sessions_json(&a.sessions, a.shown.as_deref())));
    }
    let session = match resolve(&a.sessions, a.shown.as_deref(), parsed.app()) {
        Ok(session) => session,
        Err(message) => return CommandReply::Done(Err(CommandError { usage: false, message })),
    };
    let id = session.id.clone();
    match parsed {
        Command::Now { .. } => {
            let entries = audio::entries(&id);
            CommandReply::Done(Ok(now_json(session, realtime(a, session), audio::get(&entries))))
        }
        Command::Volume { level, .. } => {
            let name = apps::display_name(&id);
            let asked = level as f32 / 100.0;
            result_reply(set_volume(&id, &name, |entries| audio::set_level(entries, asked), |v| (v.level - asked).abs() < 0.005))
        }
        Command::Mute { muted, .. } => {
            let name = apps::display_name(&id);
            result_reply(set_volume(&id, &name, |entries| audio::set_muted(entries, muted), |v| v.muted == muted))
        }
        command => match command.action() {
            Some(action) => {
                let Some(tx) = &a.media else {
                    return CommandReply::Done(Err(CommandError { usage: false, message: "the media worker isn't running".into() }));
                };
                let msg = media::Msg::Do(media::DoCmd { id, action, realtime: realtime(a, session), token });
                match tx.send(msg) {
                    Ok(()) => CommandReply::Pending,
                    Err(_) => CommandReply::Done(Err(CommandError { usage: false, message: "the media worker isn't running".into() })),
                }
            }
            None => CommandReply::Done(Err(CommandError { usage: false, message: "unknown command".into() })),
        },
    }
}

fn result_reply(result: Result<Value, String>) -> CommandReply {
    CommandReply::Done(result.map_err(|message| CommandError { usage: false, message }))
}

fn set_volume(
    id: &str,
    name: &str,
    change: impl FnOnce(&[windows::Win32::Media::Audio::ISimpleAudioVolume]),
    took: impl Fn(Volume) -> bool,
) -> Result<Value, String> {
    let entries = audio::entries(id);
    if entries.is_empty() {
        return Err(format!("{name} has no Volume Mixer entry (it hasn't played sound yet)"));
    }
    change(&entries);
    let Some(volume) = audio::get(&entries).filter(|volume| took(*volume)) else {
        return Err(format!("{name} didn't take the volume change"));
    };
    Ok(volume_json(name, volume))
}

/// Returns progress fields only when the reported position is trustworthy.
pub(crate) fn position_fields(progress: Progress, realtime: bool) -> (Option<f64>, Option<f64>) {
    match (progress, realtime) {
        (Progress::At { pos, len }, true) => (Some(pos), Some(len)),
        _ => (None, None),
    }
}

/// Builds the immediate `sessions` reply from the latest UI-thread snapshot.
pub(crate) fn sessions_json(sessions: &[Session], shown: Option<&str>) -> Value {
    json!({
        "ok": true,
        "sessions": sessions.iter().map(|s| json!({
            "id": s.id,
            "name": crate::apps::display_name(&s.id),
            "title": s.title,
            "artist": s.artist,
            "playing": s.playing,
            "shown": shown == Some(s.id.as_str()),
        })).collect::<Vec<_>>()
    })
}

/// Builds the `now` reply; volume is read from the fresh WASAPI lookup by the caller.
pub(crate) fn now_json(session: &Session, realtime: bool, volume: Option<Volume>) -> Value {
    let (position, length) = position_fields(session.progress(), realtime);
    json!({
        "ok": true,
        "id": session.id,
        "app": crate::apps::display_name(&session.id),
        "title": session.title,
        "artist": session.artist,
        "playing": session.playing,
        "position": position,
        "length": length,
        "volume": volume.map(|v| (v.level * 100.0).round() as u8),
        "muted": volume.map(|v| v.muted),
    })
}

/// Builds the confirmed volume or mute reply.
pub(crate) fn volume_json(name: &str, volume: Volume) -> Value {
    json!({ "ok": true, "app": name, "volume": (volume.level * 100.0).round() as u8, "muted": volume.muted })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> { items.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn parses_every_command_and_places_app_flag_anywhere() {
        assert_eq!(parse("now", &[]), Ok(Command::Now { app: None }));
        assert_eq!(parse("sessions", &[]), Ok(Command::Sessions));
        assert_eq!(parse("play", &[]), Ok(Command::Play { app: None }));
        assert_eq!(parse("pause", &[]), Ok(Command::Pause { app: None }));
        assert_eq!(parse("toggle", &[]), Ok(Command::Toggle { app: None }));
        assert_eq!(parse("next", &[]), Ok(Command::Next { app: None }));
        assert_eq!(parse("prev", &[]), Ok(Command::Prev { app: None }));
        assert_eq!(parse("skip", &args(&["-15"])), Ok(Command::Skip { by: -15.0, app: None }));
        assert_eq!(parse("seek", &args(&["5"])), Ok(Command::Seek { to: 5.0, app: None }));
        assert_eq!(parse("volume", &args(&["75"])), Ok(Command::Volume { level: 75, app: None }));
        assert_eq!(parse("mute", &args(&["on"])), Ok(Command::Mute { muted: true, app: None }));
        assert_eq!(parse("mute", &args(&["off"])), Ok(Command::Mute { muted: false, app: None }));
        assert_eq!(parse("skip", &args(&["--app", "Spot", "15"])), Ok(Command::Skip { by: 15.0, app: Some("Spot".into()) }));
        assert_eq!(parse("play", &args(&["--app", "Spot"])), Ok(Command::Play { app: Some("Spot".into()) }));
        assert_eq!(parse("seek", &args(&["6", "--app", "Spot"])), Ok(Command::Seek { to: 6.0, app: Some("Spot".into()) }));
        assert_eq!(parse("now", &args(&["--app", "Spot"])), Ok(Command::Now { app: Some("Spot".into()) }));
        assert_eq!(parse("pause", &args(&["--app", "Spot"])), Ok(Command::Pause { app: Some("Spot".into()) }));
        assert_eq!(parse("toggle", &args(&["--app", "Spot"])), Ok(Command::Toggle { app: Some("Spot".into()) }));
        assert_eq!(parse("next", &args(&["--app", "Spot"])), Ok(Command::Next { app: Some("Spot".into()) }));
        assert_eq!(parse("prev", &args(&["--app", "Spot"])), Ok(Command::Prev { app: Some("Spot".into()) }));
        assert_eq!(parse("volume", &args(&["50", "--app", "Spot"])), Ok(Command::Volume { level: 50, app: Some("Spot".into()) }));
        assert_eq!(parse("mute", &args(&["on", "--app", "Spot"])), Ok(Command::Mute { muted: true, app: Some("Spot".into()) }));
    }

    #[test]
    fn commands_map_to_playback_actions() {
        assert_eq!(Command::Now { app: None }.action(), None);
        assert_eq!(Command::Sessions.action(), None);
        assert_eq!(Command::Play { app: None }.action(), Some(media::Action::Play));
        assert_eq!(Command::Pause { app: None }.action(), Some(media::Action::Pause));
        assert_eq!(Command::Toggle { app: None }.action(), Some(media::Action::Toggle));
        assert_eq!(Command::Next { app: None }.action(), Some(media::Action::Next));
        assert_eq!(Command::Prev { app: None }.action(), Some(media::Action::Prev));
        assert_eq!(Command::Skip { by: -4.0, app: None }.action(), Some(media::Action::Skip(-4.0)));
        assert_eq!(Command::Seek { to: 5.0, app: None }.action(), Some(media::Action::Seek(5.0)));
        assert_eq!(Command::Volume { level: 50, app: None }.action(), None);
        assert_eq!(Command::Mute { muted: true, app: None }.action(), None);
    }

    #[test]
    fn rejects_bad_arguments_and_numbers() {
        for (cmd, values) in [("play", vec!["x"]), ("skip", vec!["0"]), ("skip", vec!["NaN"]),
            ("seek", vec!["-1"]), ("seek", vec!["inf"]), ("volume", vec!["101"]),
            ("volume", vec!["1.5"]), ("mute", vec!["yes"]), ("next", vec!["--app"]) ] {
            assert!(parse(cmd, &args(&values)).is_err(), "accepted {cmd} {values:?}");
        }
        assert_eq!(parse("pause", &args(&["--app"])).unwrap_err(), "--app needs a name");
        assert_eq!(parse("toggle", &args(&["--app", "a", "--app", "b"])).unwrap_err(), "--app given twice");
        assert_eq!(parse("sessions", &args(&["--app", "x"])).unwrap_err(), "sessions takes no arguments");
        assert_eq!(parse("missing", &[]).unwrap_err(), "unknown command");
        assert_eq!(parse("skip", &args(&["0"])).unwrap_err(), "skip takes a non-zero number of seconds");
        assert_eq!(parse("seek", &args(&["-1"])).unwrap_err(), "seek takes a number of seconds >= 0");
        assert_eq!(parse("volume", &args(&["101"])).unwrap_err(), "volume takes a whole number 0-100");
        assert_eq!(parse("mute", &args(&["yes"])).unwrap_err(), "mute takes on or off");
        assert_eq!(parse("next", &args(&["extra"])).unwrap_err(), "next takes no arguments");
    }

    #[test]
    fn resolves_id_prefix_ambiguity_and_default_session() {
        let sessions = vec![Session { id: "a-spotify-id".into(), ..Session::default() },
            Session { id: "b-spotify-id".into(), ..Session::default() },
            Session { id: "c-firefox-id".into(), ..Session::default() }];
        assert_eq!(resolve(&sessions, None, Some("a-spotify-id")).unwrap().id, "a-spotify-id");
        assert_eq!(resolve(&sessions, None, Some("fire")).unwrap().id, "c-firefox-id");
        assert!(resolve(&sessions, None, Some("spot" )).unwrap_err().contains("matches several sessions: Spotify (a-spotify-id), Spotify (b-spotify-id)"));
        assert!(resolve(&sessions, None, Some("nope")).unwrap_err().contains("no media session matches \"nope\""));
        assert_eq!(resolve(&sessions, Some("b-spotify-id"), None).unwrap().id, "b-spotify-id");
        assert!(resolve(&sessions, None, None).unwrap_err().contains("no session is shown; pick one with --app"));
        assert_eq!(resolve(&[], None, None), Err("no media sessions".into()));
    }

    #[test]
    fn unknown_or_untrusted_position_and_length_are_null() {
        assert_eq!(position_fields(Progress::Unknown, true), (None, None));
        assert_eq!(position_fields(Progress::Live, true), (None, None));
        assert_eq!(position_fields(Progress::At { pos: 4.0, len: 12.0 }, false), (None, None));
        assert_eq!(position_fields(Progress::At { pos: 4.0, len: 12.0 }, true), (Some(4.0), Some(12.0)));
    }
}
