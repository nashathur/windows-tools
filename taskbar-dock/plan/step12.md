# Step 12: the settings app

One window that is the control panel for every registered widget: an overview with status and
start/quit/restart, a page per widget, a Dock page for the shared look, and a Developer tab.
Decided with the user 2026-10-12 (grilling, plain-language rounds). Inspiration: pfcdev/TaskbarWidgets
(clone in `tools\_tmp\tbw`): its "Installed widgets" overview, setting cards (title, help line,
control on the right), sliders, segmented choices, the "Saved" badge, its Developer page.
Not taken from it: the web UI (Tauri), a Save button, search, categories, updates, a store.

## Decisions

1. **Registered widgets.** The app shows the widgets on a `widgets` list in settings.json, not
   whatever is on disk. The four (weather, bus, media, discord) are registered at install. The
   Developer tab registers (pick an exe) and removes; removing only takes the entry off the list
   (and the widget off `startup`), nothing is deleted.
2. **Home page ("Widgets")**: one row per registered widget: status dot + word, title, one-line
   description, Show on taskbar switch, and Start (stopped) or Quit + Restart (running). A click
   on the row opens the widget's page.
3. **Status**: green Running (answers), grey Stopped, orange Not responding (process or window
   there, no answer in 2 s), red Keeps crashing (the watchdog gave up). Refreshed every 2 s.
4. **Widget page**: Show on taskbar, Compact mode, Start with Windows on top (the same on every
   page), then the widget's everyday settings as cards in groups. Each card: label, help line,
   control on the right, a Reset (to default) button shown when the value differs from the default.
5. **Dock page**: the shared `dock` section, all keys, in groups (below).
6. **Developer tab**, five cards: (1) Advanced settings per widget: the setup-type settings
   (bus agency/line/stop, Discord client id/secret/redirect, weather coordinates/place name, KNMI
   key and station), editable, under a warning line; (2) Widget details: exe path, build time,
   pid, last 20 log lines; (3) Registered widgets: Register widget…, Remove from list;
   (4) Raw settings: the whole file read-only, secrets as dots, Open in Notepad; (5) Console
   commands: a copyable dockctl cheat sheet.
7. **Settings only**: actions (bus Change stop…, Refresh, Update timetable; Discord Sign in,
   Reconnect; weather Refresh) stay in the right-click menus.
8. **Controls**: switch; segmented buttons (2–3 options) or dropdown (more); slider with the value
   shown (number with a range); text box (text, open numbers); colour box (hex + swatch); secret
   (dots, reveal while held, typing replaces). Values out of range or not an option are refused,
   by the same check in the app and in dockctl `set`.
9. **Applying**: no Save button. Switches, segments, dropdowns and sliders write at once (a slider
   on release; while dragging only the label moves); text on Enter or focus loss. Invalid input
   stays in the box with a red line under the card; nothing is written; Esc restores. A "Saved"
   badge (top right) shows "Saved" for 2 s after each write, "Not saved: <reason>" on failure.
   Edits from elsewhere show within a second (the box being edited is left alone).
10. **Window**: a normal top-level window, one instance; Edit settings… opens it on that widget's
    page (notepad when the app is missing or fails to start). Nav on the left: Widgets, Dock,
    each widget, Developer (bottom). Same look as the panels (`theme`), dark/light live, per-monitor
    DPI, keyboard-only use.
11. **Tray icons** stay per widget. **Step 13** (one tray icon, owned by the launcher) starts only
    once this app has proven robust.
12. **dockctl reaches every setting** (user, 2026-10-12): anything the app can change, dockctl can
    too, through the same library code (F); dockctl is not run by the app.
13. **Testing**: dockctl drives the app (`dockctl settings …`); a smoke check after each build.

## Contracts

### 0. Reads are pure, writes are patches (library + four widgets; first, before anything else)

Today every widget's `Config::load()` reads outside the lock and then writes its whole section
back (`save()`), on start **and on every `settings_changed`** (media `main.rs:418`, bus `:239`;
weather and discord on start); `Style::load` replaces the whole `dock` object. A change made
between that read and that write (by the app, dockctl or another widget) is put back to the old
value. So:
- `settings::fill_missing(section, defaults: &Value)`: inside one `settings::update`, adds only
  the keys missing from the **current** section (nested objects: missing leaves), never touches a
  present key. Every widget's `load` and `Style::load` use it instead of `save()` / replacing
  `dock`; reloads (`settings_changed`) only read. A widget writes a key only when the user
  changes it (its own picker / menu), and then only that key (`settings::set_key(section, key,
  value)`; weather's `save_location` keeps its four keys in one update).
- `FileLock::take` returns an error after its 2 s instead of going on unlocked; `update` fails
  then (callers already handle the error).
- `settings::read` tells "file missing" (`{}`) from "can't read" (error), so a locked or
  half-written file never counts as empty.
- Tests: a write between a widget's read and its fill keeps the new value; fill adds only
  missing leaves; `update` with the lock held elsewhere fails, writes nothing.

### A. Setting descriptions (library, `src\describe.rs`)

```rust
pub struct Setting {                 // owned: built at runtime, also parsed back from JSON
    pub key: String,                 // "label", or "tint_dark.quiet" for a nested dock key
    pub label: String,               // "Taskbar label"
    pub help: String,                // one line, "" for none
    pub group: String,               // card group heading, "" = none
    pub kind: Kind,
    pub advanced: bool,              // true: Developer tab's Advanced card, not the widget page
}
pub enum Kind {
    Switch,
    Number { min: f64, max: f64, step: f64, integer: bool, unit: String },
    Text { placeholder: String, empty_ok: bool },
    Choice { options: Vec<(Value, String)>, other: bool }, // (stored value, label); `other`: a
                                                           // typed string is allowed too
    Color { hash: bool, empty_ok: bool }, // "#RRGGBB" (hash) or "RRGGBB"; empty = widget's default
    Secret,
    Optional(Box<Kind>),                  // null allowed: empty box = null
    Auto { word: String, label: String, inner: Box<Kind> },
        // the value is the string `word` ("system" / "auto") or an `inner` value;
        // drawn as a switch `label` ("Follow Windows") plus the inner control, greyed while on
}
pub fn check(s: &Setting, v: &Value) -> Result<Value, String>;  // canonical value or a short reason
pub fn parse(s: &Setting, text: &str) -> Result<Value, String>; // typed text → value, then check
pub fn to_json(list: &[Setting]) -> Value;                       // for --describe
pub fn from_json(v: &Value) -> Result<Vec<Setting>, String>;     // the app's / dockctl's side
```
Small builder helpers (`Setting::number(key, label, min, max, step)`, `.help(..)`, `.group(..)`,
`.advanced()`) keep the widget lists short.
- `check`: Number: in range (out of range = error, never clamped); `integer` → must be whole and
  is stored as a JSON integer (`u32`/`usize` fields and `hover_open_ms`'s `as_u64` need that);
  else stored as given, no rounding to the step (the step is only the slider's and arrow keys'
  increment, so defaults like 0.333 stay exact). Choice: equal to one option's value, unless
  `other` (then any non-empty string). Color: 6 hex digits with/without `#` per `hash`, case
  kept, empty only if `empty_ok`. Text / Secret: always a JSON string, even when the text looks
  like a number (client ids, stop codes); empty only if `empty_ok` (Secret always may be empty).
  Optional: null or the inner. Auto: the word or the inner.
- Tests: every kind both ways; for every widget and for dock, `check(default) == default` for
  every described key, and every described key exists in `Spec::defaults` / `dock_defaults` and
  the reverse (a missing description is a test failure, so a new setting can't be forgotten).
- Block adds its own rows to every widget: `show_on_taskbar` (Switch, "Show on taskbar") and
  `compact` (Switch, "Compact mode"); "Start with Windows" is not a key (the `startup` list).
- `Spec::settings: Option<fn() -> Vec<Setting>>` and `Spec::description: &'static str` (one line
  for the home page). `Spec::defaults` stays.
- `style::dock_settings() -> Vec<Setting>` describes the `dock` section (below).
- `secret`: a key described as `Secret` is masked everywhere (logs, `get`, `keys`, the app);
  `ctl::is_secret` by name stays as the fallback for keys without a description.

### B. `--describe` (Block)

`<widget>.exe --describe <out.json>` writes `{ "describe": 1, "section", "class", "title",
"description", "log_file", "settings": [...], "defaults": {...} }` (settings incl. Block's two
rows; defaults incl. Block's; `log_file` the absolute log path or null) to `out.json` and exits
0. **Nothing else happens**: no mutex, no window, no COM, no directory created, no settings read
or write, no network. So the first statement of each widget's `main` is
`block::describe_if_asked(describe)`, where `describe: fn() -> block::Describe` is a separate,
side-effect-free function next to the `Spec` (section, class, title, description, settings,
defaults, log path computed **without** creating its folder: bus's `data_dir()` creates
`data\`, media calls `CoInitializeEx` first). `main` then builds its `Spec` from the same
constants as today. Test (per widget, `cargo test`): `describe()` output passes
`describe::from_json`, every default passes `check`, section/class equal the `Spec`'s.
The cache (library, `describe::cached(exe) -> Result<Describe>`): `%APPDATA%\TaskbarDock\
describe\<section>.json` keyed by the canonical exe path + size + mtime + describe version; on a
miss it runs the exe to a unique temp file, 5 s limit (killed and reaped after), validates, and
renames into place. A failure is returned as an error with the reason (shown, never dropped).

### C. The `widgets` registry (library, `src\registry.rs`)

`"widgets": [ { "name": "weather", "exe": "C:\\…\\taskbar-weather.exe", "class":
"TaskbarWeatherLabel" }, … ]` in settings.json, written only through `settings::update` (only
this list). `name` is the describe's `section`. `list()`, `register(exe) -> Result<Entry>`
(canonical path; runs `describe::cached`; refuses the names `dock`, `startup`, `widgets`,
`settings`, and a class already registered under another name; same name → replaces the entry
and, in the same `update`, the exe of its `startup` entry if it has one), `remove(name)`
(registry and `startup` entry in one `update`). A registered widget whose describe fails stays on
the list and is shown with the reason (home row and Registered widgets card), never dropped.
`startup::set_exe(name, exe, on)` beside `startup::set` (the app isn't the widget, so not
`current_exe`).
Seeding: `taskbar-dock-settings.exe --register <exe>` (no window; exits 0/1) registers one widget;
the build job's install note runs it for the four release exes. dockctl resolves a widget name
through the registry first, then its built-in `WIDGETS` table (so it keeps working without one).

### D. Widget descriptions (each widget, next to its `Config`)

Ranges below; where the widget's own code clamps differently, the builder uses the code's range
and says so. Group headings in brackets. Advanced = Developer tab.

- **bus** [Taskbar] `label` Text "Taskbar label"; `dest` Text empty_ok "Destination" (help:
  "Empty: the trip's own destination"); [Line badge] `badge_color` Color no-hash empty_ok,
  `badge_text` Color no-hash empty_ok. Advanced: `agency`, `line`, `stop_code` Text (help:
  "Set by Change stop… in the menu").
- **weather** [Location] `city` Text empty_ok (help: "Empty: found by IP. Coordinates in Developer
  win over this"); [Display] `units` Choice celsius/fahrenheit, `label_position` Choice
  auto/left/right; [Data] `refresh_minutes` Number 5–180 step 5 "min", `model` Choice best_match
  "Best match", knmi_seamless "KNMI HARMONIE", dmi_seamless "DMI HARMONIE", ecmwf_ifs025 "ECMWF
  IFS", icon_seamless "DWD ICON", other: true. Advanced: `latitude` Optional Number −90–90 step
  0.0001, `longitude` Optional Number −180–180 step 0.0001, `location_name` Text empty_ok,
  `knmi_api_key` Secret, `knmi_station` Text empty_ok.
- **media** [Strip] `max_width` Number 60–400 step 1 "px", `fixed_width` Switch; [Panel]
  `click_panel_width` Number 280–600 step 1 "px"; [Playback] `hide_after_pause_s` Number 0–3600
  step 5 "s", `skip_seconds` Number 1–60 step 1 "s"; [Colour] `tint` Choice brand/cover/off,
  `tint_strength` Number 0–3 step 0.05, `live_color` Color hash.
- **discord** [Strip] `max_avatars` Number 1–10 step 1, `fixed_width` Switch. Advanced:
  `client_id` Text empty_ok, `client_secret` Secret, `redirect_uri` Text.

### E. Dock page (`style::dock_settings`)

[Text] `font` Text, `font_size` Number 6–48 step 0.5, `text_size` Auto("system", "Follow
Windows' text size", Number 0.5–3 step 0.05); [Time] `time_format` Choice system "Follow
Windows" / 12h / 24h; [Layout] `gap` 0–64 step 1 px, `panel_margin` 0–64 step 1 px,
`control_radius` 0–16 step 1 px; [Motion] `animations` Choice "system" "Follow Windows" / true
"On" / false "Off", `hover_open_ms` Auto("system", "Follow Windows", Number 0–5000 step 50 ms),
`hover_close_ms` 0–5000 step 50 ms, `fade_ms`, `glide_ms`, `zoom_ms` 0–2000 step 10 ms,
`drag_zoom` 1–2 step 0.05; [Brand colour] `tint_fixed` Auto("auto", "By state", Number 0–1 step
0.01), `tint_dark.quiet|active|hover`, `tint_light.quiet|active|hover` Number 0–1 step 0.01
(greyed while `tint_fixed` is a number). The ranges are `Style::from_json`'s clamps. Reset per
card = `dock_defaults()`'s value. A nested key writes only that leaf.

### F. One path for a setting change; dockctl reaches every setting

dockctl is not a process the app runs (one exe start per click is slow and fragile); the app and
dockctl share one library path instead, so a change from either behaves the same:
`control::apply(section, key, text_or_value) -> Applied` (`src\control.rs`): describe →
`parse`/`check` → `settings::update` of that one key (nested leaf for dock), lock released →
then a ctl `reload` to the running widget (for a dock key: to every running registered widget).
`Applied { value, saved: Result<(), String>, followed: Result<(), String> }`: saved and
followed are reported apart ("Saved", "Saved; the widget hasn't confirmed", "Not saved: …").
`control::reset` (default from the description) the same way.
- `reload` (new ctl request): Block re-reads the section and `dock` **regardless of mtime**
  (today `follow_settings` skips when the mtime matches what Block last saw, which a menu
  switch right after an app edit would cause), calls `settings_changed` / `look_changed`,
  redraws, then replies.
- Where it runs (the private `%APPDATA%` of Claude sessions): dockctl itself never touches
  settings files when the target widget is running: it sends the request (`set`, `unset`, …)
  and the **widget** calls `control::apply` (today's path, so it is the real file). Only for a
  stopped widget (or `--dock` with no widget running, or `widgets …` / `startup …`) does dockctl
  call the library in its own process, and it refuses that when it runs with a package identity
  (`GetCurrentPackageFullName` succeeds = inside the Claude app), saying to start the widget or
  run it through a scheduled task. The settings app always calls the library itself (it never
  runs inside a package: Block and the user start it).
- `set`: parses the text with `describe::parse` (JSON first as today), refuses a failed `check`
  with `{"ok":false,"kind":"usage"}` and exit 2. Without a description: as today.
- Work without the widget running (they read and write the file through `control`; today
  `set`/`get`/`keys`/`unset` need the widget): `set`, `get`, `keys`, `unset`, `--dock` too.
  Defaults and descriptions come from the describe cache (B).
- Nested dock keys (`tint_dark.quiet`) in `set/get/unset --dock`.
- New dockctl commands for what had no command yet:
  - `dockctl <w> startup on|off|get`: Start with Windows (`startup::set_exe` with the
    registry's exe).
  - `dockctl <w> status`: the app's status (running / stopped / not responding / keeps
    crashing, pid), also for a stopped widget.
  - `dockctl <w> start|restart`: as the app's buttons (G). Refused with a
    package identity, like the file commands above (a widget started from a Claude session
    would use the private `%APPDATA%`).
  - `dockctl widgets list|register <exe>|remove <name>`: the registry (C).
  - `dockctl <w> describe`: the widget's descriptions (labels, kinds, ranges).
  - `reload` (ctl request, sent by `control::apply`; also usable by hand).

### G. Process control (library `src\control.rs`; the launcher only records give-ups)

Step 12 keeps the launcher's watching as it is; per-widget watching (widgets started from the
app restarted after a crash) belongs to step 13, which reworks the launcher anyway.
- **Status** (`control::status_all(&[Entry]) -> Vec<Status>`): one process snapshot per round
  (full image paths, compared to the registry's canonical exe path, not the base name); for each
  widget whose process runs, find `<class>Main` owned by that pid and send the new quiet ctl
  `ping` (answered by Block with `{pid, build}`, **not logged**, unlike `state`), all widgets at
  once on the probing thread's own message window, one common 1.5 s deadline, replies matched by
  request id. Result per widget: Running (answered), Not responding (process there, no window or
  no answer), Stopped, Keeps crashing (`watchdog.json` has a give-up for it and no process runs:
  a live process wins over an old record). A round never overlaps the next: the next starts 2 s
  after the last ended; stale rounds are dropped by a round number.
- **The launcher's one change**: when it gives up on a widget it also writes `{ "<name>":
  { "gave_up": <unix s>, "exit_code": n } }` into `%APPDATA%\TaskbarDock\watchdog.json` (read,
  change, atomic write under the named mutex `Local\TaskbarDock.Watchdog`), and it removes that
  name's record when it starts the widget. Nothing else in the launcher changes.
- **Start / Quit / Restart** (`control::start(name)`, `quit(name)`, `restart(name)`):
  - Start runs the registered exe directly (`CreateProcessW`, working folder = the exe's
    folder, as the launcher does), then removes the name's `watchdog.json` record. A widget
    started this way is not watched for crashes until the next sign-in (step 13 changes that).
  - Quit sends ctl `quit`.
  - Restart = Quit, wait for the process to end (≤ 5 s, else "didn't quit", nothing started),
    then Start.
  - Success is reported only when the widget answers `ping` (≤ 10 s), else "started, not
    answering".
- Tests (pure): status from snapshot + replies + records (precedence above); the watchdog record
  add / clear functions.

### H. The app (`src\bin\taskbar-dock-settings.rs`, logic in `src\settings_app\`)

- Pure, unit-tested modules: `model.rs` (pages, rows, values, draft / pending / error state per
  row, apply results by sequence number), `layout.rs` (row rects, nav, scrolling, hit testing,
  all in 100 % units times the scale), `keys.rs` (which control a key goes to). Window code in
  `win.rs`, drawing in `paint.rs`.
- Args: `--widget <name>`, `--dock`, `--developer`, none = Widgets page; `--register <exe>`
  (no window; registers, prints the result, exits 0/1).
- **One instance**: mutex `Local\TaskbarDock.SettingsApp` (not `TaskbarDock.Settings`: that is
  the settings-file lock). The first instance creates its visible window `TaskbarDockSettings`
  before anything slow. A second launch: finds that window (retrying every 100 ms for up to
  3 s while the first starts), sends `WM_COPYDATA` with the page and waits for its reply
  (`SendMessageTimeout`, 2 s); the first shows, restores and comes to front
  (`AllowSetForegroundWindow(ASFW_ANY)` by the second before sending), replies, the second exits
  0. No window found or no reply: the second exits 1.
- **Window**: `WS_OVERLAPPEDWINDOW`, initial client 880×640 at 100 % times the monitor's scale,
  minimum 640×420 (scaled, `WM_GETMINMAXINFO`), position/size not saved. DWM immersive dark mode
  from `Look::light`, Mica (`DWMSBT_MAINWINDOW`) where available. Per-monitor DPI v2 (manifest);
  `WM_DPICHANGED`: take the suggested rect, rebuild fonts and every scaled resource, re-layout.
  `WM_SETTINGCHANGE` → re-read `Look` (dark/light, Windows text size) and redraw. Double-buffered
  GDI paint; colours, fonts, sizes from `theme` (add named app sizes there); `tests\theme_lint.rs`
  gains the app's folder in its roots.
- **Cards**: label + help (wrapped, height measured) on the left and the control on the right;
  sliders and text boxes wider than half the card go on a full-width line under the help text
  (as in TaskbarWidgets' dialog). An error line under the card, wrapped. Reset button only when
  the value differs from its default.
- **Text editing** (extend `text_edit`, single line): caret, Left/Right, Home/End, Shift+arrows
  / Shift+Home/End selection, Ctrl+A, Ctrl+C/X/V, Backspace/Delete, Ctrl+Backspace, a click sets
  the caret (drag-select not needed). No undo.
- **Keyboard precedence**: while a text box edits, every key goes to it (Home/End too); Enter
  commits, Esc cancels the edit. An open dropdown takes arrows/Enter/Esc. Otherwise: Tab /
  Shift+Tab through controls (nav, then the page's controls in order; the focused control is
  scrolled into view), arrows in the nav and segments, Space flips a switch, Enter/F2 starts
  editing, Left/Right a slider by one step, PgUp/PgDn/Home/End scroll, Esc closes the window.
  A focus ring on the focused control. Mouse wheel scrolls.
- **Writes**: through `control::apply` / `control::reset` only, on one writer thread in order
  (a queue: a later change to the same key replaces a queued one). Each result posts back with
  its sequence number; an older result never overwrites a newer row state or badge. A slider
  writes on release (while dragging only the value label moves; capture lost = back to the saved
  value). Badge: "Saved" (2 s), "Saved; <widget> hasn't confirmed", "Not saved: <reason>".
- **File watch**: poll `settings::mtime()` every second; on change re-read and refresh every row
  that has no draft, no pending write and no shown error. An invalid file: a banner
  "settings.json has an error; fix it in Notepad" with Open in Notepad, all controls disabled.
- **Secrets**: never logged, dots on screen, shown only while the mouse is held on the eye button
  (released or capture lost = dots again); typing replaces the whole value.
- **Developer tab**: Advanced settings (the `advanced` rows of each widget, under the warning
  "These change how the widget connects and what it shows; wrong values can stop it working");
  Widget details (exe, build time from `ping` or the exe's mtime, pid, last 20 lines of the
  describe's `log_file`, read directly, so it works while stopped); Registered widgets (Register
  widget… with the standard Open dialog filtered to `.exe`, Remove from list, broken entries with
  their reason); Raw settings (the whole file pretty-printed, `ctl::mask` + described secrets
  masked, read-only, scrollable, Open in Notepad); Console commands (a fixed list of dockctl
  lines with a Copy button each).
- **Log**: `%APPDATA%\TaskbarDock\settings-app.log` (writes with masked values, failures,
  start/quit/restart); when it passes 256 KB it is cut to its last 128 KB.

### I. dockctl drives the app

`dockctl settings <command>`: the app's hidden window `TaskbarDockSettingsMain` answers protocol
v2 like Block's `remote.rs` (copy the request before returning from `WM_COPYDATA`).
- `state`: page, window rect, scale, focused control, banner, badge text, and the page's rows
  `{ id: "<section>.<key>", label, kind, value (masked), default, draft, error, enabled }`; on
  Widgets the widget rows `{ name, title, status, pid }`; on Developer each card's content.
- `page <widgets|dock|developer|<name>>`; `row <id> set <text>` (the UI's edit path: as typing +
  Enter); `row <id> reset`; `row <id> toggle`; `click <id>` (start:<name>, quit:<name>,
  restart:<name>, remove:<name>, notepad); `key <name>` (tab, shift+tab, enter, esc, space, up,
  down, left, right, home, end); `type "<text>"`; `quit`. Replies after the change is applied
  and painted (writes: after the writer's result).
- Smoke (`scripts\settings_smoke.ps1`, the app started through a scheduled task like dev.ps1's
  widgets): start the app, `state`, visit every page; for each widget, the **union** of the rows
  on its page and its Developer advanced rows equals its described keys, and each value equals
  `dockctl <w> get` (Start with Windows is not a key and is left out); `quit`; the process is
  gone. Read-only.

### J. Edit settings…

Block's `Route::Settings` runs `taskbar-dock-settings.exe --widget <section>` from
`concat!(env!("CARGO_MANIFEST_DIR"), "\\taskbar-dock-settings.exe")` (the installed copy, like
the launcher) with `CreateProcessW` on a worker thread, and waits up to 5 s for it: exit code 0
(the second instance handed over) or its `TaskbarDockSettings` window appearing (the first
instance) = done; a missing file, a failed start, exit code ≠ 0, or nothing within 5 s → notepad
as today.

## Jobs (Codex Luna, one at a time: each builds on the last)

1. **Safe settings** (0): `fill_missing`, `set_key`, the lock and read errors, the four widgets'
   `load`/save paths and `Style::load` changed to it, the tests in 0. Small, first, on its own.
2. **Descriptions, registry, shared path, process control** (A–G): `describe.rs`,
   `Spec::settings` / `description`, `Describe` + `describe_if_asked` as each widget's first
   statement, the four widgets' and dock's descriptions, the describe cache, `registry.rs`,
   `startup::set_exe`, `control::apply` / `reset`, ctl `reload` and `ping`, dockctl
   `set/get/keys/unset` (stopped widgets included, the package-identity refusal), nested dock
   keys, `startup`, `widgets`, `describe`, `status`, `start`, `restart` commands, dockctl
   resolving through the registry; status and start/quit/restart, the launcher's give-up record.
   Done: `cargo test` green in the library and every widget, with the tests of A, B and G.
3. **The window and the widget pages** (H without Dock and Developer, I): window, nav, Widgets
   page, widget pages with every control kind, text editing, keyboard, writes, file watch, the
   app's ctl window and `dockctl settings`. Done: builds, model/layout/keys tests; then I run it
   and drive it with dockctl before job 4.
4. **Dock and Developer pages, hook-up** (H rest, J): the Dock page, the Developer tab, Edit
   settings…, the smoke script, README / BLOCK.md / CLAUDE.md current-facts updates. Done:
   builds, tests.

After each job: I read the diff, build with each widget's dev.ps1 (smoke must pass), install the
exes (widgets, dockctl, launcher, the app), check live; one Sol review over the whole step at
the end.

## Build notes (gaps filled while building)

- Job 2 runs as two builds: 2a (A–F) then 2b (G, dockctl `status` / `start` / `restart`).
- Describe cache file name: `<exe stem>-<FNV-1a 64 of the lower-cased canonical path>.json`
  (the section isn't known before the exe has run once); the key inside is as in B.
- dockctl refuses with a package identity every command it would run in its own process (file
  commands for a stopped widget, `widgets …`, `startup …` and `describe` for a stopped widget):
  from the Claude app they would all see the private copy.
- "Runs with a package identity" (F, G) is tested as `settings::redirected()`, not
  `GetCurrentPackageFullName`: shells started from the Claude app have no package identity
  (APPMODEL_ERROR_NO_PACKAGE) yet their `%APPDATA%` is still redirected. `redirected()` opens
  settings.json and compares `GetFinalPathNameByHandleW` with the path asked for.
- Registered exe paths (and the `startup` entries `register` updates) are stored without the
  `\\?\` prefix `fs::canonicalize` adds (`describe::canonical_exe`).
- The app's code is the bin's own modules (`src\settings_app\` via `#[path]`), not a library
  module, so the widgets don't compile it; its manifest (per-monitor DPI v2) is embedded into
  that bin only (`build.rs`, `rustc-link-arg-bin`).
- Second-launch hand-over: `WM_COPYDATA` with `dwData` 0x444B_5350 and the page name as UTF-8.
- A choice with `other` (weather `model`) is a dropdown ending in "Other…", which turns it into
  a text box. An Auto switch turned off writes the inner default (the described default when it
  isn't the word, else the inner kind's minimum or first option).
- A number slider's value can also be typed (Enter / F2 / double click on the value).
- Between jobs 3 and 4 the Dock and Developer entries show "This page comes in the next build."
- Remove from list asks no confirmation (nothing is deleted; Register widget… puts it back).
  Register widget… has no `dockctl settings` command (the file dialog needs a person);
  `dockctl widgets register` covers it.
- dockctl `status` / `start` / `restart` for a widget not in the registry (always so from a Claude
  session, which sees the private copy) use the full path of the running process with the
  built-in table's file name (`control::running_exe`), else dockctl's folder.
- Status colours: Running `Status.ok`, Stopped `Colors.dim` (theme has no grey status colour),
  Not responding `Status.warn`, Keeps crashing `Status.danger`.

## Acceptance checks (live, after job 4)

- Edit settings… on each widget opens the app on its page; a second Edit settings… on another
  widget switches the same window's page.
- Home page shows all four with the right status; Quit, Start, Restart of weather work and the
  status follows within 2 s.
- On each widget page + its Developer advanced rows: together they match `dockctl <w> keys`;
  flipping Compact in the app → `dockctl <w> state` shows it within a second, and back; a bad
  number shows the red line and writes nothing.
- Dock page: change `time_format` and put it back; the bus/weather times follow.
- Developer: raw view masks `client_secret` and `knmi_api_key`; widget details show the log tail.
- Dark/light switch and moving the window between monitors of different scale redraw correctly.
- The user signs out and in once: every widget on `startup` starts as before (the launcher's
  record change didn't break sign-in).
- Every real setting changed in a check is noted first and put back.
