# taskbar-dock: shared library for the taskbar widgets

Widgets using it: `..\buswidget` (bus departures), `..\taskbar-weather`, `..\taskbar-media`,
`..\taskbar-discord` (Discord voice chat via Discord's local RPC).
Each widget's CLAUDE.md imports this file. README.md here is the full recipe for a new widget;
read only the section you need.

## Library modules (windows-sys 0.52, handles are isize; `windows` 0.58 only for UI Automation in icons)
settings (shared settings.json, one section per app; `update` holds a cross-process lock, writes are temp file + rename) · crash (`log_panics`, restart rules for the launcher's watchdog) · style (`Look`: theme, fonts, colours) ·
system (Windows settings) · theme (design code: colours light/dark, `brand` allow-list, type, spacing, sizes; widgets take every colour, font and font size from it, checked by `tests\theme_lint.rs` in dev.ps1) · dock (stack order against the tray, `slot`, `notify`, sticky drop slot `track_drag`, WinEvent
hooks; `fit` next to the taskbar's buttons: narrow, compact, hide) · icons (taskbar buttons via UIA,
read by the leader only) · panel (`PanelBehaviour` hover/click/pin, `Fade`, `place`) · menu_panel (right-click panel:
rows, switches, `run` like TrackPopupMenu) · strip (`Motion`: glide, drag,
zoom, compact resize animation; `Limits`; `keep_right_edge`; `move_strip`; `client_pos`) · draw (corners: `shape_cover`, `round_rect`, `fill_round`, `card_radius`, `concentric`; `background_in`, `background_rect`, `button_box`, `centered_box`, `contains`, `grow_box`, `grow_pixels`, `tint_box`, `progress_ring`) · emoji (colour emoji via Direct2D + DirectWrite: `render`, `fit_over` into a strip's icon box, `flatten` for a GDI panel; GDI draws emoji in one colour) · badge · anim
(`pulse`, easing, `Feel`: cover hover/press size) · marquee (scrolling text) · window (`raise`, `find`, `exe_path`: bring an app's window to the front).
block (step 2 of plan\PLAN.md, not used by a widget yet): the strip window and all its behaviour; a
widget gives a `Spec` and a `Content` and calls `block::run`. `mod.rs` API (Spec, Content, Ctx requests
applied after the borrow, Frame, Canvas, Hit, MenuBuilder with the standard rows, `with` for the widget's
own windows/worker posts, `panel_closed`/`right_click_on_panel` for the interim `Spec::panel` hook),
`core.rs` (pure decisions, tested with a fake Content), `place.rs` (left side, floating), `win.rs`
(windows, timers, dock/fit/hooks, Explorer restart, floating fallback, menu).
Shared front end: all three widgets use `strip` (glide, drag, zoom) and `panel` (when the panel
opens/closes, fade, placement); each widget only wires them to its own windows and timers.
NOTES-2026-10-09.md: build-time measurements, the drag-slot fix, ideas from TaskbarWidgets (section 8:
what was taken on 2026-10-10: sticky slot, drop order, crash watchdog, settings lock), and
the icons-as-obstacles fit (section 6; test aid `TASKBAR_DOCK_TEST_ICONS_RIGHT`).

## Every widget has these five parts (rule)
1. **Full mode**: the normal strip.
2. **Compact mode**: the smallest useful strip (icon/badge + the key values; weather: temperature over
   humidity, bus: next two departures; others one line, vertically centred). Setting `compact` (bool) in the widget's section, switchable from the
   right-click panel; the strip redraws and glides back into its dock slot on change.
3. **Hover panel**: opens after the hover delay without taking focus, closes when the mouse
   leaves (`panel::PanelBehaviour`). It may be the left-click panel shown unpinned.
4. **Left-click panel**: the main panel, pinned (stays until clicked away or clicked again).
5. **Right-click panel**: a panel in the same look as the others (not a Windows menu): the
   widget's actions, Compact mode and Start with Windows as on/off switches, Edit settings…,
   Quit. Right click while the left-click panel is open switches to it, and back.
A new widget isn't done until it has all five; a change to one widget's part should be
mirrored in the others. Status 2026-10-09: all four widgets have all five. Bus, weather and discord use the shared
`menu_panel` for the right-click panel; media has its own (panel_view.rs, same look).

## Working on any widget
- Build/test/restart with the widget's `dev.ps1` (wraps `scripts\dev.ps1`): prints only errors,
  warnings, test totals and new log lines. Switches: `-NoTest -NoRestart -Lib -Start -Fast`.
  A widget that wasn't running is left stopped unless `-Start`.
- Build profiles: tests always run in `[profile.fast]` (release without LTO, incremental: ~1 s
  after a change). `-Fast` also builds the widget with it: use it in the edit loop (bus: whole
  dev.ps1 incl. restart ~1 s, vs ~14 s; exe ~8% larger, bus 2x). Without it the deployed exe is
  the small full-LTO `release` build (13-15 s): do that once at the end. dev.ps1 waits for the
  restart by polling (process gone; log quiet 300 ms, or 1 s without log lines), not fixed sleeps.
  Measured 2026-10-09: the `rust-lld` linker gained nothing; splitting crates wouldn't either
  (crates are 2-5k lines; the time is LTO, which optimises all crates together).
- Library tests: `dev.ps1 -Lib` from any widget, or `cargo test --profile fast` here.
- Big files: `main.rs` holds App state, window procedures and `main`; features live in modules
  (`use super::*;`, items `pub(crate)`). `scripts\split.sh` moves a line range into a module.
- Version control: `C:\Users\thurs\tools` is one git repo (since 2026-10-10; replaces the old
  `src_backup_*` copies, which are git-ignored). Before changing the library, make sure the last good
  state is committed (ask before committing), and keep all widgets building.
- Local model help (MCP `local-llm`, free): `local_code_search` (find code by meaning across all
  widgets), `local_docs` (windows-sys API/feature lookup for a widget crate), `local_patch`
  (compile-checked mechanical edits, e.g. mirroring a change into the other widgets; review the
  diff), `local_widget_check` (screenshot strip/panels, diff vs baseline). `sg` (ast-grep) for
  exact structural search/rewrite.
- Start with Windows: the `startup` list in settings.json (`startup::set/is_on`), started at
  sign-in by `taskbar-dock\taskbar-dock-start.exe` via the Startup-folder shortcut "Taskbar
  widgets". It then stays as a watchdog (restarts a crashed widget with growing waits; not after Quit or
  Stop-Process, so dev.ps1 restarts are unaffected). Never the Run registry key (Windows skipped those values here). After rebuilding the
  launcher, copy `target\release\taskbar-dock-start.exe` to `taskbar-dock\`.
- Z-order: a docked strip's `SetWindowPos(h, HWND_TOP, ..)` always ORs in
  `dock::raise_flag(h)` (`strip::move_strip` does): it raises only when the taskbar's own
  content is above it, or while it's dragged. Raising on every redraw made the widgets raise each
  other through their WinEvent hooks (~7 restacks/s, measured 2026-10-09) and every strip
  flickered now and then. Probe: `tools\_tmp\watch_z.ps1` (prints restacks; 0 at rest).
- Read files by section (grep the `// ----` headers or `fn name`, then read that range), not whole.

## UI: rounded corners (rule; reasoning and table in README.md "Corner design")
Modelled on Apple's hardware/UI: the closer a surface is to the hand, the rounder it is.
1. **Roundness = 2 × radius ÷ shorter side.** Pick it by role, not by eye: big panels ≤ ~8%
   (use Windows' own DWM corner, below), buttons/hover highlights ~20-30% (`highlight_shape`),
   cards and cover art ~25% (`draw::card_radius`: side ÷ 8, 4-12 px), badges/chips 30-50%,
   dots, switches, avatars, progress ends 100% (radius = shorter side ÷ 2).
2. **Nested shapes are concentric:** inner radius = outer radius − the gap between them,
   `draw::concentric(outer, gap, floor)` with the control radius as floor for clickable things
   (menu rows: `PANEL_RADIUS` − inset). Never give a child the parent's radius.
3. **One source of radii:** `Look` (`control_radius`) via `draw::highlight_shape` /
   `card_radius` / `concentric`; widgets never hard-code a radius. Multiply by DPI `scale`,
   round to whole px; `shape_cover`/`background_in` clamp to min(w, h) ÷ 2. One
   `draw::fill_round` (anti-aliased, alpha) for panel pixels; don't copy it into a widget.
   Known gap: weather's bars and discord's buttons are GDI `RoundRect` (not anti-aliased,
   drawn on a plain DC); the radius is shared, the edge is not smooth.
4. **Panels:** top-level popups get `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` (as
   `menu_panel`/`popup` do). Never `DWMWCP_DONOTROUND`, never a custom `SetWindowRgn` (jagged,
   no anti-aliasing).
5. **Drawn shapes** go through `draw::round_rect` / `background_in` (anti-aliased coverage, so
   the edge fades instead of stepping). Borders and rings follow the same shape (`progress_ring`
   runs along the rounded outline); inset a stroke by half its width.
6. **Radius above 8 px (`draw::SMOOTH_ABOVE`):** a plain circular arc shows where the straight
   edge ends and the curve starts, so `draw::shape_cover` (used by `round_rect` and
   `fill_round`) switches to a continuous-curvature corner: superellipse n = 3.2 reaching
   1.5 × radius along each edge, as deep on the diagonal as the circle (tested). Automatic;
   callers just pass the radius. Capsules and circles (radius ≥ shorter side ÷ 3) stay
   circular. Only the cover art in the media panel (12 px at 96 px) uses it today.
7. A change to a corner role changes it in every widget (same rule as the five parts).

## UI: button highlights (rule)
A button's hover / pressed / state highlight box is **its glyph plus a small margin, not the
full-height bar**: the tray-icon box (`draw::icon_box`, 16 px at 100%) + `draw::BUTTON_MARGIN`
(4 px at 100%) on every side, centred on the taskbar's middle and on the glyph, never wider than
the button's column. Get it from `draw::button_box(col, height, scale)` and draw it with
`draw::background_rect` (or a tinted copy with the same box and `highlight_shape` radius); never
pass a button's whole column or the strip's height. **The hitbox is the same box**: hit-test a
button with `button_box` at (x, y) (the layout keeps its drawn height and scale for this), so the
margin around the box, above and below it, belongs to the strip (click panel, drag), and what
lights up is exactly what is clickable. Only the *whole-strip* highlight spans
`(0, inset, width, height - inset)`. A brand colour may tint the box (discord: blurple, red for
Leave / switched-on), the geometry stays shared. A new button gets this by using `button_box`;
a change to the margin changes it in every widget (same rule as the five parts).

## UI: cover / logo opens the app (rule)
Clicking the strip's cover art or logo brings the app it shows to the front (media: the playing
app; discord: Discord), on release over the same spot; it leaves the panel be, a drag from it still
moves the strip. The widget only says which windows are the app's and the fallback to open;
`window::raise(is_app, fallback)` finds the main window, restores and raises it (or opens the
fallback: `shell:AppsFolder\<id>` or a `scheme://` link, for an app sitting in the tray). Hit-test
the cover with `draw::centered_box` + `draw::contains`, mouse position with `strip::client_pos`.
Widget colours go through `draw::tint_box` (under the pixels, same radius as the highlight).

**Cover feel:** the same box is the cover's hitbox and it reacts: it grows while hovered
(`anim::HOVER_GROW` 1.06) and dips while pressed (`anim::PRESS_SHRINK` 0.96), eased, and only
while the strip is at rest (`strip::Motion::calm`: a drag from the cover eases it back to normal
and leaves the size to the drag zoom; it reacts again on the next mouse move after landing).
`anim::Feel` holds the state: call `set(on, on && pressed_on_cover)`, on = over the cover && calm (then `snap()`
if animations are off) on every mouse move / down / up / leave / capture loss, run a
`FEEL_FRAME_MS` timer while `animating()`, `tick` it each frame and redraw. In the strip's
drawing, after the cover and its ring, `draw::grow_pixels(buf, w, ring_box, feel.value())`
rescales them about their centre. Hit-test the resting box (not the grown one). Media
(`main.rs` `feel_update`) and discord (`label.rs` `feel_update`) do this; a new widget copies it.

## Hard rules
- `%APPDATA%\TaskbarDock\settings.json` and source files: write UTF-8 only (Rust/serde, the
  Write/Edit tools, or `[IO.File]::WriteAllText(path, text, (New-Object Text.UTF8Encoding $false))`).
  Never PS 5.1 `Set-Content`/`Out-File -Encoding ASCII`/`Get-Content | ConvertTo-Json` round trips:
  they have turned "→" into mojibake and non-ASCII into "???" before.
- **Claude desktop sessions see a private `%APPDATA%`.** The Claude app is MSIX-packaged: every
  file read/write under `AppData\Roaming` from its shells (and widgets started from them) goes to
  `AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\TaskbarDock`, not the real
  folder the sign-in launcher and Explorer-started widgets use. Found 2026-10-10: the real
  settings.json had no `startup` list, so sign-in logged "started nothing". To read/write the
  real folder, or to start widgets as at sign-in, go through a one-off scheduled task
  (`Register-ScheduledTask` + `Start-ScheduledTask`, then unregister it). After changing
  settings.json from a session, copy it to the real folder that way. A widget started from a
  session with `Start-Process` reads the private order.txt and overlaps the others after a
  reorder (bus over discord, 2026-10-10); `scripts\dev.ps1` therefore starts widgets through a
  one-off scheduled task. Never start a widget directly from a session shell.
- Test clicks may only move/click on the widgets, their panels and their own menus. Find the
  window first; stop if it isn't found or its rectangle looks wrong. Validate coordinates.
- Never simulate system-UI keys (Win+A, Win+N, …) on the user's desktop.
- Prefer logs and pixel/geometry scripts that print numbers over screenshots.
- No paid services.
