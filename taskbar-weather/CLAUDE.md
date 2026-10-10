# taskbar-weather: weather label in the Windows 11 taskbar

@../taskbar-dock/CLAUDE.md

Rust, `windows` crate. README.md has features and settings. Runs straight from
`target\release\taskbar-weather.exe` (dev.ps1 stops it before building). Settings section
`weather` in the shared settings.json; start with Windows via the shared `startup` list.

## Files (src/)
- main.rs: `App` state, constants, start-with-Windows, window procedures (main, label, popup), `main`.
- fetch.rs: forecast + KNMI observation fetch on a worker thread, `on_data`.
- label.rs: the taskbar label: content, drawing, dock position, WinEvent hooks, and the wiring of
  the shared `taskbar_dock::strip::Motion` (glide, drag, zoom; taskbar client x).
- flyout.rs: the flyout panel (open, pin, hover-open, fade, close).
- menu.rs: right-click panel (shared `menu_panel`) and settings-file watcher.
- tray.rs: tray icon (only while the label is off).
- weather.rs (forecast API), knmi.rs (observations), render.rs (drawing), config.rs.
