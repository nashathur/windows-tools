# UI rules for every widget

Shared: a change to any rule here changes it in every widget (CLAUDE.md "five parts" rule).
Corner reasoning and table: README.md "Corner design".

## Rounded corners
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
   round to whole px; `shape_cover`/`background_rect` clamp to min(w, h) ÷ 2. One
   `draw::fill_round` (anti-aliased, alpha) for panel pixels; don't copy it into a widget.
   Known gap: weather's bars and discord's buttons are GDI `RoundRect` (not anti-aliased,
   drawn on a plain DC); the radius is shared, the edge is not smooth.
4. **Panels:** Block's panel windows get Windows' own round corner (`DWMWCP_ROUND`); a widget
   draws inside them and never shapes the window itself.
5. **Drawn shapes** go through `draw::round_rect` / `background_rect` (anti-aliased coverage, so
   the edge fades instead of stepping). Borders and rings follow the same shape (`progress_ring`
   runs along the rounded outline); inset a stroke by half its width.
6. **Radius above 8 px (`draw::SMOOTH_ABOVE`):** a plain circular arc shows where the straight
   edge ends and the curve starts, so `draw::shape_cover` (used by `round_rect` and
   `fill_round`) switches to a continuous-curvature corner: superellipse n = 3.2 reaching
   1.5 × radius along each edge, as deep on the diagonal as the circle (tested). Automatic;
   callers just pass the radius. Capsules and circles (radius ≥ shorter side ÷ 3) stay
   circular. Only the cover art in the media panel (12 px at 96 px) uses it today.

## Button highlights
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
Leave / switched-on), the geometry stays shared. A new button gets this by using `button_box`.

## Cover: logo opens the app
Clicking the strip's cover art or logo brings the app it shows to the front (media: the playing
app; discord: Discord), on release over the same spot; it leaves the panel be, a drag from it still
moves the strip. The widget only says which windows are the app's and the fallback to open;
`window::raise(is_app, fallback)` finds the main window, restores and raises it (or opens the
fallback: `shell:AppsFolder\<id>` or a `scheme://` link, for an app sitting in the tray). Build
the cover's box with `draw::centered_box`.

**Cover feel** is Block's. The widget does two things with the same resting box: `Content::hit`
returns `Hit::Cover(box)` over it (Block's hover, press and click on the cover need this; the
default `hit` is `Hit::Strip`, and a click there toggles the panel), and `Content::cover` returns
it. Block then grows it while hovered (`anim::HOVER_GROW` 1.06), dips it while pressed
(`anim::PRESS_SHRINK` 0.96), eased and only while the strip is at rest, and calls
`Content::cover_click` on release.
