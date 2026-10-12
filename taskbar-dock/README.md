# taskbar-dock

A shared Rust library for the taskbar widgets: the bus widget (`C:\Users\thurs\tools\buswidget`), taskbar-weather (`C:\Users\thurs\tools\taskbar-weather`), taskbar-media (`C:\Users\thurs\tools\taskbar-media`), taskbar-discord (`C:\Users\thurs\tools\taskbar-discord`), and any new one. A widget built on it docks into the Windows 11 taskbar next to the others. It looks and behaves the same way and follows Windows settings live.

## What's in it

A widget is a `Spec` + a `Content` given to `block::run` (see "Building a new widget" and BLOCK.md).

| Module | What it gives a widget |
|---|---|
| `block` | The strip window and all its behaviour (dock slot, drag, panels, right-click panel, tray icon, fit, Explorer restarts, floating fallback). A widget gives a `Spec` and a `Content` and calls `block::run`. BLOCK.md. |
| `settings` | One settings file for all widgets: `%APPDATA%\TaskbarDock\settings.json`. Reads are pure; writes patch only the changed keys and are atomic. |
| `describe` | Typed setting descriptions, validation, JSON conversion and the executable description cache. |
| `registry` | Registered widget names, executable paths and window classes in the `widgets` setting. |
| `client`, `control` | Shared v2 control requests, batched status pings and process control, plus validated settings writes used by widgets and dockctl. |
| `style` | The shared look and behaviour from `dock` (gap, font, radii, timings, hover delays, "follow Windows" switches), resolved into a `Look` for right now. |
| `system` | The Windows settings that are followed live: theme, Animation effects, mouse hover time, Text size, time format, and display scale per monitor. Also time formatting. |
| `draw` | Shared drawing for the strip and panels: text colour, icons, rounded shapes, text boxes, scrolling-list rows, buttons and highlights; anti-aliased through common helpers. |
| `text_edit` | A single-line Unicode text model with selection, editing keys and the Win32 clipboard. |
| `badge` | A coloured, rounded badge with short text (e.g. a bus line number): tray-icon tall, as wide as its text, text centred, shared corners and font, automatic black/white text. |
| `anim` | Easing and animation progress (0 ms when animations are off). `pulse`: a slow, smooth 0 → 1 → 0 every 2.4 s for "alive, nothing else to show"; `pulse_settle`: gliding down to 0 from wherever a pulse stopped. |
| `marquee` | Scrolling text for a line too long for its fixed-width box: still if it fits; otherwise wait 1.5 s, then scroll left at 30 px/s in a loop (the start follows 40 px after the end) until it's back where it began, and repeat. `fade_edges` softens the cut-off edges. Still (cut with "…") when animations are off. |
| `theme` | The design code: every colour, font, font size and spacing, light and dark; the `brand` allow-list for brand and data colours. Widgets take all of them from here (`tests\theme_lint.rs` checks it). |
| `emoji` | Colour emoji through Direct2D + DirectWrite (GDI draws emoji in one colour). |
| `window` | `raise`: bring another app's window to the front; `exe_path`, `is_exe`. |
| `dock`, `icons`, `panel`, `menu_panel`, `strip`, `startup`, `crash`, `watchdog`, `ctl` | Block's internals; stacking and fit, panel behaviour, strip motion, startup list, crash handling and dockctl command parsing. `watchdog` stores launcher give-up records. |

The settings app is its own crate in `settings-app\`, with a path dependency on this library. Its
WinUI window, page and row model, `dockctl settings` protocol, and worker threads live there.

## Settings app

`taskbar-dock-settings.exe` is the control panel for registered widgets and shared Dock settings.
It has a Widgets overview, a page for each widget, a Dock page, and a Developer page. Developer
contains advanced settings and details for each registered widget, registry management, masked raw
settings, and copyable console commands. It applies validated edits immediately and follows
settings-file changes while open.

Build and install it from `taskbar-dock\settings-app\`:

```powershell
cargo build --release
Copy-Item target\release\taskbar-dock-settings.exe ..\taskbar-dock-settings.exe
```

Block's Edit settings runs `taskbar-dock\taskbar-dock-settings.exe`. On Windows, the app uses
the Windows-provided `Microsoft.WindowsAppRuntime.CBS.2` when available and copies its
`resources.pri` beside the executable on first start. Otherwise it uses the installed
`Microsoft.WindowsAppRuntime.2` Store package; if neither runtime is available, it shows a message
with Microsoft's download link and exits.

Launch with no arguments for Widgets, `--widget <name>` for a widget page, `--dock` for Dock,
`--developer` for Developer, or `--register <exe>` to register a widget without opening a window.

`dockctl settings` drives the running app and returns JSON:

| Command | Does |
|---|---|
| `state` | Read page, rows, window and app status. Row kinds are `switch`, `dropdown`, `slider`, `numberbox`, `text`, `color`, `secret`, and `auto`. Developer `cards` contains `advanced`, `details`, `registered`, `raw`, and `commands`; `focused` is null. |
| `page <widgets\|dock\|developer\|name>` | Navigate to a page. |
| `row <id> set <text>`, `row <id> reset`, `row <id> toggle` | Edit or reset a setting row. |
| `click <id>` | Activate actions such as `open:<name>`, `restore:<section>:<group>`, `start:<name>`, `quit:<name>`, `restart:<name>`, `remove:<name>`, `copy:<n>`, `refresh-log:<name>`, or `notepad`. |
| `key <name>`, `type <text>` | Bring the app to the front, then send supported keyboard input. |
| `quit` | Close the settings app. |

After installing the app and `dockctl.exe`, run `scripts\settings_smoke.ps1` for the read-only
settings-app smoke check. It starts the installed app through a one-off scheduled task, visits
the pages, compares widget rows with descriptions and `get`, then closes the app.

## The shared `dock` settings

```json
"dock": {
  "gap": 3,
  "font": "Segoe UI Variable Text",
  "font_size": 12,
  "text_size": 1.0,
  "control_radius": 4,
  "time_format": "system",
  "animations": "system",
  "hover_open_ms": "system",
  "hover_close_ms": 300,
  "fade_ms": 150, "glide_ms": 300, "zoom_ms": 120,
  "drag_zoom": 1.15,
  "panel_margin": 12,
  "tint_dark":  { "quiet": 0.18, "active": 0.333, "hover": 0.49 },
  "tint_light": { "quiet": 0.149, "active": 0.275, "hover": 0.412 },
  "tint_fixed": "auto"
}
```

| Key | Meaning |
|---|---|
| `gap` | Pixels (at 100%) between docked widgets and to the tray. |
| `font`, `font_size` | Taskbar text. The defaults are the taskbar clock's font and size. |
| `text_size` | `1.0` matches the clock, which does **not** follow Windows' Text size. `"system"` follows Windows' Text size instead. |
| `control_radius` | Corner radius of the hover highlight and badges. Windows 11 controls use 4 px. Panels get Windows' own 8 px from DWM. |
| `time_format` | `"system"` uses the Windows region format, e.g. `h:mm tt` → "7:45 PM". `"12h"` or `"24h"` fix it. |
| `animations` | `"system"` follows Settings → Accessibility → Visual effects → Animation effects. `true`/`false` fix it. When off, slides, zooms and fades are instant. |
| `hover_open_ms` | How long the mouse rests on a widget before its panel opens. `"system"` uses Windows' mouse hover time (400 ms by default). |
| `hover_close_ms` | A hover-opened panel closes this long after the mouse leaves both the widget and the panel. |
| `fade_ms`, `glide_ms`, `zoom_ms` | Panel fade-in, slide into place, and drag zoom durations. `glide_ms` also times the compact switch; its default 300 and the slides' easing match the Windows 11 taskbar's own button slide (measured: cubic-bezier(0.5, 0, 0, 1) over 300 ms). |
| `drag_zoom` | Content size while dragged (1.15 = 115%). |
| `panel_margin` | Pixels (at 100%) a panel keeps from the taskbar and from the screen edges. Windows 11's own flyouts use 12. |
| `tint_dark`, `tint_light` | Opacity (0–1) of a widget's brand colour behind its strip (discord's blurple, media's app colour), on a dark / light taskbar: `quiet` while paused or muted, `active` while playing or unmuted, `hover` under the mouse. |
| `tint_fixed` | `"auto"`: the levels above, by state. A number 0–1: that opacity always, for every state and widget (0 = no tint, 1 = solid colour). |

Missing keys are filled in with the defaults the first time a widget starts, so the file always shows every option. Edits apply within a second, without restarting the widgets.

## Corner design

Source: arun.is, "The secret formula for Apple's rounded corners". The rules to follow are in
UI-RULES.md; this is the reasoning.

**Closer to the human, the rounder.** The author measured Apple products as a roundness score,
corner diameter ÷ the rectangle's shorter side, and found each category sits in its own band:

| Role (Apple) | Roundness | Widget equivalent | Radius to use |
|---|---|---|---|
| Desktop boxes (Mac mini, Apple TV) | 0% | Full-width strips, dividers | 0 |
| Displays | 2-3% | Large panel (≈300 px tall) | DWM's 8 px (≈3%) |
| MacBook | 7-10% | Medium panel / menu | DWM's 8 px |
| iPad | 12-18% | Cards, cover art in a panel | `card_radius`: side ÷ 8, 4-12 px (96 px cover: 12 px, 25%) |
| iPhone | ~33% | Hover highlight on a 32 px button, badges | `control_radius` 4 px (≈25%) |
| Watch, AirPods case | ~50% | Small chips, key caps | ≥ shorter side × 0.4 |
| Pencil, earbuds | 100% | Dots, switch knobs, progress caps | shorter side ÷ 2 |

Rules of thumb: 4 px for a 16-32 px control is the Windows 11 default and stays; the only
panel radius is the one DWM draws, so a panel and the system flyouts next to it match.

**Concentric corners.** A phone's screen corner is the case corner shrunk by the bezel, so the
bezel keeps one width all the way round. Same for UI: inner radius = outer radius − gap
(floored at 0). A child 4 px inside an 8 px panel gets 4 px, not 8; a child 8 px or more inside
gets 0 from the formula, so use `control_radius` as the floor for anything interactive.

**Continuous curvature ("squircle").** A circular arc meets the straight edge with a jump in
curvature, so reflections/eyes see a seam. Apple's corners ease the curvature in and out
instead (superellipse-like; Figma's "corner smoothing 60%" is the common approximation). Only
the property matters: no visible seam, radius appearing larger
near the straight edges. At 4 px it can't be seen, so small corners stay circular arcs;
`draw::shape_cover` switches to the continuous corner above 8 px (superellipse n = 3.2, reach
1.5 × radius, chosen so the diagonal cut is as deep as the circle's; checked by a unit test,
not against Apple's real curve).

## Building a new widget

**A new widget = a `Spec` + a `Content`.** The widget supplies the parameters and what to draw;
`block::run(spec, content)` does everything else (BLOCK.md has the API). The closest examples:
`taskbar-weather` (smallest), `buswidget\rust` (a picker panel, a worker), `taskbar-media` (buttons, a
cover, a width that follows its content), `taskbar-discord` (a worker, a width that follows).

1. **The crate**: `taskbar-dock = { path = "../taskbar-dock" }`, `build.rs` + `app.manifest`
   copied from `taskbar-weather` (a layered child window of the taskbar needs the Windows 8+
   `supportedOS` manifest), a `dev.ps1` like theirs (it wraps `taskbar-dock\scripts\dev.ps1`), and a
   `CLAUDE.md` that imports `..\taskbar-dock\CLAUDE.md`. `windows-sys` (like this library and bus)
   or `windows` (weather, media: handles are `h.0 as isize`).
2. **`Spec::new(class, section, title, log)`**: `class` is the strip window's class and its name
   in the dock order, `section` the settings section (also the name on the startup list). Change
   only what differs from the defaults: `mutex`, `float`, `force_float`, `draggable`, `hover_panel`,
   `compact_when_crowded`, `stretch` (any width between compact and full, when crowded),
   `glide_width` (a width that follows the content), `log_file` (dockctl's `log`).
3. **A `Content`** (`block::Content`): `measure(frame, compact)` and `draw(canvas, frame)` are
   required; everything else has a default: `hit`, `cover`, `tint`, `visible`, `side`, `start`,
   `timer`, `button`, `cover_click`, the panels (`panel_size`, `panel_draw`, `panel_mouse`,
   `panel_key`, `panel_char`, `panel_wheel`, `panel_shown`, `panel_state`), the right-click panel's own rows (`menu`, `menu_switch`, `menu_action`),
   `settings_changed`, `look_changed`, `taskbar_restarted`, `on_message`, `closing`, and the tray
   icon (`tray_icon`, `tray_tip`). A `Ctx` collects what a callback wants done (`redraw`,
   `redraw_panel`, `every`, `after`, ...).
4. **Drawing**: colours, fonts and sizes come from `theme` (`Theme::new(look)`, `theme::space`,
   `theme::size`, the text styles), never typed in; `tests\theme_lint.rs` (dev.ps1) checks it.
   Times: `look.time(h, m)` / `look.hour(h)`, never a fixed format. Helpers: `draw::icon_box`
   (the tray-icon box: 16 px, so everything lines up with Windows' tray icons), `draw::fit_icon`
   (a glyph drawn large, white on black, then fitted and centred), `draw::round_rect`,
   `draw::progress_ring`, `badge` (a coloured rounded badge with short text), `emoji` (colour
   emoji), `marquee` (a line too long for its box). UI-RULES.md for corners, highlights, hit boxes
   and the cover.
5. **Settings**: reads are pure; fill defaults on start with `settings::fill_missing` and write
   only changed keys with `settings::set_key` or a focused `settings::update`. Block owns the
   section's `compact` and `show_on_taskbar`. Reload on
   `Content::settings_changed` (an edit of the file applies live). Secrets (API keys) stay in the
   section but are never shown in full in logs, menus or panels; mask them. A data file the user
   may edit (bus's `line_colors.txt`): check its mtime from a content timer.
6. **Data and network**: fetch on a worker thread, never the UI thread; the worker posts a
   message to `ctx.main_window()` (`WM_APP + n`), `Content::on_message` takes it and calls
   `ctx.redraw()`. Be careful with APIs: cache responses on disk, poll no more often than the data
   changes, fetch only when it's useful (bus: live times only when a bus is due within 90
   minutes). On HTTP 429 or an error back off: keep the old data, save the next allowed try so a
   restart doesn't retry early, and show that the data is stale rather than an empty widget.
7. **Start with Windows** is one of Block's standard right-click rows (the widget's `section` is
   its name on the `startup` list in settings.json). The launcher `taskbar-dock-start.exe`
   (src/bin, installed as `taskbar-dock\taskbar-dock-start.exe`) is run at sign-in by the
   Startup-folder shortcut "Taskbar widgets" and starts everything on the list (problems go to
   `startup.log`). It then stays as a watchdog: a widget it started that crashes (exit code
   0xC0000xxx) is started again after 2, 4, 8 … s, and given up on after 5 crashes in 10 min; Quit
   or being stopped is left alone (`taskbar_dock::crash`; Block logs the panic reason in the
   widget's log). Give-ups are recorded in `%APPDATA%\TaskbarDock\watchdog.json` for dockctl
   status; a successful launcher start clears that widget's record. Use the launcher, never the
   `HKCU\...\Run` key (its values are skipped at sign-in on this PC).
8. **Block's job, not the widget's** (the same for every widget): the strip
   window and its DPI handling, the dock slot, order, drag and drop, fit next to the taskbar's
   buttons (narrow, compact, hide), instant reflow on neighbours' changes, hover / click / pin
   panels with their fade and placement, the right-click panel's standard rows (Show on taskbar,
   Compact mode, Start with Windows, Edit settings…, Quit), the tray icon, one instance,
   Explorer restarts, the floating fallback and following Windows' theme and settings.
9. **Done** when it has the five parts in CLAUDE.md ("Every widget has these five parts"), it
   behaves like the other widgets (drag, drop, compact, crowding, panels, cover), and
   `dockctl <widget> state|panel|menu` answers.

## Building

- Build, test and restart with the widget's `dev.ps1` (CLAUDE.md "Working on any widget"); the
  widgets build the library as part of their own builds.
- Keep paths short, because MSVC `link.exe` fails on paths over 260 characters.
- Don't `cargo clean`: keeping `target\` makes rebuilds take seconds.
- A running widget locks its exe; dev.ps1 stops it before building or copying.

## Testing

### dockctl: command-line control of widgets on Block

`taskbar-dock\dockctl.exe [--timeout <s>] <widget|all> <command> [args]` talks to a running
widget or uses the shared library for supported settings commands when it is stopped. It prints JSON.
A stopped widget given any other command is reported as `"absent": "not running"`; for `all`
that isn't an error. Commands dockctl runs in its own process (stopped-widget settings,
`describe`, `startup`, `widgets`, `start`, `restart`) are refused when it sees a private copy of
the settings (started from the Claude app, `settings::redirected`): run it through a scheduled
task. `status` still reads process state, but reports that crash records aren't visible there.
Exit code: 0 all succeeded, 1 error or timeout, 2 usage error.
For `all`, 2 takes precedence if any widget reports a usage error. The default deadline is 5
seconds, or 120 seconds for `do`; `--timeout` overrides it.

| Command | Does |
|---|---|
| `state` | pid, exe build time; strip, panel (kind, open, pinned, focused, rect, content), menu, tray, look |
| `panel open\|close\|toggle` | the left-click panel, as a click on the strip does |
| `panel open picker` | open a widget's picker panel (error if it has none) |
| `panel key <name>` | send a normalized panel key: enter, esc, up/down, pgup/pgdn, home/end, ctrl+home/end, back (Alt+Left), backspace, delete, left/right, ctrl+a/c/v/x/backspace |
| `panel type "<text>"` | send panel text through the same character-input path |
| `panel state` | active panel kind, open state, rectangle and widget-owned content state |
| `menu open\|close` | the right-click panel |
| `menu rows` | id, label, switch state of every row (of the open menu, or the one a right click would show) |
| `menu click <id\|label>` | a row, as a click does: a switch flips and the menu stays open; opens the menu first if needed |
| `tray click left\|right` | a click on the tray icon (error while it isn't shown) |
| `set <key> <value>` / `get [key]` | the widget's settings section, written by the widget (`settings::update`) and followed at once; value is JSON or else text; secrets (`*key*`, `*token*`, …) print as `***` |
| `set/get --dock ...` | set or inspect the shared `dock` section; changes are re-read and drawn before the reply |
| `keys [--dock]` | sorted setting names with current and default values; secret values are masked |
| `unset [--dock] <key>` | restore a setting's default |
| `describe` | print the widget description and defaults |
| `startup on\|off\|get` | inspect or change Start with Windows |
| `status` | process state, pid, build and reason; status is read locally and works while stopped |
| `start`, `restart` | start a widget, or quit and start it again; refused from a private Claude copy |
| `reload`, `ping` | force settings/look refresh, or quietly return pid/build time |
| `widgets list\|register <exe>\|remove <name>` | inspect or change the registered widget list |
| `do help` | list this widget's content commands and one-line usage |
| `do <name> [args]` | run a widget content command; the default timeout is 120 seconds |
| `log [n]` | the widget's log, last n lines (`Spec::log_file`) |
| `redraw`, `quit` | as Block does them |
| `simulate taskbar-restarted`, `simulate look-changed` | test only: Block's Explorer-restart path; re-read Windows' theme and tell the widget even if unchanged |

How it works (`src\ctl.rs`, `src\client.rs`, `src\block\remote.rs`): protocol v2 sends
each `WM_COPYDATA` request with a process-unique id; replies echo it, and dockctl accepts a reply
only from the widget window it asked. It pumps messages until the matching reply or deadline.
Slow content work may reply later through Block's UI thread. The widget copies request bytes
before returning from the message. The widget does the reading and writing, so it's the real
`%APPDATA%` even from a Claude session. Build: `cargo build --release --bin dockctl`, then copy
`target\release\dockctl.exe` to `taskbar-dock\`.

#### `do` commands per widget

`dockctl <widget> do help` prints each command's usage. Bad arguments give
`{"ok":false,"kind":"usage",…}` with the usage line and exit code 2. Commands that wait on a
worker (timetable, network, Discord, the media session) reply when it finishes; nothing blocks
the widget meanwhile.

- **bus**: `now` (line, stop, the next six departures, live or scheduled, and the age of the live
  data the widget already has: no new OVapi request); `stops <query>` (up to 20 matching stops);
  `lines <stop_code>` (lines and directions, numbered); `choose <stop_code> <line> [agency]
  [--to <n|text>]` (saves the stop and line as the picker's Select does; a line with several
  directions fails and lists them until `--to` picks one). Timetable reads go through one queue
  shared with the picker and never download: without a timetable on disk they say so.
- **weather**: `now` (place, temperature, conditions, last update); `places <query>` (up to 5
  Open-Meteo geocoding matches, numbered); `place <n | lat,lon | auto>` (`n` from the last
  `places` reply, valid 10 minutes; `auto` = by IP). `place` saves the location, then replies
  once the new weather (or the fetch error) is in.
- **media**: `now`, `sessions` (one per app, with its id); `play`, `pause`, `toggle`, `next`,
  `prev`, `skip <±s>`, `seek <s>`, `volume <0-100>`, `mute on|off`, each with `--app <id or
  name prefix>` (default: the session the strip shows). Unknown position or length is null.
- **discord**: `now` (status, channel, members speaking / muted); `mute on|off`, `deafen on|off`
  (set, not toggle: already in that state replies `"changed": false` at once); `leave`. These
  wait for Discord's answer and fail on an error, a disconnect or after 10 s. Sign in and
  Reconnect stay `menu click`.

Examples:
```
dockctl weather state
dockctl weather menu click "Compact mode"
dockctl weather set show_on_taskbar false
dockctl all state
dockctl bus do choose 59150850 3 --to "Utrecht CS"
dockctl media do pause --app spotify
```

`scripts\restart_explorer.ps1` restarts Explorer without keystrokes (the taskbar's own Exit
Explorer message, then explorer.exe started through a one-off scheduled task). Use it with
`dockctl … state` / `log` to check the Explorer-restart path for real.

### Other rules (the test order and mouse rules are in CLAUDE.md "Testing", "Hard rules")

- Prefer unit tests for rules: layouts, timings and behaviour live in the library as plain functions so they can be tested without windows (see `panel.rs`, `badge.rs`, `draw.rs`).
- On screen, measure instead of eyeballing: UI Automation gives the strip and tray button rectangles, and a screenshot of the taskbar gives pixel extents (that's how the clock baselines and the tray-icon box were found).
- PowerShell trap: passing `$null` to a P/Invoke `string` parameter sends `""`, so `FindWindow(class, $null)` finds nothing. Use `[NullString]::Value`.
- Don't re-download big data (e.g. a GTFS zip) just to test. Use the cached copy or a saved sample in `tests\`.
