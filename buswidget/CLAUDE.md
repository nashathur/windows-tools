# buswidget: next bus departures in the Windows 11 taskbar

@../taskbar-dock/CLAUDE.md

Rust (`windows-sys`), crate in `rust\`. Runs from a copy: `buswidget.exe` in this folder
(dev.ps1 copies it). Data folder `data\` next to the exe: `buswidget.log`, `schedule.tsv`
(timetable built from the GTFS zip; ovapi.nl may answer 429, it retries after 180 min),
`line_colors.txt`. Settings section `bus`; start with Windows via the shared `startup` list
(switch in the right-click panel). `compact: true`: badge + next two departures on two lines.
The strip window, dock, panels, tray icon and the standard menu rows are Block's
(`..\taskbar-dock\BLOCK.md`). Bus-specific: the floating fallback position (`data\pos.txt`), the
right-click rows from `Content::menu` (Change stop…, Refresh live times, Update timetable), the
tray icon's line badge (shown when Show on taskbar is off). `compact` and `show_on_taskbar` in the
`bus` section are Block's; `Config::save/load` merge their keys.

## Files (rust/src/)
- main.rs: `App` (the `block::Content`: measure/draw, 1 s content timer, worker messages
  → `refresh`, panel hooks, menu rows, `settings_changed`, tray icon/tip), the `Spec`,
  `badge_colors`, `main`. The stop picker state also lives in `App`.
- worker.rs: `Shared` state, `current()`, `set_config`/`apply_config`, the worker thread (timetable
  builds, live polling from OVapi every 60 s when a bus is due within 90 min); posts WM_DATA to Block's main window.
- bar.rs: strip text (`bar_text_for`), `measure`/`draw` into Block's canvas, `tray_logo`.
- popup.rs: departures panel content (`popup_content`), `panel_size`, `panel_draw` (Block owns the window).
- data.rs: config (merges into its section), GTFS parsing, schedule, live data, logging.
- picker.rs: stop picker content drawn inside Block's focused `Picker` panel; worker results are
  posted to Block's main window and tagged with a generation so stale searches are discarded.

## Testing
Use `taskbar-dock\dockctl.exe bus state|panel …|menu …|simulate …` (README there), including
`panel open picker`, `panel key`, `panel type` and `panel state`, not `_tmp\real.ps1`
or mouse clicks. The dock leader is the strip nearest the tray; find it by the `IconsRight` prop in
`_tmp\strips.ps1 -Props` (weather on 2026-10-11, not bus), and `_tmp\fit_test.ps1 -Widget <its
folder>` must name it for the edge tests (with another widget named, the fake edge has no effect).
