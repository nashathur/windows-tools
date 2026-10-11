# buswidget: next bus departures in the Windows 11 taskbar

@../taskbar-dock/CLAUDE.md

Rust (`windows-sys`), crate in `rust\`. Runs from a copy: `buswidget.exe` in this folder
(dev.ps1 copies it). Data folder `data\` next to the exe: `buswidget.log`, `schedule.tsv`
(timetable built from the GTFS zip; ovapi.nl may answer 429, it retries after 180 min),
`line_colors.txt`. Settings section `bus`; start with Windows via the shared `startup` list
(switch in the right-click panel). `compact: true`: badge + next two departures on two lines.
On Block (`taskbar-dock\src\block`, since plan step 6): the strip window, dock, drag, floating
fallback (`data\pos.txt`), panels' open/close, right-click panel (Block's standard rows + the
widget's rows from `Content::menu`: Change stop…, Refresh live times, Update timetable), tray icon
(line badge, when Show on taskbar is off) and settings watch are the library's. `compact` and
`show_on_taskbar` in the `bus` section are Block's; `Config::save/load` merge their keys.

## Files (rust/src/)
- main.rs: `App` (the `block::Content`: measure/draw, 1 s content timer, WM_DATA from the worker
  → `refresh`, panel hooks, menu rows, `settings_changed`, tray icon/tip), the `Spec`, `badge_colors`, `main`.
- worker.rs: `Shared` state, `current()`, `set_config`/`apply_config`, the worker thread (timetable
  builds, live polling from OVapi every 60 s when a bus is due within 90 min); posts WM_DATA to Block's main window.
- bar.rs: strip text (`bar_text_for`), `measure`/`draw` into Block's canvas, `tray_logo`.
- popup.rs: departures panel content (`popup_content`), `panel_size`, `panel_draw` (Block owns the window).
- data.rs: config (merges into its section), GTFS parsing, schedule, live data, logging.
- picker.rs: stop picker, a plain Win32 window; keys via `Spec::dialog`.

## Testing
Use `taskbar-dock\dockctl.exe bus state|panel …|menu …|simulate …` (README there), not `_tmp\real.ps1`
or mouse clicks. The dock leader is whichever strip is first in the real order.txt (now weather), so
`_tmp\fit_test.ps1` needs `-Widget taskbar-weather` for the edge tests.
