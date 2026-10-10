//! The right-click panel and the settings-file watcher.

use super::*;
use taskbar_dock::menu_panel::{self, glyph, Item, Menu, Row};

/// The right-click panel (taskbar_dock::menu_panel), from the label, the tray icon or the
/// flyout (a right click there switches to this panel).
pub(crate) fn show_menu() {
    let Some((main, label, label_on, fahrenheit, compact)) =
        with(|a| (a.main, a.label, a.cfg.taskbar_label, a.cfg.fahrenheit(), a.cfg.compact))
    else { return };
    // a right-click isn't a hover: cancels a pending hover-open, closes a hover-opened flyout
    if let Some(a) = with(|a| a.panel.right_click(false)) {
        panel_do(a);
    }
    if with(|a| a.popup.is_some()) == Some(true) {
        popup_closed(false);
    }
    // above the label; from the tray icon (label off), above the pointer
    let mut rc = RECT::default();
    let scale;
    unsafe {
        if let (Some(l), Some(tb)) = (label, taskbar()) {
            let _ = GetWindowRect(l, &mut rc);
            scale = scale_of(tb);
            if !menu_panel::is_open() {
                dock::close_other_panels(raw(tb), raw(l)); // one panel at a time across the widgets
            }
        } else {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            rc = RECT { left: pt.x, top: pt.y, right: pt.x + 1, bottom: pt.y + 1 };
            scale = GetDpiForWindow(main).max(96) as f32 / 96.0;
        }
    }
    let (title, subtitle) = with(|a| {
        let c = full_content(a);
        let title = a.weather.as_ref().map_or("Weather".to_string(), |w| w.location.clone());
        (title, [c.line1, c.line2, c.desc].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · "))
    })
    .unwrap_or_default();
    let menu = Menu {
        title,
        subtitle,
        items: vec![
            Row::action(ID_REFRESH, glyph::REFRESH, "Refresh now"),
            Item::Sep,
            Row::switch(ID_LABEL, "Show on taskbar", label_on),
            Row::switch(ID_FAHRENHEIT, "Fahrenheit (°F)", fahrenheit),
            Row::switch(ID_COMPACT, "Compact mode", compact),
            Row::switch(ID_STARTUP, "Start with Windows", startup_enabled()),
            Item::Sep,
            menu_panel::settings_and_quit(ID_SETTINGS, ID_EXIT),
        ],
    };
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
            ID_LABEL => {
                let on = save(&mut |c| {
                    c.taskbar_label = !c.taskbar_label;
                    c.taskbar_label
                });
                if on { create_label() } else { destroy_label() }
                update_tray(false);
                on
            }
            ID_FAHRENHEIT => {
                let on = save(&mut |c| {
                    c.units = if c.fahrenheit() { "celsius" } else { "fahrenheit" }.into();
                    c.fahrenheit()
                });
                start_fetch();
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
            _ => {
                set_startup(!startup_enabled());
                startup_enabled()
            }
        }
    };
    let anchor = menu_panel_rect(rc);
    let cmd = unsafe { menu_panel::run(anchor, menu, &look(), scale, &mut flip) };
    // the mouse is likely still on the label: no hover-open until it leaves and comes back
    with(|a| a.panel.menu_closed());
    match cmd {
        Some(ID_REFRESH) => {
            let cfg = Config::load();
            with(|a| {
                a.cfg = cfg;
                a.cfg_mtime = config_mtime();
                a.loc = None; // re-detect location too
            });
            start_fetch();
        }
        Some(ID_SETTINGS) => {
            let path = wide(&Config::path().display().to_string());
            unsafe { ShellExecuteW(None, w!("open"), w!("notepad.exe"), pcwstr(&path), None, SW_SHOWNORMAL); }
        }
        Some(ID_EXIT) => unsafe { let _ = DestroyWindow(main); },
        _ => {}
    }
}

/// A `windows` crate RECT as the library's (windows-sys) RECT.
fn menu_panel_rect(r: RECT) -> taskbar_dock::panel::RECT {
    taskbar_dock::panel::RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

/// Picks up edits to the shared settings file (e.g. via "Edit settings…"). The bus widget
/// writes to the same file, so act only on what changed: the weather data settings → fetch
/// again; the label switch → recreate it; the gap or position → just move.
pub(crate) fn watch_config() {
    let mtime = config_mtime();
    if with(|a| a.cfg_mtime != mtime) != Some(true) {
        return;
    }
    let cfg = Config::load();
    // load() may have tidied the file; take the time after that so it isn't seen as a new edit
    let Some(old) = with(|a| {
        let old = a.cfg.clone();
        a.cfg = cfg.clone();
        a.cfg_mtime = config_mtime();
        old
    }) else { return };
    let data = |c: &Config| {
        (c.city.clone(), c.latitude, c.longitude, c.location_name.clone(), c.units.clone(),
         c.refresh_minutes, c.model.clone(), c.knmi_api_key.clone(), c.knmi_station.clone())
    };
    if old.taskbar_label != cfg.taskbar_label {
        destroy_label();
        if cfg.taskbar_label {
            create_label();
        }
        update_tray(false);
    } else if old.label_position != cfg.label_position || old.compact != cfg.compact {
        redraw_label();
    }
    if data(&old) != data(&cfg) {
        update_tray(false);
        start_fetch();
    }
}
