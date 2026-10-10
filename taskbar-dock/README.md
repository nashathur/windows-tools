# taskbar-dock

A shared Rust library for the taskbar widgets: the bus widget (`C:\Users\thurs\tools\buswidget`), taskbar-weather (`C:\Users\thurs\tools\taskbar-weather`), taskbar-media (`C:\Users\thurs\tools\taskbar-media`), taskbar-discord (`C:\Users\thurs\tools\taskbar-discord`), and any new one. A widget built on it docks into the Windows 11 taskbar next to the others. It looks and behaves the same way and follows Windows settings live.

## What's in it

| Module | What it gives a widget |
|---|---|
| `settings` | One settings file for all widgets: `%APPDATA%\TaskbarDock\settings.json`. Each widget has its own section and the shared one is `dock`. Each widget rewrites only its own section, and every write is atomic. |
| `style` | The shared look and behaviour from `dock` (gap, font, radii, timings, hover delays, "follow Windows" switches), resolved into a `Look` for right now. |
| `system` | The Windows settings that are followed live: theme, Animation effects, mouse hover time, Text size, time format, and display scale per monitor. Also time formatting. |
| `dock` | The stacking protocol: tray boundary, member/dragging window properties, order file, live drag preview, drop reorder, WinEvent hooks for instant reflow, and "one panel at a time". The fit next to the taskbar's own buttons: each member publishes its widths (`publish_widths`), `slot` gives its x and its `Fit` (full, narrower, hidden), `FitState` says what changed. |
| `icons` | The taskbar's own buttons (Start, apps) through UI Automation, on a worker thread, read only by the stack's leader (the member nearest the tray), which publishes their right edge for the others. |
| `panel` | How a widget's panel opens and closes: hover, click, pin, right-click, drag, one at a time, and the fade-in. The same for every widget, with tests. |
| `menu_panel` | The right-click panel, in place of a Windows menu: an optional title and subtitle, action rows with an icon, on/off switches (flip and stay open), separators, and "Edit settings… / Quit" side by side. `run(anchor, menu, look, scale, on_switch)` works like `TrackPopupMenu` (own message loop, returns the action chosen); call `close()` on the shared close-panel message. Same colours as the other panels. With tests. |
| `strip` | How a strip moves in the dock: slide right / jump left into its slot, press vs drag (3 px), follow the mouse centred while the content zooms to `drag_zoom`, shrink back after the drop. `Motion` answers with positions and does no window work (move outside your state's borrow); `move_strip` puts a child or floating strip at a screen x. A width change keeps the right edge (`keep_right_edge`); switching compact mode animates the width (`resize`, `resize_width`); a drag stays within `Limits` (tray, taskbar buttons). With tests. |
| `draw` | Drawing for a per-pixel-alpha taskbar strip: text colour, two lines on the clock's baselines, icons in the tray-icon box, rounded shapes, the hover highlight (whole strip, or one button with `background_in`), and an anti-aliased `progress_ring` along a rounded square (fractional positions allowed). |
| `badge` | A coloured, rounded badge with short text (e.g. a bus line number): tray-icon tall, as wide as its text, text centred, shared corners and font, automatic black/white text. |
| `anim` | Easing and animation progress (0 ms when animations are off). `pulse`: a slow, smooth 0 → 1 → 0 every 2.4 s for "alive, nothing else to show"; `pulse_settle`: gliding down to 0 from wherever a pulse stopped. |
| `marquee` | Scrolling text for a line too long for its fixed-width box: still if it fits; otherwise wait 1.5 s, then scroll left at 30 px/s in a loop (the start follows 40 px after the end) until it's back where it began, and repeat. `fade_edges` softens the cut-off edges. Still (cut with "…") when animations are off. |

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
  "fade_ms": 150, "glide_ms": 150, "zoom_ms": 120,
  "drag_zoom": 1.15,
  "panel_margin": 12
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
| `fade_ms`, `glide_ms`, `zoom_ms` | Panel fade-in, slide into place, and drag zoom durations. |
| `drag_zoom` | Content size while dragged (1.15 = 115%). |
| `panel_margin` | Pixels (at 100%) a panel keeps from the taskbar and from the screen edges. Windows 11's own flyouts use 12. |

Missing keys are filled in with the defaults the first time a widget starts, so the file always shows every option. Edits apply within a second, without restarting the widgets.

## Corner design

Source: arun.is, "The secret formula for Apple's rounded corners" (2023, updated 2026), plus
the r/apple discussion of it. The rules to follow are in `CLAUDE.md` ("UI: rounded corners");
this is the reasoning.

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
instead (superellipse-like; Figma's "corner smoothing 60%" is the common approximation). The
r/apple thread disagrees on whether Apple's exact curve is a superellipse or a hand-drawn
spline; for our purposes only the property matters: no visible seam, radius appearing larger
near the straight edges. At 4 px it can't be seen, so small corners stay circular arcs;
`draw::shape_cover` switches to the continuous corner above 8 px (superellipse n = 3.2, reach
1.5 × radius, chosen so the diagonal cut is as deep as the circle's; checked by a unit test,
not against Apple's real curve). Not yet looked at on screen at 100% and 200% scale.

## Building a new widget

The steps below follow the bus widget. `buswidget\rust\src\main.rs` uses `windows-sys` like this library, so it's the closest template. taskbar-weather shows the same steps with the `windows` crate. It converts handles with `h.0 as isize`.

1. **Dependency**: `taskbar-dock = { path = "C:/Users/thurs/tools/taskbar-dock" }` (or a relative path).
2. **Manifest**: embed a Windows 8+ `supportedOS` app manifest. A transparent (layered) child window needs it. Copy `buswidget\rust\build.rs` and `buswidget.exe.manifest`.
3. **DPI**: call `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` at startup.
4. **The strip**: create a `WS_EX_LAYERED` `WS_CHILD` window with `dock::taskbar()` as parent and a unique window class, which is the widget's name in the dock order. Then call `dock::mark_member(hwnd)`.
5. **Look**: keep a `Look` with `Look::current()`. Refresh it every second and on `WM_SETTINGCHANGE`/`WM_DPICHANGED`, and redraw when it changed. Get the scale with `system::scale_for(taskbar)`.
6. **Drawing** (see the bus widget's `render_bar`):
   1. Make a 32-bit DIB as tall as the taskbar.
   2. Draw the text white on black with GDI at `draw::two_line_tops(...)`, then call `draw::text_to_color` with `look.text_rgb()`.
   3. Icons and badges go in the tray-icon box from `draw::icon_box(height, scale)`. That's the same 16 px size and height as Windows' tray icons (network, volume), so everything lines up.
      - For a glyph (emoji, symbol font, future icons), draw it large (~64 px) white on black in a scratch buffer, then call `draw::fit_icon`. It scales the visible part to fit the box and centres it, whatever spacing the font adds.
      - For a badge (a coloured, rounded shape with short text, like the bus line number), use `badge`. Make the font with `badge::font(&look, scale)` and measure with `Badge::measure(dc, font, text, height, scale)`; its `.width` is the room it takes. After `text_to_color`, call `.paint(dc, buf, width, x, font, bg, fg, &look, scale)`. It's tray-icon tall, as wide as the text plus 4 px each side (at least square), with the text centred by its visible pixels and the shared corner radius. Pass `None` as `fg` for automatic black or white text.
   4. Draw opaque shapes, then `draw::round_rect`.
   5. Call `draw::background` (hover highlight) and `UpdateLayeredWindow`.
7. **Position**: after drawing, `dock::publish_widths(me, Size { full, min, stretch })` (full: in the chosen mode; min: compact; stretch: any width between, like media's text column). Then `dock::slot(taskbar, me, size, dock::tray_boundary(taskbar, scale), gap_px)` gives `x_for(drawn width)`, `slide` and `fit`. `None` means "stay put": the taskbar's window list is briefly empty while the action center is open. Feed `fit` to a `dock::FitState`: `Redraw` → draw compact or at the narrowed width; `Hide` → close the panel, `set_squeezed(true)`, hide, log; `Show` → `set_squeezed(false)`, draw, show, log. Keep checking once a second while hidden (no events reach a hidden strip). Move there with `strip::Motion::glide_to` (it slides right and jumps left) and its `Glide` answer; see `glide_to` in the bus widget. Re-check once a second.
8. **React instantly**: keep a `dock::Hooks` and call `hooks.update(taskbar, Some(callback))` every second. In the callback, re-snap when `dock::is_neighbour_event(...)` is true.
9. **Drag**: keep a `strip::Motion`; the rules and the wiring are at the top of `src/strip.rs` (the bus widget's `placement.rs` is the example):
   - Mouse-down: capture the mouse and `press`. Mouse move: `mouse_move`; on `Started` call `dock::set_dragging(me, true)`, `panel.drag_started()`, start the zoom timer, `dock::notify(me)`; move to the x it returns, and after every move of the dragged strip call `dock::track_drag(taskbar, me, boundary, gap)` (it decides the sticky drop slot and publishes it; does nothing when not dragged). Pass `strip::Limits` with `max` against the tray and `min` = `dock::icons_right(taskbar) + gap` (not over the taskbar's buttons).
   - Release (or `WM_CAPTURECHANGED`): `release()`. `Click` is the click; on `Drop` call `dock::drop_reorder(...)` and only then `set_dragging(false)` (the others keep the preview until the new order is saved), start the zoom timer, glide into the slot, `dock::notify(me)`.
   - Zoom timer: `zoom_step`, redraw at `zoom()`, then follow `drag_x` or glide to the slot.
10. **Panel**: when it opens and closes is decided by `panel::PanelBehaviour`, the same for every widget. Don't write your own hover/click logic. The rules are in `src/panel.rs` and tested there:
    - Hovering opens it after `look.hover_open_ms` without taking focus. It closes `look.hover_close_ms` after the mouse leaves both widget and panel.
    - A click opens it with focus, or closes it. A click on a hover-opened panel keeps it open.
    - A right-click isn't a hover, and starting a drag closes a hover-opened panel.

    Wiring:
    - Keep a `PanelBehaviour` and a `panel::Fade`.
    - Report events:
      - `mouse_entered` on the first `WM_MOUSEMOVE` (then start a 50 ms timer).
      - `mouse_left(pointer outside the strip)` on `WM_MOUSELEAVE`.
      - `click` on a left click that wasn't a drag.
      - `button_click` instead, for a click on a button on the strip (e.g. play/pause): it doesn't open, pin or close the panel, and a pending hover-open waits its full delay again.
      - `right_click(on_panel)` before the context menu, and `menu_closed` after it.
      - `drag_started` / `drag_ended`.
      - `closed(lost_focus)` when the panel closed by itself: deactivated, Esc, or `dock::close_panel_message()` from another widget.
    - On the timer, call `tick(now, on_strip, on_panel, &look)`. Stop the timer when `wants_ticks()` is false.
    - Do what each returned `Action` says:
      - `OpenByHover`: show without activating.
      - `OpenByClick`: show and `SetForegroundWindow`.
      - `Pin`: `SetForegroundWindow`.
      - `Close`: hide.
    - Make the panel a top-level `WS_EX_LAYERED` popup with DWM `DWMWCP_ROUND`. When showing it, call `dock::close_other_panels(taskbar, me)` (one panel at a time), set alpha 0 and `fade.start(now)`. Then every 10 ms call `SetLayeredWindowAttributes` with `fade.alpha(now, &look)` until it returns `None`.
    - Position it with `panel::place(strip, w, h, &look, scale)`. This centres it on the widget's centre, `panel_margin` above the taskbar, and keeps it inside the monitor's work area (it shifts sideways near a screen edge, and works on any monitor). `panel::position` is the same rule for any anchor rectangle.
    - Make the panel follow the theme: pick its colours from `look.light`, and set DWM `DWMWA_USE_IMMERSIVE_DARK_MODE` to match.
    - See `panel_do` in either widget.
11. **Mouse on the strip**:
    - Return `MA_NOACTIVATE` from `WM_MOUSEACTIVATE`, so clicking the widget doesn't activate the taskbar.
    - On the first `WM_MOUSEMOVE`, call `TrackMouseEvent(TME_LEAVE)` and redraw with the hover highlight. On `WM_MOUSELEAVE` (0x02A3, which windows-sys 0.52 doesn't export there), redraw without it.
12. **Context menu** (right-click):
    - The strip is a child of the taskbar, so give `TrackPopupMenu` a top-level owner (e.g. the hidden panel or a hidden main window). Call `SetForegroundWindow(owner)` first, or the menu won't close when you click elsewhere.
    - Use `TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN`.
    - Call `panel.right_click(...)` before it and `panel.menu_closed()` after it (step 10). A right-click on the panel can show the same menu (the weather widget does), with `right_click(true)`.
    - Standard items for every widget: **Edit settings…**, which opens `settings::path()` in Notepad (edits apply live), and **Quit**/**Exit**.
13. **Settings**: keep your own settings in your own section with `settings::read()` and `settings::save_section("yourwidget", value)`.
    - Watch `settings::mtime()` once a second to pick up edits. Do the same for any data file the user may edit (e.g. the bus widget's `line_colors.txt`).
    - Secrets (API keys) go in your section but are never shown in full in logs, menus or panels; mask them.
14. **Times**: use `look.time(h, m)` / `look.hour(h)`, never a fixed format.
15. **Data and network**:
    - Fetch on a worker thread, never on the UI thread, and redraw from the UI timer.
    - Be careful with APIs: cache responses on disk and reuse them, poll no more often than the data changes, and only fetch when it's useful (the bus widget fetches live times only when a bus is due within 90 minutes).
    - On HTTP 429 or an error, back off: keep the old data, save the next allowed try to disk so a restart doesn't retry early, and show that the data is stale rather than an empty widget.
16. **Lifecycle**:
    - **One instance**: create a named mutex (`Local\YourWidget.SingleInstance`) and exit if it already exists.
    - **Explorer restarts** destroy the taskbar and the strip with it. Don't quit on `WM_DESTROY` unless the user chose Quit. Once a second, check that `dock::taskbar()` still is the parent; if it changed, create the strip again. Also listen for the `TaskbarCreated` registered message.
    - **Fallback**: if attaching to the taskbar fails, show the strip as a floating topmost window just above it, and hide it while a full-screen app is in front.
    - **Start with Windows**: `taskbar_dock::startup::set("<name>", on)` and `is_on("<name>")` put the widget on the `startup` list in settings.json. The launcher `taskbar-dock-start.exe` (src/bin, installed as `taskbar-dock\taskbar-dock-start.exe`) is run at sign-in by the Startup-folder shortcut "Taskbar widgets" and starts everything on the list (problems go to `startup.log`). It then stays as a watchdog: a widget it started that crashes (exit code 0xC0000xxx) is started again after 2, 4, 8 … s, and given up on after 5 crashes in 10 min; Quit or being stopped is left alone (`taskbar_dock::crash`). Call `taskbar_dock::crash::log_panics(log)` first thing in `main` so the reason is in the widget's log. Don't use the `HKCU\...\Run` key: on this Windows build its values were skipped at sign-in. Offer it as a menu item if it's easy.

## Building

```
cargo build --release
cargo test --release
```
- The widgets build the library as part of their own builds. Run the library's tests after changing it.
- Keep paths short, because MSVC `link.exe` fails on paths over 260 characters.
- Don't `cargo clean`: keeping `target\` makes rebuilds take seconds.
- A running widget locks its exe. Stop the process before rebuilding or copying it, then start it again. The bus widget runs from a copy (`buswidget\buswidget.exe`); the weather widget runs from `target\release`.

## Testing

### dockctl: command-line control of widgets on Block

`taskbar-dock\dockctl.exe <weather|bus|discord|media|all> <command> [args]` talks to a running
widget and prints JSON. Only widgets on Block answer (weather and discord now; bus and media once ported); one
that isn't is reported as `"absent": "running, not on Block yet"` or `"not running"`.
For `all` that isn't an error. Exit code: 0 ok, 1 error / no reply, 2 usage.

| Command | Does |
|---|---|
| `state` | pid, exe build time; strip (hwnd, rect, shown, visible_by_state, show_on_taskbar, compact, fit, floating, side), panel (open, pinned, focused, rect), menu (open, focused, rect), tray (shown, tip), look (theme, scale) |
| `panel open\|close\|toggle` | the left-click panel, as a click on the strip does |
| `menu open\|close` | the right-click panel |
| `menu rows` | id, label, switch state of every row (of the open menu, or the one a right click would show) |
| `menu click <id\|label>` | a row, as a click does: a switch flips and the menu stays open; opens the menu first if needed |
| `tray click left\|right` | a click on the tray icon (error while it isn't shown) |
| `set <key> <value>` / `get [key]` | the widget's settings section, written by the widget (`settings::update`) and followed at once; value is JSON or else text; secrets (`*key*`, `*token*`, …) print as `***` |
| `log [n]` | the widget's log, last n lines (`Spec::log_file`) |
| `redraw`, `quit` | as Block does them |
| `simulate taskbar-restarted`, `simulate look-changed` | test only: Block's Explorer-restart path; re-read Windows' theme and tell the widget even if unchanged |

How it works (`src\ctl.rs`, `src\block\remote.rs`, `src\bin\dockctl.rs`): dockctl finds the
widget's hidden main window (`<strip class>Main`, owned by the widget's exe) and sends a
`WM_COPYDATA`; Block runs the command through its own paths and replies with a `WM_COPYDATA` to
dockctl's window. The widget does the reading and writing, so it's the real `%APPDATA%` even
from a Claude session. Build: `cargo build --release --bin dockctl`, then copy
`target\release\dockctl.exe` to `taskbar-dock\`.

Examples:
```
dockctl weather state
dockctl weather menu click "Compact mode"
dockctl weather set show_on_taskbar false
dockctl all state
```

`scripts\restart_explorer.ps1` restarts Explorer without keystrokes (the taskbar's own Exit
Explorer message, then explorer.exe started through a one-off scheduled task). Use it with
`dockctl … state` / `log` to check the Explorer-restart path for real.

### Other rules

- Prefer, in this order: unit tests, dockctl, logs and geometry scripts that print numbers,
  screenshots (`local_widget_check`, which opens Block widgets' panels through dockctl).
- Prefer unit tests for rules: layouts, timings and behaviour live in the library as plain functions so they can be tested without windows (see `panel.rs`, `badge.rs`, `draw.rs`).
- On screen, measure instead of eyeballing: UI Automation gives the strip and tray button rectangles, and a screenshot of the taskbar gives pixel extents (that's how the clock baselines and the tray-icon box were found).
- Mouse tests may move and click only on the widgets, their panels and their own menus. Find the windows first and stop if one isn't found or its rectangle looks wrong, so nothing is clicked by accident.
- Never simulate system-UI keys (Win+A, Win+N, …) on the user's desktop. Ask the user to try those instead.
- PowerShell trap: passing `$null` to a P/Invoke `string` parameter sends `""`, so `FindWindow(class, $null)` finds nothing. Use `[NullString]::Value`.
- Don't re-download big data (e.g. a GTFS zip) just to test. Use the cached copy or a saved sample in `tests\`.