# Step 11: bus stop picker as a Block panel; dockctl configures widgets

Decided with the user 2026-10-11 (grilling, three rounds). Two builds, one after the other
(both touch `block/remote.rs` and `ctl.rs`): **11a** the picker panel and the panel commands,
then **11b** the dockctl additions. Each is reviewed and checked live with dockctl before the
next; one overall Codex review over both before committing.

## 11a: the picker as a Block panel

**Window.** A second panel kind in Block (`PanelKind::Picker` or similar, generic: nothing
bus-specific in the library). Same shell as the main panel: drop shadow, round corners, fade,
theme background, placement rule (above the strip; above the tray icon when the strip is hidden).
Opening it closes any other panel (one panel at a time, as now). Size: `size::PANEL_WIDE` (360 px
at 100%) wide, fixed height of about 10 list rows, the same in both steps (no jump).

**Opens from** the right-click row "Change stop…" (strip menu and tray menu).

**Focus loss.** It closes like the other panels (focus loss, Esc in the stop step, click on the
strip) but keeps its state: the search text, the chosen stop, the highlighted row, and a line
search that keeps running in its thread. Reopening "Change stop…" returns to where it was, with
results if they arrived meanwhile. The stop list (large) is freed 60 s after the picker closed,
unless a search is still running (then 60 s after it finishes); selecting a line frees it at once.

**Layout, top to bottom.**
- Header: title "Change stop"; a smaller caption under it with the step: "Find your stop" /
  "Pick line and direction". In the line step a back chevron at the header's left.
- Search box (stop step), drawn by Block in the Windows 11 text-box style: rounded, theme colours,
  accent underline while focused, caret. Editing: typing, Backspace, Delete, Left/Right,
  Home/End, Ctrl+A, Ctrl+C/V/X, Ctrl+Backspace. No mouse selection, no IME.
- The list, scrolling. Stop rows: the name, the stop code as a smaller grey caption. Line rows:
  the line badge in its own colour (bus's `line_colors.txt` / timetable colour, as the strip's
  badge), then "next stop X · headsign" (or "trip ends here").
- Bottom: the status line on the left (the current texts, unchanged); buttons **Cancel** and
  **Select** (accent) on the right.

**Keys and mouse.** Typing goes to the search box. Up/Down, PageUp/PageDown, Home/End move the
highlight; Home/End move the caret in the search box, Ctrl+Home/Ctrl+End the list highlight. Enter = Select. Esc: in the line step back to the
stop list, in the stop step close. Alt+Left = back. Mouse wheel scrolls the list; a click
highlights a row; a double-click or Select chooses it; Cancel closes.

**Flow unchanged** (keep it simple): the two steps, the 300-row cap, the status texts, the
background threads (`data::all_stops`, `data::lines_at_stop`), the config built on Select.
No grouping or other new features. Only the look changes, plus the line badges.

**Where the code goes.** Library (generic, per CLAUDE.md "Where a rule lives: in Block or theme"):
the second panel kind; panel keyboard, text and wheel events to the Content (`panel_key`,
`panel_char`, `panel_wheel` or similar, with defaults); draw helpers for a text box, a scrolling
list with a highlight, and a button (accent and plain) in `draw` / `theme`; theme tokens for them.
Bus (`buswidget/rust/src/picker.rs`): the picker's own state, modes, threads, rows and the config.

**Removed:** the Win32 picker window, `Spec::dialog` and Block's `IsDialogMessageW` loop,
`theme::font_override::BUS_PICKER`. Docs follow (README, BLOCK.md, bus CLAUDE.md, theme lint).

**dockctl (panel commands, generic):**
- `panel open picker` (the second panel kind by name; `panel open` stays the main panel)
- `panel key <name>` (enter, esc, up, down, pgup, pgdn, home, end, back, tab, ctrl+a, …) and
  `panel type "<text>"`, delivered through the same paths as real keys
- `panel state`: kind, open, rect, and the Content's own report of the panel (new optional
  `Content::panel_state`): for the picker its step, search text, row count, highlighted row,
  first rows shown, status text, whether a search is running.

## 11b: dockctl configures widgets

**Reply protocol.** dockctl sends the command, then waits for the widget's reply message for up
to `--timeout` seconds (default 5, `do` commands 120). The widget runs slow work (timetable,
network) on a worker thread and replies when done; its message loop never blocks. Every command
keeps today's shape: one call, one JSON reply.

**Shared settings:** `set --dock <key> <value>`, `get --dock [key]` for the `dock` section.

**`keys`:** every setting of the widget's section with its current and default value (new
`Spec::defaults: Option<fn() -> Value>`, from the widget's `Config::default`). `keys --dock` for
the dock section. No per-key descriptions (later, settings app step 12).

**`unset <key>`** (and `unset --dock <key>`): the key goes back to its default.

**Widget commands:** optional `Content::command(name, args) -> reply` (sync, or replying later
from a thread), run as `dockctl <w> do <name> [args]`. `do help` lists the widget's commands with
a one-line usage each. Bad arguments: `{"ok":false,"error":…}` with the usage, exit code 2.
- **bus:** `now` (line, stop, next departures with live/scheduled); `stops <query>`;
  `lines <stop_code>`; `choose <stop_code> <line> [agency]` (label built by the same code as the
  picker's Select).
- **weather:** `now` (place, temperature, conditions, last update); `places <query>` (up to 5
  Open-Meteo geocoding matches); `place <n | "lat,lon" | auto>` (sets `city` / `latitude` /
  `longitude` / `location_name`, refreshes; `auto` = by IP).
- **media:** `now` (app, title, artist, position/length, playing, volume); `sessions`; `play`,
  `pause`, `toggle`, `next`, `prev`, `skip <±s>`, `seek <s>`, `volume <0-100>`, `mute on|off`;
  each with optional `--app <name>` (default: the session shown).
- **discord:** `now` (status, channel, members with speaking / muted); `mute on|off`,
  `deafen on|off` (set, not toggle); `leave` (leave the call). Sign in / Reconnect stay
  `menu click`.

README's dockctl table gets the new commands and a `do` section per widget.

## Contracts (after Codex's plan critique, 2026-10-11, `codex\runs\step11-plan.md.answer.md`)

**Panel kinds (11a).** Block keeps one panel HWND and one fade, plus the active kind; every shell
callback (`panel_size`, `panel_draw`, `panel_mouse`, `panel_shown`, keys) passes that kind
instead of the hard-coded `Main`. Widgets return `None` / ignore kinds they don't have (all four
are checked: today they ignore the argument), so `panel open picker` on weather is an error, not
its main panel. Opening the picker is its own transition: pinned, hover timers cancelled, any
open panel or menu (this widget's and the others', tray and floating ones included) closed first,
exactly one `panel_shown(.., false)` / `(.., true)` pair. The strip's click toggle and its
focus-loss guard (panel.rs) stay as they are. Anchoring uses the strip's effective visibility
(a strip hidden for room counts as none: open above the tray icon). Height shrinks to the work
area if it doesn't fit (fewer list rows), never off-screen.

**Keyboard (11a).** The picker takes focus when it opens (`SetFocus` after activation, outside the
Content borrow); the hover-opened main panel stays non-activating. Block dispatches normalised
events to the Content: key + modifiers (`WM_KEYDOWN` and `WM_SYSKEYDOWN`, for Alt+Left), text
(`WM_CHAR`, surrogate pairs joined, control characters from Enter / Backspace / Tab / Ctrl+letter
dropped), wheel. Esc goes to the Content first; Block closes only if the Content doesn't take it.
The message loop keeps `TranslateMessage` / `DispatchMessage` (only `IsDialogMessageW` goes).
`CS_DBLCLKS` on the panel class and a `PanelMouse::DoubleClick`. dockctl's `panel key` / `type`
call the same dispatch functions with explicit modifiers (not the physical keyboard state). No
Tab order: Tab does nothing. No Shift-selection.

**Text box (11a).** A generic edit model in the library (text, caret, select-all, the editing
keys in 11a, clipboard), with unit tests, plus its draw helper; the caret blinks at the system
caret rate and the text scrolls to keep the caret visible.

**Picker state (11a).** It lives in bus's `App`, not in a window: results are posted to Block's
main window with a message number of its own (bus's `WM_DATA` is already `WM_APP + 2`), tagged
with a job generation; a result whose generation is stale (Back, another stop, reopened and
searched again) is dropped. The chosen stop is kept as code + name, not an index into the stop
list. Eviction: 60 s after the picker closed with no job running, the stop list is freed; the
step, search text and chosen stop stay, and reopening reloads the list in the background and
refilters (status "Loading stops…" meanwhile). Line colours: `LineOpt` gets the route colour
from the timetable, resolved by the same helper as the strip's badge.

**Reply protocol (11b).** Requests carry a protocol version and a request id; replies echo the id
and dockctl accepts only a reply with its id from the window it asked. dockctl pumps messages
until the reply or the deadline (`--timeout`, default 5 s, `do` 120 s), accepting a reply that
arrives during the send or after it. The widget copies the request out of `WM_COPYDATA` before
returning. A late reply to a dockctl that's gone fails to deliver and is dropped (no crash, no
block). Content's command API: `Content::command(name, args, ctx) -> Done(Value) |
Pending(ReplyToken)`; a worker finishes by posting to the main window, and the UI thread sends the
reply (workers never touch Block state). Mutations are applied on the UI thread before the reply.
Old commands keep replying inline; a new dockctl talking to an old widget gets a prompt "unknown
command" error. A mutation is never retried after a timeout. Exit codes: a remote error tagged
`"kind": "usage"` gives exit code 2.

**Settings (11b).** `keys` shows the effective defaults: the widget's `Spec::defaults` merged
with Block's own keys (`compact` false, `show_on_taskbar` true); `--dock` uses the dock section's
JSON form (`"system"`, `"auto"`), exposed from `style.rs`. Secrets are masked by setting name in
`current` and `default`. **The dockctl command log masks secrets too** (today `remote.rs:43` logs
`set` values in plain text). `unset` writes the default value (the loaders reinsert keys anyway);
an unknown key is an error listing the known ones; a widget without `Spec::defaults` says so.
`set --dock` / `unset --dock` re-read the look and redraw before replying. Bus gets a real
`Default` for its config.

**Widget commands (11b).**
- Bus: commands never download: with no timetable on disk they say so. Timetable reads for
  commands and the picker go through one queue (no two scans at once, no competing download).
  `choose` builds and saves the config through the same code as the picker's Select and reports
  a save failure (today `set_config` applies even when saving fails).
- Weather: `places` asks Open-Meteo for 5 results; the four location fields are written in one
  `settings::update`; a fetch for an old location is dropped; the reply says "saved" and, once
  the fetch ends, the weather or the fetch error.
- Media: explicit play / pause (not a toggle of a cached state); `sessions` is one entry per app
  (as the snapshots are), each with a stable app id; `--app` matches the id or a case-insensitive
  name prefix, an ambiguous match is an error listing them; volume is read on demand, not from the
  panel's cache; a missing session or a refused operation is an error; unknown position / length
  are null.
- Discord: commands wait for Discord's answer (matched by RPC nonce) and fail on disconnect,
  error or 10 s; `mute on` when already muted succeeds at once.

**Picker memory (Q16).** Closing by Cancel, Esc in the stop step or a click outside keeps the
search text, step and chosen stop. A successful Select clears it all: the next "Change stop…"
starts empty. Back while lines are loading cancels that job (its result is dropped on arrival).

**Bus `choose` with several directions (Q17).** It fails and lists the candidates, numbered, each
with "next stop · headsign"; `--to <number | text>` picks one (text matches headsign or next stop,
case-insensitive; still ambiguous = the same error). It never guesses.

**Weather `place <n>` (Q18).** `n` is from the last `places` reply (numbered from 1), kept in
memory 10 minutes; after that `place <n>` fails with "run places again". `place "lat,lon"` and
`place auto` always work.

**Network (user, 2026-10-11: be careful with the timetable download and the live times).**
- Picker reads and bus commands open the timetable read-only: no zip on disk = the error "timetable
  not downloaded yet (it downloads once a day; `menu click \"Update timetable\"` starts it)" (Q19).
  `open_timetable`'s download stays only on the worker's daily/forced path.
- Nothing new polls OVapi: the picker and `do` commands never call `fetch_live`; `do now` reports the
  live data the worker already has (with its age). The worker's poll rate and 429 back-offs are
  unchanged.
- Builders (Codex) run `cargo build` / `cargo test` only: never `dev.ps1`, never a widget, no
  network calls in tests.
- Live checks: Select is tested by choosing the current stop and line again (UOV line 3, stop
  59150850, label "Rhonedreef → Utrecht CS"), so the config is unchanged and nothing is fetched;
  keep restarts few; `bus do choose` is tested the same way.

**Acceptance checks.** 11a: real typing and focus loss by hand (dockctl can't prove physical
focus), Esc in both steps, close and reopen during a line search, Back then a stale result,
eviction and reload, `panel open picker` on the other widgets fails, main panels unchanged
(hover, click, pin). 11b: inline and slow replies, `all` with one slow widget, dockctl killed
before a late reply, concurrent `set`s, Discord disconnected, media app closed mid-command.
