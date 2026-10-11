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
