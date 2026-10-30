# Block: the shared strip window (`src\block`)

A widget gives a `Spec` and a `Content` and calls `block::run`; Block owns the strip window and all
its behaviour. On Block: weather, discord, bus, media, sysmon.
How it got here: the plan step commits in git history.

Files: `mod.rs` (the API), `core.rs` (pure decisions, tested with a fake Content), `place.rs` (left
side, floating), `win.rs` (windows, timers, dock/fit/hooks, Explorer restart, floating fallback,
menu), `shell.rs` (main panel, child module of win.rs), `tray.rs`, `remote.rs` (answers dockctl).

## API (`mod.rs`)
Spec, Content, Frame, Canvas, Hit, MenuBuilder with the standard rows. Ctx requests are applied
after the borrow: `redraw`, `redraw_panel`, `resize_panel`, `redraw_tray`, `every` / `stop` (content
timers), `close_panel`, `quit`, `after`.
`Content::commands()` lists optional dockctl `do` commands; `Content::command(name, args, token,
ctx)` returns `CommandReply::Done(result)` or `Pending`. A pending command retains its non-cloneable
`ReplyToken`; finishing it from any thread posts the result to Block's main window for delivery.
Dropping an unfinished token replies with an error. `Spec::defaults` supplies the widget's default
settings as JSON for dockctl `keys` and `unset`; `Spec::settings` and `Spec::description` provide
setting metadata. A side-effect-free `Describe` supplies the JSON written by `--describe`; Block
includes `compact` and `show_on_taskbar` in both. `reload` re-reads widget and dock settings;
`ping` returns the process id and executable build time without adding a log entry. dockctl
`status` batches these quiet pings from its own message window and also reports stopped widgets.
Right-click panel rows: `menu_panel::Row::switch` calls `Content::menu_switch`; `menu_panel::Row::choice`
(a label with a segmented picker, e.g. Update speed [High|Normal|Low|Paused]) calls
`Content::menu_choice(id, option, ctx) -> usize`, the option now selected; the panel stays open.
A left click on the strip always toggles the panel (Block's, no widget decides); a tray icon's clicks
are Block's too (no strip needed: the panel opens above the pointer).

The standard Edit settings… row opens `taskbar-dock-settings.exe` on this widget's page.
If the app is missing or does not start successfully, Block opens `settings.json` in Notepad.

## Main panel (`shell.rs`)
Block's window (class `<class>Panel`): create, place, paint, Esc / lost focus / right click. The
Content gives `panel_size(kind, scale)` (px; None: no panel), `panel_draw(kind, cv, &PanelFrame)`
(theme bg already filled, opaque GDI) and optionally `panel_mouse`.
`Ctx::resize_panel()`: the open panel takes a new `panel_size`, re-placed by the rule it opened
with, after the strip's redraw, no fade.
- The shell owns one HWND and one fade. Its active `PanelKind` is passed to every callback.
  `Main` is the hover / click panel; `Picker` is a focused, pinned secondary panel opened with
  `Ctx::open_panel_kind`. Opening a secondary kind closes menus and any current panel first.
  Unsupported kinds return `None`.
- `Content::panel_shown(kind, open, ctx)`: after it opens / closes (start or stop a frame timer
  for what moves in it; media's scrolling text and volume).
- The panel has the mouse from a press to its release (a slider dragged off the panel keeps
  following): `PanelMouse::Down` … `Up`, `DoubleClick`, or `Cancel` if the capture is lost.
- Focused panels receive normalized `panel_key`, `panel_char`, and `panel_wheel` events.
  `panel_key` returns true when handled; Esc closes the panel if the Content returns false.
  Text editing and Win32 clipboard access are shared in `text_edit`.
- No hover-open from a strip button (`Hit::Button`): while the mouse rests on one, each hover
  tick restarts the delay (`PanelBehaviour::over_button`); off it, onto the strip, the whole delay
  runs from there. An open panel is left as it is.
- A click inside a hover-opened panel pins it (`PanelBehaviour::click` → Pin), for every widget;
  the click still reaches `panel_mouse`.

## Strip
- `Content::tint(area, look) -> Option<Tint>` supplies only an automatic colour and active state
  for the strip or button; None means no automatic colour, so Block draws its neutral box. Block's
  `Highlight` setting owns Automatic / Chosen colour / Off, the chosen colour and Strength. Strength
  multiplies the shared opacity levels. With Chosen colour Block recolours areas using the strip's
  automatic colour while preserving a button's distinct warning colour. Opacities come from
  `Look::tint` (`core::Shade::of`): the `dock` section's `tint_dark` / `tint_light` quiet / active /
  hover levels, or `tint_fixed` (one opacity for every state: 0 none, 1 solid). Defaults
  `theme::tint_levels`.
- During a compact switch `Frame::fit_width` is the animated width (`core::fit_width`), so a
  Content that keeps things at its right end lays out to it instead of being cut off (discord: the
  avatar column narrows, its avatars fading with it).
- Width following the content, `Spec::glide_width` (media, discord): Block takes a
  new measured width at once (`core::WidthHold`: a width up to 4 px narrower than the held one is
  ignored, room to spare; any wider is taken, else the content is cut off; no delay, the user wants
  none), publishes it as `full` (the others refit
  once) and glides to it like a compact switch (`start_resize` watches the target width: compact
  or crowded; `core::animate_width`, tested), `publish_drawn` each frame so the neighbours follow. Meanwhile `Frame::fit_width` may
  be wider than the content: lay out to it up to your maximum (media's text column, discord's
  avatar column). Without the flag a new width jumps (weather, bus); a compact switch
  animates for every widget.
- The compact switch and every glide take `glide_ms` (300) on the taskbar's own curve
  (`anim::ease_dock`). Each frame of a switch Block publishes the drawn width
  (`dock::publish_drawn`, before the window takes it; cleared + `notify` on the last frame) and
  passes the right-anchored position to `UpdateLayeredWindow` (`strip::right_anchored`: size and
  position in one step). The others place themselves by it (`dock::placed_width`) and, while
  `Slot::follow`, jump to their slot each frame (`Motion::follow`) instead of gliding.
- Block's animations (glide, zoom, switch, panel fade, cover feel: `FRAME_TIMERS` in win.rs) run
  on the display's refresh, not a timer (`frames.rs`: `DwmFlush` on a worker thread, one frame
  message at a time). Content timers (`Ctx::every`) are plain SetTimer (~15.6 ms resolution).
- Fade in / out (every widget): a new strip (start, Explorer restart, Show on
  taskbar on, `visible()` turning true) fades in over `glide_ms`; one switched off or with
  nothing to show fades out first and keeps its slot meanwhile (the others close up after), then
  is removed; wanted again mid-fade, it turns back from where it is (`anim::Appear`, `T_APPEAR`
  on the frames). Only the window's opacity changes (`UpdateLayeredWindow` blend only), so the
  Content needn't draw while it goes; clicks on a fading-out strip do nothing. Instant: Quit,
  hiding for lack of room, Windows' animations off.
- Explorer restart: Block re-creates the strip in the new taskbar (watch tick every second and
  `TaskbarCreated`). Floating fallback (`Spec::float`, `Float::Fallback`): attaching failed or
  no taskbar for 10 s → a topmost window (`FloatAt::BesideDock`: left of the dock, not
  draggable; `FloatAt::Saved`: at a saved x, draggable); hides while a full-screen app is in
  front; re-attaches when it can (after a failed attach, retried every 60 s).
  `Spec::force_float` floats from the start.
- Floating strips get a backdrop in the panels' background (`theme.color.bg`, 96 %), in the
  highlight's shape, under everything (readable over other windows).
- `badge::font_px(look, scale)` for badge text.
- Cover feel and click: `Content::hit` returns `Hit::Cover(box)` and `Content::cover` the same
  box (UI-RULES.md "Cover").

## Show on taskbar + tray icon
The strip shows when the switch is on and `Content::visible()`; the tray icon (`tray.rs`, logo
from `Content::tray_icon`, tooltip `tray_tip`) shows whenever the strip doesn't. A widget without
`tray_icon` gets neither the switch nor the icon. `Ctx::redraw_tray()`: the tray logo changed,
Block asks `Content::tray_icon` again.

## Idle
A widget is Idle while it is running, has a tray icon, Show on taskbar is off, and no Main or Picker
panel is open; the right-click menu does not count. A hidden strip caused only by `Content::visible()`
is not Idle. Block keeps the process loaded, pauses all content timers, and re-arms each timer still
requested when the widget wakes. `Content::idle` pauses or resumes work outside those timers; opening
a tray panel or turning Show on taskbar on wakes it and requests an immediate fetch. Weather and bus
pause their fetch workers; media keeps its session watch, and Discord keeps its RPC connection and
`VOICE_CHANNEL_SELECT` watch. While idle the tray tooltip is Block's "<title>: paused. Click to
update." instead of `Content::tray_tip` (which would show old or no data), unless
`Content::live_while_idle` says the data stays live (Discord, media). dockctl `ping` and `state` report `idle`; Settings shows “Idle” with a
neutral dot and the process id.

## Settings Block owns
Block owns the section's `compact` and `show_on_taskbar` keys. A widget writes only keys the user
changed; on start it fills absent keys with `settings::fill_missing`, preserving Block's keys and
any values already in the section. If the section did not exist before first start, Block enables
Start with Windows; later user changes are kept. Block also owns `highlight`, `highlight_color` and `highlight_strength` in every widget section. The Colour row shows only when Highlight box Mode is Chosen colour.
