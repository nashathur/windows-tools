//! The right-click panel and the settings-file watcher.

use super::*;
use taskbar_dock::menu_panel::{self, glyph, Item, Menu, Row};

/// The right-click panel (taskbar_dock::menu_panel), from the strip or the panel (a right
/// click there switches to this one).
pub(crate) fn show_menu() {
    let Some((main, Some(label), voice, cfg)) = with(|a| (a.main, a.label, a.voice.clone(), a.cfg.clone())) else { return };
    if let Some(a) = with(|a| a.panel.right_click(false)) {
        panel_do(a);
    }
    if with(|a| a.popup.is_some()) == Some(true) {
        popup_closed(false);
    }
    let mut rc = RECT::default();
    let scale;
    unsafe {
        let _ = GetWindowRect(label, &mut rc);
        let tb = GetParent(label).ok().or_else(taskbar);
        scale = tb.map_or(1.0, scale_of);
        if let (Some(tb), false) = (tb, menu_panel::is_open()) {
            dock::close_other_panels(raw(tb), raw(label));
        }
    }
    let (l1, l2) = render::strip_lines(&voice);
    let mut items = Vec::new();
    if voice.in_call() {
        items.push(Row::switch(ID_MUTE, "Mute microphone", voice.self_mute));
        items.push(Row::switch(ID_DEAF, "Deafen", voice.self_deaf));
        items.push(Row::action(ID_LEAVE, '\u{E711}', "Leave call"));
        items.push(Item::Sep);
    }
    items.push(Row::switch(ID_COMPACT, "Compact mode", cfg.compact));
    items.push(Row::switch(ID_IDLE, "Show when not in a call", cfg.show_when_idle));
    items.push(Row::switch(ID_STARTUP, "Start with Windows", startup_enabled()));
    items.push(Item::Sep);
    if cfg.configured() {
        items.push(Row::action(ID_SIGNIN, '\u{E77B}', "Sign in to Discord"));
        items.push(Row::action(ID_RECONNECT, glyph::REFRESH, "Reconnect"));
        items.push(Item::Sep);
    }
    items.push(menu_panel::settings_and_quit(ID_SETTINGS, ID_EXIT));
    let menu = Menu { title: l1, subtitle: l2, items };

    // the switches flip and the panel stays open
    let mut flip = |id: usize| -> bool {
        let save = |f: &mut dyn FnMut(&mut Config) -> bool| {
            with(|a| {
                let on = f(&mut a.cfg);
                a.cfg.save();
                a.cfg_mtime = config_mtime();
                on
            })
            .unwrap_or(false)
        };
        match id {
            ID_MUTE => {
                let on = with(|a| !a.voice.self_mute).unwrap_or(false);
                rpc(rpc::Cmd::Mute(on));
                on
            }
            ID_DEAF => {
                let on = with(|a| !a.voice.self_deaf).unwrap_or(false);
                rpc(rpc::Cmd::Deaf(on));
                on
            }
            ID_COMPACT => {
                let on = save(&mut |c| {
                    c.compact = !c.compact;
                    c.compact
                });
                redraw_label();
                on
            }
            ID_IDLE => save(&mut |c| {
                c.show_when_idle = !c.show_when_idle;
                c.show_when_idle
            }),
            _ => {
                set_startup(!startup_enabled());
                startup_enabled()
            }
        }
    };
    let anchor = taskbar_dock::panel::RECT { left: rc.left, top: rc.top, right: rc.right, bottom: rc.bottom };
    let cmd = unsafe { menu_panel::run(anchor, menu, &look(), scale, &mut flip) };
    with(|a| a.panel.menu_closed());
    match cmd {
        Some(ID_LEAVE) => rpc(rpc::Cmd::Leave),
        Some(ID_SIGNIN) => rpc(rpc::Cmd::SignIn),
        Some(ID_RECONNECT) => {
            let key = with(|a| a.cfg.connection_key()).unwrap_or_default();
            rpc(rpc::Cmd::Reconnect(key));
        }
        Some(ID_SETTINGS) => {
            let path = wide(&Config::path().display().to_string());
            unsafe { ShellExecuteW(None, w!("open"), w!("notepad.exe"), pcwstr(&path), None, SW_SHOWNORMAL); }
        }
        Some(ID_EXIT) => unsafe { let _ = DestroyWindow(main); },
        _ => {}
    }
    // show_when_idle may have changed
    update_visibility();
}

/// Picks up edits to the shared settings file (e.g. via "Edit settings…"): new app credentials
/// reconnect; the look settings redraw.
pub(crate) fn watch_config() {
    let mtime = config_mtime();
    if with(|a| a.cfg_mtime != mtime) != Some(true) {
        return;
    }
    let cfg = Config::load();
    let Some(old) = with(|a| {
        let old = a.cfg.clone();
        a.cfg = cfg.clone();
        a.cfg_mtime = config_mtime();
        old
    }) else { return };
    if old.connection_key() != cfg.connection_key() {
        log("app settings changed: reconnecting");
        rpc(rpc::Cmd::Reconnect(cfg.connection_key()));
    }
    if old != cfg {
        update_visibility();
        redraw_label();
    }
}
