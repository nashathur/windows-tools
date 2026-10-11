# Block: the shared strip window (`src\block`)

A widget gives a `Spec` and a `Content` and calls `block::run`; Block owns the strip window and all
its behaviour. On Block: weather (since plan step 3), discord (step 5), bus (step 6); media in step 8.
How it got here: plan\PLAN.md, plan\done.md and the step commits.

Files: `mod.rs` (the API), `core.rs` (pure decisions, tested with a fake Content), `place.rs` (left
side, floating), `win.rs` (windows, timers, dock/fit/hooks, Explorer restart, floating fallback,
menu), `shell.rs` (main panel, child module of win.rs), `tray.rs`, `remote.rs` (answers dockctl).

## API (`mod.rs`)
Spec, Content, Frame, Canvas, Hit, MenuBuilder with the standard rows. Ctx requests are applied
after the borrow: incl. `toggle_panel` / `open_menu` for a tray icon (no strip needed: the panel
opens above the pointer), `redraw_panel`, `resize_panel`, `redraw_tray`; `with` for the widget's
own windows / worker posts.

## Main panel (`shell.rs`)
Block's window (class `<class>Panel`): create, place, paint, Esc / lost focus / right click. The
Content gives `panel_size(kind, scale)` (px; None: no panel), `panel_draw(kind, cv, &PanelFrame)`
(theme bg already filled, opaque GDI) and optionally `panel_mouse`; no `Spec::panel` hook any more.
`Ctx::resize_panel()`: the open panel takes a new `panel_size`, re-placed by the rule it opened
with, after the strip's redraw, no fade.

## Strip
- `Content::tint(area, look) -> Option<Tint>`: a brand colour for the strip's highlight boxes
  instead of the neutral one: whole strip at rest/hover strength, the hovered button's box on top;
  order in `core::tints`, tested.
- During a compact switch `Frame::fit_width` is the animated width (`core::fit_width`), so a
  Content that keeps things at its right end lays out to it instead of being cut off (discord: the
  avatar column narrows, its avatars fading with it).
- `badge::font_px(look, scale)` for badge text.
- Cover feel and click: return the box from `Content::cover` (UI-RULES.md "Cover").

## The widget's own dialog
`Spec::dialog: Option<fn() -> isize>`: the widget's own dialog-like window open now (0: none).
Block's loop gives it Tab/Enter/Esc via IsDialogMessageW; the widget opens it from `menu_action`
via `ctx.after` and closes it itself.

## Show on taskbar + tray icon
The strip shows when the switch is on and `Content::visible()`; the tray icon (`tray.rs`, logo
from `Content::tray_icon`, tooltip `tray_tip`) shows whenever the strip doesn't. A widget without
`tray_icon` gets neither the switch nor the icon. `Ctx::redraw_tray()`: the tray logo changed,
Block asks `Content::tray_icon` again.

## Settings Block owns
Block owns the section's `compact` and `show_on_taskbar` keys: a widget saving its section must
merge, not replace it (weather `Config::save`).
