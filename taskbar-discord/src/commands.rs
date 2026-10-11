//! dockctl commands for reading and controlling the Discord voice call.

use crate::rpc::{self, Op};
use crate::{App, voice};
use serde_json::{json, Value};
use taskbar_dock::block::{CommandError, CommandReply, ReplyToken};

/// The Discord commands exposed through dockctl.
pub(crate) fn commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("now", "now: status, channel and members (speaking, muted)"),
        ("mute", "mute on|off: set your microphone mute"),
        ("deafen", "deafen on|off: set deafen"),
        ("leave", "leave: leave the voice call"),
    ]
}

/// Parses one command and its exact argument shape.
fn parse(name: &str, args: &[String]) -> Result<Option<Op>, CommandError> {
    let bad_args = |message: &str| CommandError { usage: true, message: message.into() };
    match name {
        "now" if args.is_empty() => Ok(None),
        "mute" | "deafen" if args.len() == 1 => {
            let on = match args[0].as_str() {
                "on" => true,
                "off" => false,
                _ if name == "mute" => return Err(bad_args("mute takes on or off")),
                _ => return Err(bad_args("deafen takes on or off")),
            };
            if name == "mute" { Ok(Some(Op::Mute(on))) } else { Ok(Some(Op::Deaf(on))) }
        }
        "leave" if args.is_empty() => Ok(Some(Op::Leave)),
        "now" => Err(bad_args("now takes no arguments")),
        "mute" => Err(bad_args("mute takes on or off")),
        "deafen" => Err(bad_args("deafen takes on or off")),
        "leave" => Err(bad_args("leave takes no arguments")),
        _ => Err(bad_args("unknown command")),
    }
}

/// Produces the current voice state in dockctl's stable JSON shape.
fn now(voice: &voice::Voice) -> Value {
    let channel = voice.channel.as_ref().map(|channel| json!({
        "id": channel.id,
        "name": channel.name,
        "guild": channel.guild,
    }));
    let members: Vec<Value> = voice.channel.as_ref().map_or_else(Vec::new, |channel| {
        channel.members.iter().map(|member| json!({
            "id": member.id,
            "name": member.name,
            "me": voice.me.as_deref() == Some(member.id.as_str()),
            "speaking": member.speaking,
            "muted": member.muted,
            "deafened": member.deafened,
        })).collect()
    });
    json!({
        "ok": true,
        "status": voice.status.text(),
        "ready": voice.status == voice::Status::Ready,
        "self_mute": voice.self_mute,
        "self_deaf": voice.self_deaf,
        "channel": channel,
        "members": members,
    })
}

/// Runs a Discord command on the UI thread; state reads and command submission never block.
pub(crate) fn command(app: &mut App, name: &str, args: &[String], token: ReplyToken) -> CommandReply {
    let op = match parse(name, args) {
        Ok(op) => op,
        Err(error) => return CommandReply::Done(Err(error)),
    };
    let Some(op) = op else {
        return CommandReply::Done(Ok(now(&app.voice)));
    };
    if !app.try_send(rpc::Cmd::Request(op, token)) {
        return CommandReply::Done(Err(CommandError { usage: false, message: "the Discord connection isn't running".into() }));
    }
    CommandReply::Pending
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn parser_accepts_only_the_documented_arguments() {
        assert_eq!(parse("now", &args(&[])).ok().unwrap(), None);
        assert_eq!(parse("mute", &args(&["on"])).ok().unwrap(), Some(Op::Mute(true)));
        assert_eq!(parse("mute", &args(&["off"])).ok().unwrap(), Some(Op::Mute(false)));
        assert_eq!(parse("deafen", &args(&["on"])).ok().unwrap(), Some(Op::Deaf(true)));
        assert_eq!(parse("leave", &args(&[])).ok().unwrap(), Some(Op::Leave));
        assert!(parse("now", &args(&["extra"])).is_err());
        assert!(parse("mute", &args(&[])).is_err());
        assert!(parse("mute", &args(&["yes"])).is_err());
        assert!(parse("mute", &args(&["on", "off"])).is_err());
        assert!(parse("deafen", &args(&["off", "extra"])).is_err());
        assert!(parse("leave", &args(&["now"])).is_err());
        assert!(parse("unknown", &args(&[])).is_err());
    }
}
