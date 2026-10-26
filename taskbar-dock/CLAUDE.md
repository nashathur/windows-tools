# taskbar-dock: shared library for the taskbar widgets

Widgets using it: `..\buswidget` (bus departures), `..\taskbar-weather`, `..\taskbar-media`,
`..\taskbar-discord` (Discord voice chat via Discord's local RPC), `..\taskbar-sysmon` (CPU / GPU /
RAM / network, with an elevated CPU-temperature helper in `temp\`). Each widget's CLAUDE.md imports
this file. Reference, read by section when the task reaches it:
- **BLOCK.md**: Block's API (Spec, Content, Ctx, panel shell, tray icon, settings it owns). Read
  before changing Block or building or changing a widget.
- **UI-RULES.md**: corners, button highlights and hitboxes, cover/logo click and feel. Read before
  changing any drawing, hit box, highlight, radius or cover behaviour.
- README.md: the recipe for a new widget (a `Spec` + a `Content`), the shared `dock` settings,
  corner design reasoning, dockctl ("Testing"). plan\PLAN.md: open ideas.

## Library modules (windows-sys 0.52, handles are isize; `windows` 0.58 only for UI Automation in icons)
**A widget is a `Spec` + a `Content` handed to `block::run`; Block does the rest** (BLOCK.md).
A widget's own code uses settings, style, system, theme (every colour, font and size; checked by
`tests\theme_lint.rs`), draw, text_edit, emoji, badge, anim, marquee, window; the other modules
are Block's internals. One line per module: README.md "What's in it".
The settings app crate is `settings-app\`; build, install and `dockctl settings` in README.md "Settings app".

## Where a rule lives (rule)
A behaviour every widget shares is written once, in Block or in `theme`, never copied into a
widget: **change it there, and every widget has it.** Only content is per widget (what the strip
and panels show, their rows, their data). If a change would touch the same code in two widgets,
it belongs in Block (or `theme`); if Block can't express it, add a `Spec` field or a `Content`
method with a default (and a test in `block/core.rs`), don't special-case one widget. A change to a
rule in UI-RULES.md is made in Block / `theme`, then checked on every widget (dockctl `state`,
`_tmp\shot_strip.ps1`).

## Every widget has these five parts (rule)
1. **Full mode**: the normal strip.
2. **Compact mode**: the smallest useful strip (icon/badge + the key values; weather: temperature over
   humidity, bus: next two departures; others one line, vertically centred). Setting `compact` (bool)
   in the widget's section, switchable from the right-click panel; the strip redraws and glides back
   into its dock slot on change.
3. **Hover panel**: opens after the hover delay without taking focus, closes when the mouse
   leaves (`panel::PanelBehaviour`). It may be the left-click panel shown unpinned.
4. **Left-click panel**: the main panel, pinned (stays until clicked away or clicked again).
5. **Right-click panel**: a panel in the same look as the others (not a Windows menu): the
   widget's actions, then Show on taskbar, Compact mode and Start with Windows as on/off
   switches, Edit settings…, Quit (Block's `menu_panel` with its standard rows).
   Right click while the left-click panel is open switches to it, and back.
Every widget has the Highlight box settings (Block's); a new widget supplies its automatic colour
through `Content::tint`, or None for the neutral box.
A new widget isn't done until it has all five. The parts themselves are Block's: a widget
supplies the content of each (the strip's two modes, the panels' drawing, its menu rows).
Every widget also supports Idle (BLOCK.md "Idle"): Start with Windows is on by default; switched
off, a widget with a tray icon idles. A new widget implements `Content::idle` for worker threads or
fetches it runs outside content timers.

## Working on any widget
- Build/test/restart with the widget's `dev.ps1` (wraps `scripts\dev.ps1`): prints only errors,
  warnings, test totals and new log lines. Switches: `-NoTest -NoRestart -Lib -Start -Fast -NoSmoke`.
  A widget that wasn't running is left stopped unless `-Start`. After a restart it runs a smoke
  check (dockctl `state`, `do help`, `do now`; read-only), then checks that the same pid still
  runs; "SMOKE FAILED" exits 1.
- Use `-Fast` in the edit loop (`[profile.fast]`: release without LTO, incremental; whole dev.ps1
  ~1 s vs ~14 s); build the small full-LTO `release` exe once at the end. Tests always run in
  `fast`. `rust-lld` and splitting crates were measured to gain nothing (the time is LTO).
- Library tests: `dev.ps1 -Lib` from any widget, or `cargo test --profile fast` here.
- After a library or app change: `scripts\build_all.ps1` rebuilds and installs everything;
  `scripts\status.ps1` then must report nothing OUT OF DATE (README.md "Building").
- Big files: a widget's `main.rs` holds its `App` (the `Content`), the `Spec` and `main`; features
  live in modules. `scripts\split.sh` moves a line range into a module.
- Version control: `C:\Tools` is one git repo. Before changing the library, make
  sure the last good state is committed (ask before committing), and keep all widgets building.
- Start with Windows (README "Building a new widget", step 7): the launcher `taskbar-dock-start.exe`
  and its watchdog. After rebuilding the launcher, copy `target\release\taskbar-dock-start.exe`
  to `taskbar-dock\`.
- Z-order: a docked strip's `SetWindowPos(h, HWND_TOP, ..)` always ORs in `dock::raise_flag(h)`
  (`strip::move_strip` does), so it raises only when the taskbar's own content is above it or
  while dragged; raising on every redraw makes the widgets raise each other and flicker. Probe:
  `tools\_tmp\watch_z.ps1` (0 restacks at rest).
- Read files by section (grep the `// ----` headers or `fn name`, then read that range), not whole.

## Code tools (MCP `local-llm`, free; decide yourself, state the choice in one line)
- `local_code_search`: find code by meaning ("where does X happen?"); returns whole functions with file:line, so open only those. Grep or `sg` (ast-grep) for exact names/patterns.
- `local_docs`: which windows-sys function/constant/feature to use, or other dependency APIs, instead of web searches.
- `rust_refs` / `rust_rename` (rust-analyzer, exact, no model): all uses of a Rust symbol across every crate (incl. taskbar-dock used by all widgets), and renames of it (compile-checked diff). Prefer these over grep or `local_patch` for finding uses and renaming.
- `local_patch`: small mechanical edits the compiler can verify (mirror a change into the other widgets, add a field/switch like an existing one, add a similar test). Give exact instructions, the files (line ranges when big) and an example diff; review the returned diff before applying. Not for design, new logic or debugging.
- `local_widget_check`: screenshot a running widget (strip, hover/click panels) and diff against a baseline after UI changes; view the PNG yourself when pixels matter.

## Testing (prefer in this order)
1. Unit tests (pure logic in the library: `block/core.rs`, `ctl.rs`, `panel.rs`, …).
2. **dockctl** for widgets on Block (weather, discord, bus, media, sysmon):
   `taskbar-dock\dockctl.exe <widget|all> state|panel …|menu …|tray click …|set|get|log|redraw|quit|simulate …`
   (README.md "Testing"). It runs commands through Block's own paths in the running widget, so
   settings, logs and state are the REAL `%APPDATA%`. Use it instead of `_tmp\real.ps1` for the
   widget's settings and log, and instead of mouse clicks for opening panels and menus and
   flipping switches. `scripts\restart_explorer.ps1` restarts Explorer (no keystrokes).
3. Logs and geometry scripts that print numbers (`_tmp\watch_z.ps1`, `fit_test.ps1`,
   `drag_probe.ps1`, `shot_strip.ps1`; `_tmp\real.ps1` for a widget not on Block).
4. Screenshots: `local_widget_check` (click / rightclick go through dockctl for Block widgets:
   no mouse moved), only where pixels matter.
5. End-to-end, before calling a Block or settings-app change done: `scripts\live\run.ps1`
   (one report with screenshots; README.md "Live checks").

Bug fix: first make the bug go red at the lowest rung above that reaches it (a failing unit test,
else a dockctl or log/geometry check), then fix it and watch it pass; keep the test. If no rung
reaches the bug's real path, say so instead of adding a shallow test.

## Hard rules
- `%APPDATA%\TaskbarDock\settings.json` and source files: write UTF-8 only (Rust/serde, the
  Write/Edit tools, or `[IO.File]::WriteAllText(path, text, (New-Object Text.UTF8Encoding $false))`).
  Never PS 5.1 `Set-Content`/`Out-File -Encoding ASCII`/`Get-Content | ConvertTo-Json` round trips:
  they have turned "→" into mojibake and non-ASCII into "???" before.
- **Claude desktop sessions see a private `%APPDATA%`** (MSIX): reads/writes under
  `AppData\Roaming` from session shells, and widgets started from them, go to
  `AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\TaskbarDock`, not the real folder
  the sign-in launcher and Explorer-started widgets use. To read/write the real folder or start widgets as at sign-in, use a one-off scheduled task
  (`Register-ScheduledTask` + `Start-ScheduledTask`, then unregister it); `scripts\dev.ps1` already
  starts widgets that way. After changing settings.json from a session, copy it to the real folder
  that way. Never start a widget directly from a session shell.
- Test clicks may only move/click on the widgets, their panels and their own menus. Find the
  window first; stop if it isn't found or its rectangle looks wrong. Validate coordinates.
- Never simulate system-UI keys (Win+A, Win+N, …) on the user's desktop.
- No paid services.
