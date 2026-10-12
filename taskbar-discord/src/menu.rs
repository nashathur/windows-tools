//! The right-click panel's own rows (Block adds Show on taskbar, Compact mode, Start with
//! Windows, Edit settings… and Quit) and following the settings file.

use super::*;
use taskbar_dock::menu_panel::{glyph, Row};

const ID_MUTE: usize = 100;
const ID_DEAF: usize = 101;
const ID_LEAVE: usize = 102;
const ID_SIGNIN: usize = 106;
const ID_RECONNECT: usize = 107;

/// The call's switches and Leave (in a call), then Reconnect (once set up), and Sign in only
/// while signed out (the panel then says to use it).
pub(crate) fn build(a: &App, m: &mut MenuBuilder) {
    let (l1, l2) = render::strip_lines(&a.voice);
    m.title(l1).subtitle(l2);
    if a.voice.in_call() {
        m.item(Row::switch(ID_MUTE, "Mute microphone", a.voice.self_mute));
        m.item(Row::switch(ID_DEAF, "Deafen", a.voice.self_deaf));
        m.item(Row::action(ID_LEAVE, '\u{E711}', "Leave call"));
        m.sep();
    }
    if a.cfg.configured() {
        if matches!(a.voice.status, Status::NeedsSignIn(_)) {
            m.item(Row::action(ID_SIGNIN, '\u{E77B}', "Sign in to Discord"));
        }
        m.item(Row::action(ID_RECONNECT, glyph::REFRESH, "Reconnect"));
        m.sep();
    }
}

/// Mute / Deafen flipped: Discord does it and reports back (the strip follows its event).
pub(crate) fn switch(a: &mut App, id: usize) -> bool {
    match id {
        ID_MUTE => {
            let on = !a.voice.self_mute;
            a.send(rpc::Cmd::Mute(on));
            on
        }
        ID_DEAF => {
            let on = !a.voice.self_deaf;
            a.send(rpc::Cmd::Deaf(on));
            on
        }
        _ => false,
    }
}

pub(crate) fn action(a: &mut App, id: usize) {
    match id {
        ID_LEAVE => a.send(rpc::Cmd::Leave),
        ID_SIGNIN => a.send(rpc::Cmd::SignIn),
        ID_RECONNECT => a.send(rpc::Cmd::Reconnect(a.cfg.connection_key())),
        _ => {}
    }
}

/// The settings file changed (Edit settings…, another widget): new app credentials reconnect;
/// media's text column width or the avatar count redraw.
pub(crate) fn settings_changed(a: &mut App, ctx: &mut Ctx) {
    let cfg = Config::read();
    if a.cfg.connection_key() != cfg.connection_key() {
        log("app settings changed: reconnecting");
        a.send(rpc::Cmd::Reconnect(cfg.connection_key()));
    }
    if a.cfg.max_avatars != cfg.max_avatars {
        a.slots.update(a.voice.channel.as_ref().map_or(&[], |c| &c.members), cfg.max_avatars);
    }
    a.cfg = cfg;
    a.media_w = media_text_width();
    ctx.redraw();
    ctx.redraw_panel();
}
