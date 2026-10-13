# Step 12b: changes before the 12a commit (asked 2026-10-13)

The user asked for these after trying the 12a app. 12a is committed and pushed (2026-10-13).

## The six changes (the user's words, then what was found in the code)

1. **Widgets page.** No Stop / Restart buttons on the cards. Write "Off" left of the Show on taskbar
   switch; when it is off, write "Off" under the widget's name too. The status dot and
   "Running · pid N" move into each widget's own page (its status card already has them; the
   Stop / Restart buttons stay there).
   Code: `settings-app\src\win.rs`, the Widgets page card (search `reserve = if process_present`;
   copies of the card and text area are in `tools\_tmp\card.rs`, `_tmp\text_area.rs`).
   Removing the action column also changes the text's right reserve (236 / 172 px) and the
   column layout `[Star, Auto, Auto, Pixel(52)]`.
2. **"Idle" status.** A widget whose strip isn't shown stays loaded (quick to show again) but
   stops live updates and data fetching; the app shows "Idle" instead of "Running".
   Found: Block decides showing in `block/core.rs` `shown(switch, visible, has_tray)`; content
   timers (`Ctx::every`) and worker threads (weather and bus fetches, discord RPC, media SMTC
   events) keep running while hidden. Honest note for the user: idle saves processor time and
   network, not memory (the program stays loaded, which is what makes it quick).
3. **Animation like Windows Settings.** Page changes slide / fade in as in Windows Settings, and
   clicks have a pressed effect.
   Found: the vendored reactor exposes only `ExitTransition::fade` (on removal) and Border's
   `OpacityTransition` / `ScaleTransition` properties; no page or entrance transitions
   (`EntranceThemeTransition`, `NavigationThemeTransition`). They need a reactor patch (record it
   in `vendor\PATCHES.md`). Buttons already have WinUI's pressed colour; check the whole-card
   button shows it.
4. **Highlight box settings for every widget**, like media's: the box colour automatic or chosen,
   strength (opacity), off. A rule for every future widget (CLAUDE.md "Every widget has these
   parts", UI-RULES.md, BLOCK.md, README "Building a new widget").
   Found: media has `tint` (brand / cover / off), `tint_strength`, `live_color` in
   `taskbar-media\src\config.rs`, group "Colour"; Block draws it from `Content::tint` →
   `Tint { rgb, active, strength }` (`block/mod.rs`, search `pub struct Tint`), levels shared in the
   `dock` section (`tint_dark`, `tint_light`, `tint_fixed`; `style.rs`). Discord always tints
   blurple; weather and bus use Block's neutral box. Per the "where a rule lives" rule, the settings
   (mode, colour, strength) belong in Block (like `show_on_taskbar`), with the Content only
   supplying its automatic colour.
5. **Bus widget's name: "Bus stop"** (the title in its describe / Spec; check the tray tooltip,
   menu and settings app follow).
6. **Discord avatars.** The 5 places always go to whoever spoke last; muted or deafened people
   also get a place if there's room, and keep it until someone who speaks takes it.
   Found: `taskbar-discord\src\voice.rs` `Slots::update`: when the call is crowded, it drops the
   muted / deafened from their places and never gives them one. New rule: everyone may hold a
   place; a speaker takes an empty place, else the one quiet longest; the muted keep theirs until
   then. Update the two tests (`slots_skip_the_muted_and_go_to_speakers` becomes the new rule).
7. **Neighbours glide, never jump.** Turning Show on taskbar from off to on: the shown widget's
   fade-in is right, but the widgets it pushes aside jump to their new place at once. Rule from
   now on: whenever a widget's position changes for any reason (another strip appears, hides,
   resizes, a drag ends), it glides there. Write it in UI-RULES.md as a rule for every widget.
   Found: Block's `move_to` in `block/win.rs` (search `fn move_to`) asks
   `strip::Motion::glide_to(cur, x, always_slide, …)`, which chooses a slide or a jump
   (`Glide::Jump` → `put`); the jump case is what the neighbours get here. Find which rule in
   `strip.rs` picks the jump (likely a slot change caused by another strip, or a distance limit)
   and make it a slide. Check with two widgets: flip one's Show on taskbar through dockctl and
   watch the other's x over time (a geometry script that prints numbers, then a screenshot).

## Decided (the user, 2026-10-13)

- **1, the switch text.** "Off" left of the switch only while it is off; nothing there while
  on. The card's second line under the name: "Off" while off, the widget's short description
  while on (the status moves to the widget's own page). (Reading of "when on, display the short
  desc": if the user meant the text left of the switch, it is a one-line change.)
- **2, Idle, a rule for every widget and future widget.** Start with Windows is on by default
  for every widget. A widget that is running with Show on taskbar off is **Idle**: loaded, quick
  to show again, no live updates and no data fetching. Discord and media keep only their cheap
  watch for a call starting / something playing while idle (they're running because Start with
  Windows started them). With Start with Windows off a widget isn't running at all, so nothing
  listens. Opening an idle widget's panel from its tray icon fetches at once; it goes idle again
  when the panel closes. Block owns the idle state (a `Content` method with a default, as the
  "where a rule lives" rule says); write the rule in CLAUDE.md "Every widget has these parts",
  BLOCK.md and README "Building a new widget". The app shows "Idle" for it.
- **4, the automatic colour follows what the widget shows.** Media: the playing
  app's brand colour; for a website, worked out from the site's tab icon where possible (media
  already finds site icons and brands: `taskbar-media\src\iconurl.rs`, `sites.rs`, `logos.rs`,
  `brands.rs`). Discord: Discord's colour. Weather: a colour for each weather description (clear, cloudy,
  rain, storm, fog, …), the set of colours to be chosen next session and shown to the user
  before building. Bus: its line badge colour, the one the badge already uses: the `badge_color` setting if
  set, else the line's row in the colour list `buswidget\data\line_colors.txt`
  (`find_line_color` in `buswidget\rust\src\data.rs`), else the timetable's route colour; with
  none of these, the widget's neutral box. Every widget also offers
  a manual colour, a strength, and Off. The settings live in Block, the Content supplies its
  automatic colour.
- **3, motion for now:** opening a page (a widget's page, Dock, Developer) slides its content
  in from the bottom upwards, as in Windows Settings, and clicks have the pressed effect. No
  card-by-card fading.

## Gaps filled while building (2026-10-13)

- **1, cards:** the Start button goes too (no buttons on a card at all); Start / Stop / Restart
  live on the widget's page. A widget in trouble ("Not responding", "Keeps crashing") still shows
  that, with its coloured dot, under its name on the card, whatever the switch, so a problem
  isn't hidden.
- **2, Idle:** the app shows "Idle" with a grey dot. "Start with Windows on by default" means a
  widget's first run ever (no settings section yet) puts it on the startup list; once the user
  turns it off it stays off. Block pauses a widget's content timers while idle (no widget code
  for those); a widget implements `Content::idle` only for its worker threads.
- **7, glide:** the jump came from one rule in `strip.rs`: a strip moving left (making room for a
  newcomer) jumped, to avoid briefly overlapping it. Now every move slides; the newcomer is still
  faint while it fades in, so the overlap doesn't show. Neighbours still follow a neighbour whose
  width is animating frame by frame. Done directly (small change), not through a Codex brief.
- **4, highlight box:** three Block keys in every widget's section, group "Highlight box":
  `highlight` (auto / custom / off), `highlight_color`, `highlight_strength` (0-3). A chosen colour
  replaces the strip's own colour but not a warning colour (discord's red Leave stays red). Media's
  old `tint` / `tint_strength` move over once (brand and cover → auto); the "cover" choice is gone.
  Media's site colour from the tab icon works only when the tab title shows the site's address;
  otherwise the brand file's colour, else the browser's. Weather's colours are the ones the user
  approved (theme.rs `weather.*`).
- **3, motion:** a page's heading and content rise 40 px while fading in (WinUI's
  EntranceThemeTransition, about 0.6 s); opening a widget's page from the Widgets list slides it
  in 80 px from the right instead, and going back slides the list in from the left (user,
  2026-10-13; `entrance_for` in win.rs). WinUI's own buttons already show the pressed effect.
- **Page width (user, 2026-10-13):** as Windows Settings, a page's cards and heading stop growing at
  1000 px; on a wide window they sit in the middle of the whole window (the screen when
  maximised), not of the area right of the menu (user, 2026-10-13): a right margin of up to the
  menu's width from spare room (`PAGE_MAX_WIDTH`, `page_shift`, a WM_SIZE subclass in win.rs).
- **5, bus name:** the title is "Bus stop" in the bus widget's Spec; the app, tray and menu read it
  from there.

## Status
Built, checked live and committed 2026-10-13 (`c56eda6`, after one Codex Sol review: four
findings, all fixed).

## Still open (the next session: `step12c-session.md`)
- Edit settings… from each widget's right-click menu, and the Dock time format followed by bus
  and weather: re-check live on the WinUI app.
- 125% / 150% scaling: needs a display-scale change (ask the user).
- The user signs out and in; then check every widget on `startup` runs, and that a widget with
  Show on taskbar off starts idle (no fetch until its tray panel opens).
- 12b checked live only through dockctl and scripts so far: the Idle wake from a real tray click,
  the Highlight box rows changed in the app itself (Chosen colour, Strength slider, Off), and the
  page slide-ins and centring clicked by the user.
- Media: a site's colour from its tab icon only when the tab title shows the address; reading the
  tab's address through UI Automation would cover every site (an idea; ask).
- The Highlight box Colour row shows in every mode; hiding it unless "Chosen colour" needs a
  "shown when" rule in the settings description (an idea; ask).
- Optional Gemini Flash rating of the screenshots; delete `tools\winui-test\` (ask).
