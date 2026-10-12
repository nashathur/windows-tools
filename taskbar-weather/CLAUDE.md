# taskbar-weather: weather label in the Windows 11 taskbar

@../taskbar-dock/CLAUDE.md

Rust, `windows` crate. README.md has features and settings. Runs straight from
`target\release\taskbar-weather.exe` (dev.ps1 stops it before building). Settings section
`weather` in the shared settings.json; start with Windows via the shared `startup` list.

## Files (src/)
Everything about the strip window, dock, panels, tray icon and the standard menu rows is Block's
(`..\taskbar-dock\BLOCK.md`); weather supplies the `Content`.
- main.rs: `App` (the `block::Content`: label content, side, visible, hooks, the flyout's
  `panel_size` / `panel_draw`; Block owns its window), the `Spec`, `main`.
- fetch.rs: forecast + KNMI observation fetch on a worker thread, `on_data` (refresh timer;
  `redraw_panel` for an open flyout).
- menu.rs: the right-click panel's own rows (Refresh now, Fahrenheit; Block adds Show on taskbar,
  Compact mode, Start with Windows), switches, and `settings_changed`.
- tray.rs: the tray icon's tooltip (`Content::tray_tip`).
- weather.rs (forecast API), knmi.rs (observations), render.rs (label `measure_label`/`draw_label`,
  flyout `paint_panel`, `tray_logo`: the cloud glyph for `Content::tray_icon`), config.rs (fills
  missing defaults on start; user changes write only their changed key or location keys).
