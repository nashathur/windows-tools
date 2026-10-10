# buswidget: next bus departures in the Windows 11 taskbar

@../taskbar-dock/CLAUDE.md

Rust (`windows-sys`), crate in `rust\`. Runs from a copy: `buswidget.exe` in this folder
(dev.ps1 copies it). Data folder `data\` next to the exe: `buswidget.log`, `schedule.tsv`
(timetable built from the GTFS zip; ovapi.nl may answer 429, it retries after 180 min),
`line_colors.txt`. Settings section `bus`; start with Windows via the shared `startup` list
(switch in the right-click panel). `compact: true`: badge + next two departures on two lines. Right click
(strip or departures popup) opens the shared `taskbar_dock::menu_panel` (main.rs `context_menu`).

## Files (rust/src/)
- main.rs: constants, colours, UI state (`Ui`, `ui()`), context menu, `on_tick`, the strip's
  window procedure, fonts, `run_ui`, `main`.
- worker.rs: `Shared` state, `current()`, settings watcher, the worker thread (timetable
  builds, live polling from OVapi every 60 s when a bus is due within 90 min).
- bar.rs: strip text, drawing helpers, look, dock geometry.
- placement.rs: dock position, glide, drag, zoom (rules in taskbar_dock::strip, this file wires
  them up and moves the window), WinEvent hooks, creating the strip.
- popup.rs: departures popup (content, drawing, open/pin/hover/fade, window procedure).
- data.rs: config, GTFS parsing, schedule, live data, logging. picker.rs: stop picker.
