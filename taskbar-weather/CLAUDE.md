# taskbar-weather: weather label in the Windows 11 taskbar

@../taskbar-dock/CLAUDE.md

Rust, `windows` crate. README.md has features and settings. Runs straight from
`target\release\taskbar-weather.exe` (dev.ps1 stops it before building). Settings section
`weather` in the shared settings.json; start with Windows via the shared `startup` list.

## Files (src/)
The strip window, dock, drag, panels' open/close, right-click panel and settings watch are the
library's `block` (plan step 3); weather supplies the `Content`.
- main.rs: `App` (the `block::Content`: label content, side, visible, hooks), the `Spec`, `main`.
- fetch.rs: forecast + KNMI observation fetch on a worker thread, `on_data` (refresh timer).
- flyout.rs: the flyout window for Block's interim `Spec::panel` hook (create, place, paint, close).
- menu.rs: the right-click panel's own rows (Refresh now, Show on taskbar, Fahrenheit), switches,
  and `settings_changed`.
- tray.rs: tray icon (only while the label is off; clicks → `Ctx::toggle_panel` / `open_menu`).
- weather.rs (forecast API), knmi.rs (observations), render.rs (label `measure_label`/`draw_label`,
  flyout, tray icon drawing), config.rs (`save` merges: Block owns `compact`).
