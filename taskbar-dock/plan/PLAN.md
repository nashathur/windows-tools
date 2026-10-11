# Plan: Block (bottom-up strip) + one design code

Status: steps 0-10 done (2026-10-11): all four widgets run on Block; animations match the
taskbar's; media and discord's width follows their content; the old wiring is gone and the docs
say so. Open: 11 (bus picker as a panel + dockctl configures widgets, decided: `step11.md`), 12 (settings app, idea). Renumbered 2026-10-11 (old 7→8, 8→10, 9→11,
10→12, 11→9, 12→7); commits before that use the old numbers.
Spec gaps found in step 0 (not yet in section 1; decide before step 2): weather's "Show on
taskbar" switch + tray icon (strip off, widget keeps running); media floats too (attach
failure / no taskbar for 10 s / `--float`, re-attach later), so `float_fallback` serves bus
and media; floating strips hide while a full-screen app is in front.
Inputs: `inventory-label.md`, `inventory-placement.md` (local-llm behaviour lists of each
widget's wiring file, used for the parity checklist below; spot-checked, may miss items).

## The idea

Today the library gives parts (`strip::Motion`, `dock::slot`/`fit`, `panel::PanelBehaviour`,
`menu_panel`) and each widget wires them up itself: weather `label.rs` 519 lines, discord
`label.rs` 579, media `placement.rs` 253, bus `placement.rs`, plus timers and wndproc code in
each `main.rs`. Weather and discord's wiring is ~60% the same code.

New: the library owns a **Block**, the strip window plus all its behaviour. A widget supplies
a **Spec** (adjustable parameters) and a **Content** (what to draw, what clicks do, its panels).

```
Block (library)                         Widget
  window, wndproc, timers                 Spec      : parameters
  press/drag/zoom/glide/resize            Content   : size, draw, hit, menu rows,
  dock slot, order, drop, fit/hide                      panels' drawing, data timers
  WinEvent hooks, Explorer restart
  z-order raise, floating fallback
  hover/click/pin panels, fade, place
  right-click panel standard rows
  cover feel, button_box hit-tests
  theme (design code)
```

## 1. Adjustable parameters (Spec)

Every special case found in the inventories becomes a Spec field with a default, so no
widget needs its own wiring. Defaults = what weather does today.

| Field | Default | Who needs a non-default |
|---|---|---|
| `class`, `section`, `title` | required | all |
| `side` | `Right` (against tray) | weather has a left/right option |
| `float_fallback` | on: float + remember position when the taskbar can't be joined | bus does this today (`pos.txt`); becomes standard for all |
| `visible` | `Always` | discord (only in a call / needs attention), media (nothing playing) → `ContentDecides` (`Content::visible()`) |
| `width` | measured by `Content::size()` → `dock::Size { full, min, stretch }` | media: `max_width` from its settings |
| `when_crowded` | narrow → compact → hide | per widget if one should never narrow |
| `compact` | supported | all (rule) |
| `draggable` | true | |
| `click` | `TogglePanel` | per widget |
| `hover_panel` | true | |
| `cover` | none | media, discord: hit box + `window::raise` target |
| `frame_ms` | none | animations the content runs: discord pulse ring, marquee, media progress |
| `data_timers` | none | refresh loops (weather fetch, bus departures) |

Escape hatches, for things that don't fit (kept small, each used by name):
- `Content::on_message(msg, wp, lp) -> Option<isize>`: any window message the Block doesn't
  handle (bus picker opening, worker-thread posts).
- `Spec::dialog: Option<fn() -> isize>`: the widget's own dialog-like window open right now (bus
  "Change stop…" picker; 0: none). Block only routes Tab / Enter / Esc to it (IsDialogMessageW);
  the widget opens it (`ctx.after`) and closes it itself (replaces the planned `Content::own_window()`).

If a widget needs a new behaviour later, the rule is: add a Spec field (with a default)
to Block, not wiring in the widget.

## 2. Panels: widget draws, Block frames, design code decides the look

Split of who does what:
- **Block (panel shell):** creates the panel window (DWM round corners, theme background),
  size from `Content::panel_size(kind)`, placement (`panel::place`, `panel_margin`), fade,
  hover/click/pin, one panel at a time, closing on drag.
- **Widget:** `Content::panel_draw(kind, canvas, &Theme)` and `panel_mouse(kind, event)`.
- **Design code (`theme` module, new):** every colour, font, size and spacing comes from it.

Theme tokens, measured in step 0 (full tables with sources and every deviation:
`tokens.md`; light / dark):

| Group | Tokens and values |
|---|---|
| Colours | `bg` F3F3F3/202020, `fg` 1A1A1A/FFFFFF, `dim` 5F5F5F/A8A8A8, `faint` 8A8A8A/808080, `line` E0E0E0/3A3A3A, `hover` E4E4E4/333333, `button` E6E6E6/2D2D2D, `button_hover` DADADA/383838, `track` D6D6D6/454545, `placeholder` DEDEDE/383838, `chip` D0D0D0/3C3C3C. Slider/switch `fill` = `fg`, `knob` = `dim` |
| Strip | `strip_fg` 1A1A1A/FFFFFF, `strip_dim` 5A5A5A/C8C8C8 (`Look::text_rgb`), `strip_disabled` 38% coverage |
| Status | `accent` = Windows accent (fallback 0067C0/60CDFF), `danger` C42B1C/FF99A4, `danger_fill` C43135 (hover D83C3E), `danger_soft` F6D6D7/4A2426, `ok` 0F7B0F/6CCB5F, `warn` 9D5D00/FCE100 |
| Brand | `discord.blurple` 5865F2, `discord.green` 23A55A, `discord.red` F23F43, `bus.badge_fallback` 00A8A8, `media.live` FF4E45 (setting), `weather.temp_bar` E8892B/FFB454 |
| Type | family `look.font` for all text, `icon` Segoe Fluent Icons, `emoji` Segoe UI Emoji. `caption` 12/400, `caption_strong` 12/600, `row` 13/400, `body` 14/400, `body_large` 15/400, `title_small` 14/600, `title` 15/600, `title_large` 18/600 (media only), `display` 46/300, `footnote` 11/400; `icon` 16, `icon_large` 20; `emoji` 20 / 42 |
| Spacing | `xs` 4, `tight` 6, `s` 8, `m` 12, `l` 16, `xl` 20; `panel_padding` 16 (main) / 12 (menu); `row_height` 32, `row_height_large` 40; `section_gap` 8 |
| Sizes | panel widths `narrow` 280, `standard` 320, `wide` 360; `icon_box` 16, `button_box` 24, `panel_button` 36, covers 44 / 96 |
| Radii, motion | already shared (`control_radius` 4, `card_radius`, `concentric`, `PANEL_RADIUS` 8, DWM round; `Style` gap/margin/fade/glide/zoom) |

Today each widget has its own copy of the palette and font list (weather/discord/media
`render.rs`, media `panel_view.rs`, and the library's `menu_panel.rs`). They agree on
`bg`/`fg`/`dim`; they differ on separators (weather/discord `line` E0/3A, media + menu_panel
D6/45), hover (discord DA/38 on a button, media + menu_panel E4/33 on a row), the weather
range-bar track (DA/3A) and font family (weather/discord panels and bus picker hard-code
"Segoe UI"; everything else uses `look.font` = "Segoe UI Variable Text"). Bus popup is always
dark (`POP_BG` 2B2B2B, text white/9A9A9A, live 7DDC7D, cancelled FF6B6B) and scales its fonts
by `font_size / 12`; bus picker is a plain white Win32 dialog. Visible unifications beyond the
bus popup are open questions in `tokens.md`; until answered, step 1 gives those widgets a
named override so nothing else changes.

Enforcement: a check in `scripts\dev.ps1` (and a library test over the widget folders) that
fails on raw `rgb(0x..)`, font names, or literal font sizes in widget source outside
`theme::brand` entries. Then "follow the design code" is checked by the build, not by memory.

## 3. Ownership: "requests, applied afterwards" (recommended)

Problem: Block holds the Content and calls it (`draw`, `click`, ...). If that call makes
Windows send a message straight back to Block (SetWindowPos, ShowWindow, DestroyWindow do),
Block's state is still borrowed and Rust panics (`RefCell already borrowed`).

Options considered:
- **A. Requests (recommended).** Content never touches windows. Every callback gets a
  `&mut Ctx` and only records what it wants: `ctx.redraw()`, `ctx.resize()`,
  `ctx.open_panel(kind)`, `ctx.close_panels()`, `ctx.every(id, ms)`, `ctx.save_settings()`,
  `ctx.quit()`. Block finishes the call, releases the borrow, then carries the requests out.
  Same rule `strip::Motion` already follows. Easy to test: a test calls the Content and checks
  the request list, no windows needed.
- B. Post everything as window messages (`PostMessage` to self). Safe but harder to follow
  and to test, and adds a message-loop round trip (visible lag on drag/zoom).
- C. Careful borrowing (`try_borrow`, drop guards). Works until someone forgets once; the
  panic shows up only at runtime in a rare order of events.

A is the choice. Two supporting rules:
1. Block keeps its state in one `thread_local` `RefCell`, borrowed only inside Block, never
   across a Win32 call that can send messages. Requests are applied after the borrow ends.
2. Data from worker threads (bus fetch, discord RPC, media session) arrives as a posted
   message → `Content::on_data(...)` → requests, as today.

## 4. Steps (one widget at a time, everything builds after every step)

Each step: commit the last good state first (ask before committing), keep all widgets building, run
tests, restart via `dev.ps1`. One or two steps per session. A done step shrinks to one line here;
its full notes go to `done.md`.

| # | Step | Behaviour change |
|---|---|---|
| 0 | **Done 2026-10-10.** Prep: parity checklist (`parity.md`), today's measurements, token table (`tokens.md`) | none |
| 1 | **Done 2026-10-10** (e5ef004). `theme` module replaces the four copied palettes/fonts; weather emoji in colour (`emoji.rs`); dev.ps1 lint | bus popup in light theme only |
| 2 | **Done 2026-10-10** (fd6d67e). `block` in the library, tested with a fake Content | none (unused yet) |
| 3 | **Done 2026-10-10** (e98783e). Port **weather** | none |
| 3b | **Done 2026-10-10** (75032be) for weather. Show on taskbar + tray icon owned by Block. **Decided for every widget** (done.md 3b, points 1–5; media follows them in step 8): `show_on_taskbar` setting owned by Block; every right-click panel ends Show on taskbar / Compact mode / Start with Windows, then Edit settings… / Quit; the tray icon is a **static logo**, shown while the strip is off **and** while the widget is idle; strip shown = switch on **and** `Content::visible()` | weather tray icon becomes a static logo; every widget gets Show on taskbar + tray icon |
| 3c | **Done 2026-10-10** (9bf333f). dockctl (README "Testing"); other widgets get it with their ports | none |
| 4 | **Done 2026-10-10** (0a78233). Panel shell in Block; weather's flyout on it. Overflow test: `_tmp\overflow_test.ps1` | none |
| 5 | **Done 2026-10-11** (6c367ad). Port **discord**; `Ctx::resize_panel`, `Content::tint` | discord gets the tray icon; Show when not in a call goes |
| 6 | **Done 2026-10-11** (8b92874). Port **bus**; `Spec::dialog`, `Ctx::redraw_tray`. Hand checks passed 2026-10-11 | none |
| 7 | **Done 2026-10-11.** Animations: glides and the compact switch 300 ms on the taskbar's measured curve (`anim::ease_dock`, `glide_ms` 300); neighbours follow a switching strip frame by frame (`dock::publish_drawn`, `Slot::follow`). Media follows from step 8 | the switch and glides take 300 ms on the taskbar's curve; neighbours follow a switching strip |
| 8 | **Done 2026-10-11.** Port **media**; Block: strip fade in/out for every widget, `Content::panel_shown`, panel mouse capture, click pins a hover-opened panel, floating backdrop. Hand checks passed 2026-10-11 | right-click panel is the shared one; strips fade in/out; click pins a hover panel; no hover-open from a strip button |
| 9 | **Done 2026-10-11.** Dynamic width: `Spec::glide_width` (Block glides to a new content width at once, neighbours following; `Spec::width_hold` optional, none by default; `Frame::fit_width` may be wider than the content); media's text column = its text (up to `max_width`), discord's avatar column = everyone in the call (… media's `max_width`); `fixed_width` setting in both; discord shows every avatar when everyone fits | media and discord change width with their content |
| 10 | **Done 2026-10-11.** Deleted the unused library parts (no old per-widget wiring was left: each port removed its own), the width-hold option and the port-time migrations; README recipe is "Spec + Content", CLAUDE.md rule is "it's in Block / theme". Details: done.md | none |
| 11 | **Decided 2026-10-11** (`step11.md`). 11a: bus stop picker as a second Block panel kind (own text box, list, Cancel / Select; keeps its state when it closes), `Spec::dialog` goes; dockctl `panel open picker / key / type / state`. 11b: dockctl configures widgets: replies wait for slow work, `--dock`, `keys`, `unset`, per-widget `do` commands | picker looks like the others; widgets configurable from dockctl |
| 12 | **Settings app (idea, 2026-10-10).** Later (decided 2026-10-10): the per-widget tray icons go and merge into one tray icon for the settings app, which manages every widget. One settings app for every widget, like a library: a page per widget where the user can configure anything. Each widget describes its settings (label, type: switch / number with range / text / choice, help line); the app draws a generic page from that, so a new widget appears on its own and its page always matches its real settings; the standard ones (Show on taskbar, Compact mode, Start with Windows) look the same on every page. A Rust exe in taskbar-dock using the `theme` module (looks like the panels); "Edit settings…" (Block's standard row, one change) opens it at that widget's page, e.g. `taskbar-dock-settings.exe --widget weather`; notepad stays as a fallback. Writes go through `settings::update` (lock); widgets already follow file edits live (Block's watch). After step 10; the settings descriptions grow during the ports (5, 6, 8) | Edit settings… opens the settings app |

## 5. Parity check for each port

From `inventory-*.md`, per widget, checked before and after the port:
- drag starts after 3 px; centred under the mouse; zoom 1.15 while dragged; limits (tray,
  taskbar buttons); others jump, not slide, while one is dragged
- drop: reorder, glide into slot, zoom back; order file written
- compact on/off from the right-click panel animates the width, keeps the right edge
- crowded: narrow → compact → hide, back again (`TASKBAR_DOCK_TEST_ICONS_RIGHT`)
- neighbours move → instant reflow; z-order: `_tmp\watch_z.ps1` shows 0 restacks at rest
- hover panel opens after the hover time, closes 300 ms after leaving; click pins;
  right-click switches; drag closes panels; only one panel open across widgets
- cover: grows on hover, dips on press, opens the app on release; drag from cover still drags
- widget-specific items from its inventory (discord show/hide by state, bus floating, ...)
- Explorer restart and theme switch: the user does these by hand (not simulated)

Prefer the geometry/log scripts that print numbers over screenshots (CLAUDE.md rule).

## 6. Where the local LLM helps (and where it doesn't)

Uses the local model (free) for:
- Behaviour inventories per widget (done: `inventory-*.md`) → turned into `parity.md`.
- Step 0/1: `local_extract` of every colour, font, size and spacing literal in each widget's
  render/panel code into one table, to set the theme tokens.
- After each restart: `local_summarize` of the widget log and dev.ps1 output (errors, fit
  changes, drag events) instead of reading it in full.
- Per port: `local_ask_file` against the old wiring file for "where does it do X?" when an
  item of the checklist isn't obviously covered.
- Step 10: first drafts of README/CLAUDE.md wording (`local_ask`), then reviewed.

Done by Claude (needs exact reasoning): the Block design and code, every port, the theme
values, test writing, and deciding whether a parity item passes.
