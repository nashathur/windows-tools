# Design tokens (step 0, measured 2026-10-10)

Source: `local_extract` over weather/discord/media `render.rs`, media `panel_view.rs`, bus
`main.rs`/`popup.rs`/`bar.rs`/`picker.rs`; every value below re-checked by grep in the source
(file:line given where it decides a token). The library's own `menu_panel.rs` and
`style.rs` were added: the shared right-click panel has its own palette copy too.

Sizes are px at 100% (× DPI scale, rounded). Colours are light / dark.

**Change in step 1?** "none" = the token reproduces today's value for every user. "visible" =
some widget would change by a few shades or px if it adopts the shared value; step 1's plan
allows only the bus popup to change, so those widgets keep an override until you decide
(open questions at the end).

## Colours (panel surfaces)

| Token | Proposed | Used today by | Deviations | Change in step 1? |
|---|---|---|---|---|
| `bg` | F3F3F3 / 202020 | weather (render.rs:257), discord (render.rs:609), media (panel_view.rs:251), menu_panel (:190) | bus popup always 2B2B2B (`POP_BG`, main.rs:59); bus picker system white (`COLOR_WINDOW`) | bus only (planned) |
| `fg` | 1A1A1A / FFFFFF | all four panels + menu_panel; also `fill` (slider/switch on) in media + menu_panel | bus popup always white | bus only |
| `dim` | 5F5F5F / A8A8A8 | all panels + menu_panel; also `knob` (switch off) | bus popup GREY 9A9A9A (main.rs:61) on dark only | bus only |
| `faint` | 8A8A8A / 808080 | media (source line, disabled buttons) | – | none |
| `line` (separators) | E0E0E0 / 3A3A3A | weather (render.rs:354, 369), discord (:715, 801) | media and menu_panel draw separators with `track` D6D6D6 / 454545 (panel_view.rs:336, menu_panel.rs:259) | visible (media, menu_panel) |
| `hover` (row / glyph highlight on `bg`) | E4E4E4 / 333333 | menu_panel, media | – | none |
| `button` (resting filled button) | E6E6E6 / 2D2D2D | discord panel buttons | – | none |
| `button_hover` | DADADA / 383838 | discord | – | none |
| `track` (bar/slider/switch background) | D6D6D6 / 454545 | media, menu_panel | weather range bar DADADA / 3A3A3A (render.rs:258, 261; dark equals `line`, barely visible) | visible (weather) |
| `placeholder` (missing cover) | DEDEDE / 383838 | media | – | none |
| `chip` (count pill bg / text) | D0D0D0 + 1A1A1A / 3C3C3C + FFFFFF | discord "+N" pill (render.rs:456) | – | none |

Notes
- Weather, discord and media panels agree exactly on `bg`, `fg`, `dim` (the plan's assumption holds).
- Media tints `hover`, `placeholder` and the progress `bar` toward the cover's colour (`wash` ≤ 0.5, panel_view.rs:386-391). Keep as a widget effect on top of the tokens: `theme.tinted(rgb, k)`.

## Colours (strip, on the taskbar)

| Token | Proposed | Used today by | Deviations |
|---|---|---|---|
| `strip_fg` | 1A1A1A / FFFFFF (`Look::text_rgb`, style.rs:287) | all four strips (text drawn as white coverage, then coloured) | – |
| `strip_dim` | 5A5A5A / C8C8C8 | weather humidity line, bus | media dims its artist line by coverage 78% (render.rs:297): dark = C7 (same), light = 1A at 78% alpha (not 5A) |
| `strip_disabled` | coverage 38% of `strip_fg` | media unsupported buttons (0x60 coverage, render.rs:302) | – |

`strip_dim` (5A/C8) differs from the panel `dim` (5F/A8) on purpose: different background (taskbar vs panel). Keep both.

## Status

| Token | Proposed | Used today by | Deviations |
|---|---|---|---|
| `accent` | Windows accent colour (live); fallback 0067C0 / 60CDFF | weather rain chance (render.rs:258, 261: Windows' *default* accent, hard-coded) | not used elsewhere yet |
| `danger` (text/icon) | C42B1C / FF99A4 (Windows `SystemFillColorCritical`) | new | discord active Mute/Deafen glyph uses Discord RED F23F43; bus "cancelled" FF6B6B on dark |
| `danger_fill` + hover (white text) | C43135 + D83C3E, both themes | discord Leave (render.rs:755-756) | – |
| `danger_soft` (switched-on bg) | F6D6D7 / 4A2426 | discord active Mute/Deafen (render.rs:757) | – |
| `ok` | 0F7B0F / 6CCB5F (Windows success) | new | bus "live" 7DDC7D on dark (popup.rs:31) |
| `warn` | 9D5D00 / FCE100 (Windows caution) | new | discord attention dot F0A030 on the strip (render.rs:463) |

Bus statuses are dark-only today; light values have to come from these tokens (step 1, planned change).

## Brand (allow-listed, `theme::brand(name)`)

| Name | Value | Where |
|---|---|---|
| `discord.blurple` | 5865F2 | discord logo, avatars bg, button tint (render.rs:16) |
| `discord.green` | 23A55A | speaking ring (render.rs:14) |
| `discord.red` | F23F43 | deafened ring, active glyphs (render.rs:15) |
| `bus.badge_fallback` | 00A8A8 | bus line badge without colour data (main.rs:58); real colours come from settings / line_colors.txt / GTFS (data, not tokens) |
| `media.live` | FF4E45 (setting `live_color`) | LIVE label + ring (config.rs:46) |
| `weather.temp_bar` | E8892B / FFB454 | temperature range bar fill (render.rs:258, 261); a data colour, kept as a brand entry |

## Type

Families today: weather and discord panels hard-code **"Segoe UI"** (render.rs:308-314, 698-701), bus picker too (picker.rs:122). Media panel, menu_panel, bus popup and all strips use **`look.font`** (default "Segoe UI Variable Text", style.rs:62). Proposed: every text token uses `look.font`; `icon` = "Segoe Fluent Icons" (discord, media, menu_panel), `emoji` = "Segoe UI Emoji" (weather).

| Token | Size / weight | Used today by | Deviations |
|---|---|---|---|
| `caption` | 12 / 400 | weather small, discord small (subtitle, count), menu_panel subtitle, media menu body + source | – |
| `caption_strong` | 12 / 600 | weather hourly times | – |
| `row` | 13 / 400 | menu_panel rows (:274), media menu rows (panel_view.rs:340), media click source line, bus popup rows (13) | bus 11 footer → `footnote` |
| `body` | 14 / 400 | weather body, discord member names, bus popup title (14/**400**), bus picker | – |
| `body_large` | 15 / 400 | media click artist/album | – |
| `title_small` | 14 / 600 | menu_panel title, media menu title | – |
| `title` | 15 / 600 | weather, discord panel titles | media click title 18/600 (panel_view.rs:421) |
| `title_large` | 18 / 600 | media click only | candidate to drop in favour of `title` (visible) |
| `display` | 46 / 300 | weather temperature | – |
| `footnote` | 11 / 400 | bus popup footer | – |
| `icon` | 16 | discord, media, menu_panel panel glyphs | – |
| `icon_large` | 20 | media transport buttons | – |
| `emoji` / `emoji_large` | 20 / 42 | weather | – |
| `strip_text` | `look.font_size` (12) / 400 | all strips | bus badge 700 (`badge::font`), media badge tile 0.42 × size / 700, discord pill 0.9 × / 600 |

Scaling: bus popup fonts scale by `look.font_size / 12` (main.rs:370) as well as DPI; every other panel is fixed px × DPI. Proposed: panels fixed × DPI (as Windows' own flyouts); bus loses the factor (no change at the default 12).

## Spacing (4 px grid + 2 px half steps)

| Token | Value | Used today by | Deviations |
|---|---|---|---|
| `xs` | 4 | menu row inset (menu_panel INSET), separator margin above/below, bus row extra height, slider track height | – |
| `tight` | 6 | strip icon-text gap (weather, bus, discord `TEXT_GAP`), discord button icon-text, bus title→rows | off the 4 grid but universal: keep as a token |
| `s` | 8 | strip padding (weather, bus, discord idle), discord pill/button gap | – |
| `m` | 12 | menu_panel padding, separator side inset (menu_panel, media), bus popup padding, strip fade-out width | – |
| `l` | 16 | discord panel padding, media click padding | – |
| `xl` | 20 | weather panel padding (render.rs:315) | proposed: weather → `l` 16 (visible) |
| `panel_padding` | `l` 16 (main panels), `m` 12 (menu panels) | see above | media menu 10 (panel_view.rs:114; goes away in step 7), bus picker 14 |
| `row_height` | 32 | menu_panel ROW_H, media menu rows | bus rows text height + 4 (~21); weather uses fixed y positions |
| `row_height_large` | 40 | discord member rows (ROW_H) | – |
| `section_gap` | 8 (4 + 1 px line + 4) | menu_panel separators | – |

## Sizes

| Token | Value | Used today by | Deviations |
|---|---|---|---|
| `panel_narrow` | 280 | menu_panel WIDTH | media menu 300 (setting `menu_panel_width`; replaced by menu_panel in step 7) |
| `panel_standard` | 320 | discord PANEL_W | – |
| `panel_wide` | 360 | weather POPUP_W, media click (setting `click_panel_width`) | bus popup and picker sized by content (picker 600 × 470 dialog) |
| `panel_height` | content | discord, menu_panel, bus | weather fixed 494 (POPUP_H), media click 232 / menu 250 |
| `icon_box` | 16 (tray icon) | `draw::icon_box`, `TRAY_ICON` | – |
| `button_box` | 16 + 2 × 4 = 24 | `draw::button_box`, discord `STRIP_BUTTON` 24 | – |
| `panel_button` | 36 (step 52) | media transport | discord panel buttons fill a 60 px band (BUTTONS_H) |
| `cover_small` / `cover_large` | 44 / 96 | media menu / click panel | – |
| `strip_max_width` | per widget | media `max_width` 136 | – |

## Already shared (no work)

Radii (`control_radius` 4, `card_radius`, `concentric`, `PANEL_RADIUS` 8, DWM round), `BUTTON_MARGIN` 4, motion in `Style`: `gap` 3, `panel_margin` 12, `hover_close_ms` 300, `fade_ms` 150, `glide_ms` 150, `zoom_ms` 120, `drag_zoom` 1.15, cover feel 1.06 / 0.96.

## Open questions for step 1 (visible changes, need your yes)

1. Separators: media + menu_panel D6/45 → `line` E0/3A (Windows' divider shade), or the reverse?
2. Weather range-bar track DA/3A → `track` D6/45 (dark today equals the separator colour).
3. Weather/discord panels "Segoe UI" → `look.font` ("Segoe UI Variable Text"): slightly different letterforms.
4. Weather panel padding 20 → 16.
5. Media click title 18 → 15 (drop `title_large`)?
6. Bus popup: besides following the light theme, use `row` 13 / `body` 14 / `footnote` 11 without the font-size factor, and `ok`/`danger`/`dim` for live/cancelled/scheduled.

Answered 2026-10-10 (step 1): all six yes. 1 separators → `line` E0/3A everywhere; 2 weather
track → `track` D6/45; 3 weather/discord panels → `look.font`; 4 weather padding → 16; 5 media
click title → `title` 15 (`title_large` dropped); 6 bus popup → light theme, `row`/`body`/`footnote`
without the font-size factor, `ok`/`danger`/`dim` statuses.
Named overrides kept in `theme.rs` (not asked, would change visibly): `weather.rain` (Windows'
default accent blue; the live accent here is green), `discord.attention` F0A030 (strip dot),
`font_override::WEATHER_TRAY` and `BUS_PICKER` ("Segoe UI"). Media's artist line (78% coverage)
and floating backdrop (EE/20) stay as they are (coverage/backdrop, not tokens yet).
