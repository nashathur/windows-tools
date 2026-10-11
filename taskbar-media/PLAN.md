# taskbar-media: plan

A "now playing" widget docked in the Windows 11 taskbar, built on the shared `taskbar-dock` library (`C:\Users\thurs\tools\taskbar-dock`, see its README for the 16-step recipe). It looks and behaves like the bus and weather widgets.

Status: plan agreed in chat on 2026-10-08. Stage 1 (strip) built 2026-10-09 and checked on screen: text rows match the tray clock (baselines at rows 19 and 35 of the 48 px taskbar), buttons centred on the tray icons, ring, compact mode, menu, ±15 s on Firefox (22 ms), play/pause. Not yet seen live: Spotify's previous/next and moving ring, the 2-minute hide, Explorer restarts.

Stage 2 (panels) built 2026-10-09 and checked on screen with Spotify: hover panel 300 × 136 opens on hover; clicking the strip grows it into the click panel 360 × 270 (with the volume row's space kept for stage 3); Esc closes it; the click panel's play/pause and seek bar work (seek to 50% landed at 2:11 of 4:22). The panels' progress bar follows the strip's ring: live position → filled with times; none (Firefox) → full, pulsing, no times; live stream → red, pulsing, "LIVE". Not yet seen live: scrolling of a long title, a live stream.

Stage 3 (volume) built 2026-10-09 and checked on screen with Firefox: the click panel's volume row (speaker = mute, slider, percentage) sets the app's Volume Mixer entries on every device; 50 % → 100 % → muted → unmuted read back exactly from the mixer. Changes made in the mixer show up within half a second.

Changed after review (2026-10-09), replacing sections 4.3 and 4.5: **no hover panel** (hovering opens nothing) and **no Windows menu**. A right click opens a **menu panel** (300 px, same look as the click panel): small cover with title, artist · album and app; Open *app*; Copy "Artist – Title"; Compact mode and Start with Windows as on/off switches; Edit settings… and Quit. A right click while the click panel is open switches to the menu, a left click while the menu is open switches to the click panel. Setting `hover_panel_width` became `menu_panel_width`. Also: empty text lines in the click panel close up (no album → no gap).

Changed after review (2026-10-09, later): hovering opens the **click panel** again (without focus; it closes when the mouse leaves; a click on the strip or in it pins it). The panels are more compact: click panel 360 × 232 (was 270 tall), menu panel 300 × 250 (was 290).

Fixed 2026-10-09: the widget crashed (access violation in USER32's DrawTextW, seen twice in the Windows event log) when a session briefly had an empty title or artist, e.g. a new Firefox tab starting a video or a stream starting: drawing empty text passed Windows a zero-length buffer. Empty text is no longer drawn or measured. Crashes from Rust code are written to taskbar-media.log ("crashed: …").

Fixed 2026-10-09: switching Firefox to another video or stream (another tab) could keep the previous cover. Firefox reports a new tab's title before its picture, and replaces its session object when another tab takes over, so the widget (a) looks at the picture again 1.5 s and 4 s after a track change and whenever the session says its properties changed, (b) listens to each session object rather than once per app, and (c) of several sessions with the same app id (several Firefox tabs) shows and controls the playing one, else the newest.

Stage 4 (tint and logos) built 2026-10-09, not yet checked on screen:
- `media_brands.txt` got the `logo` column: `app` for the apps, the site's domain for the sites (the widget finds the site's own icon in its home page: apple-touch-icon first, never SVG).
- Firefox/Chrome/Edge: the playing tab is found by UI Automation (its title contains the media title) when the track changes; its site picks the brand. Panels say "YouTube · Firefox".
- Tint: strip 20 % (dark taskbar) / 15 % (light) × `tint_strength`, fading in with playback and out on pause (`fade_ms`); panels get a 10 % wash, and their progress bar and volume fill take the brand colour (a neutral brand keeps the plain ones). Off with `tint: "off"` or in high contrast; `tint: "cover"` takes the cover's main colour. An app not in the brand file gets its icon's main colour.
- No cover: the site's or app's logo (cached in `logos\`, refreshed after 30 days, at most 1 MB, a failed download retried after a day), else a badge in the brand colour with its label, else ♪.

Live streams, revised 2026-10-09: Firefox drops the length of every video while it plays (an ordinary YouTube video then looks like a live stream: "a position but no length"), so for apps that don't report progress live a track counts as live only when it's paused with no length (an ordinary video reports its real length then), or when its tab is on Twitch or Kick. Apps that report progress live (Spotify) are still trusted. Fixed again the same day: Firefox also reports "paused, no length" for a moment while an ordinary video loads, which turned YouTube red after a restart. Now that rule needs the track to have played and to stay that way for 2 s (checked every second), and a track that later reports a real length while paused stops being live. Twitch/Kick tabs are live at once.

Known since stage 3: Firefox only shows that a stream is live when it starts (or after pause and play), so after a widget restart during a stream it looks like an ordinary video (white ring, −/+ active) until then.

Stage 5 (polish) built 2026-10-09 and checked on screen with `--float`: floating strip, panels and buttons work, hides for full-screen video. Fixed after review: it first sat against the tray, over the docked bus and weather widgets; now it sits left of all dock members and follows them.
- Fallback window: if attaching to the taskbar fails, or there has been no taskbar for 10 s (an Explorer restart brings it back sooner), the strip is a floating topmost window left of the tray and the docked widgets (bottom right without a taskbar) with a taskbar-coloured backdrop (96 %) in the hover highlight's shape (inset, so the taskbar's top border stays visible; it covered it at first). It hides while a full-screen app is in front, isn't draggable, and attaches as soon as a taskbar is there (after a failed attach: retried every minute). `--float` forces it, for testing. A `TaskbarCreated` message no longer drops a floating strip without destroying it.
- Start with Windows and Explorer-restart recovery were already in place (stage 1); README.md written; cleanup: clippy's mechanical fixes applied (backup in `src_backup_20261009`), the remaining warnings are "too many arguments" on the drawing functions.
- Ported (2026-10-09) to the shared `taskbar_dock::strip` module like the bus widget: glide, drag-to-reorder and the drag zoom follow the same rules in every widget; placement.rs only wires them (backup `src_backup_20261009_2`).

Differences from the plan found while building:
- The ring sits 1 px away from the cover, so it adds 6 px of width (not 4).
- Changed after review (2026-10-09): the strip's cover is no longer tray-icon sized; it spans both text lines, from the top of the title's capitals to the bottom of the artist's descenders (26 px at 100%, rows 12–37), with 4 px corners; the ring around it is 32 px.
- Changed after review (2026-10-09): the strip has a static width. The text column is always `max_width` (now 136 px; 200 before), so the strip, its buttons and the neighbouring widgets don't move when the track changes. The ring has the same 4 px margin to the hover highlight on the left, top and bottom (the cover moved up 1 px for it). The artist line is 78% bright. The strip fades in and out (`fade_ms`, 300).
- Changed after review (2026-10-09): with no position to show (Firefox while playing) the ring is full and its brightness pulses slowly and smoothly (35% → 100% → 35% every 2.4 s, `anim::pulse` in the shared library) instead of having no ring. Still and dim when paused, still and bright with Windows animations off; red for live streams.
- −15 s / +15 s use Segoe Fluent Icons' rewind / fast-forward glyphs; the font has no "15" glyph, and a number wouldn't be readable at 16 px anyway.

## 1. Basics

| | |
|---|---|
| Folder | `C:\Users\thurs\tools\taskbar-media` |
| Language | Rust, `windows` crate (WinRT media API, audio API, UI Automation, WIC for images), like taskbar-weather |
| Library | `taskbar-dock = { path = "C:/Users/thurs/tools/taskbar-dock" }` |
| Settings | section `media` in `%APPDATA%\TaskbarDock\settings.json` |
| Brand file | `media_brands.txt` next to the exe (the agreed colour dictionary plus a logo column), watched for edits |
| Logo cache | `logos\` next to the exe |
| Runs from | a copy, `taskbar-media\taskbar-media.exe`, like the bus widget, so rebuilding doesn't need the widget stopped first |
| Supported players | Spotify (desktop app) and Firefox (YouTube and other sites) are tested. Anything else that reports to Windows' media controls should work. Discord is not supported: it doesn't report to Windows. |

## 2. Data sources (all verified on this PC)

| What | API | Findings |
|---|---|---|
| Title, artist, album, cover, play state, buttons, position | Windows media sessions (`GlobalSystemMediaTransportControlsSessionManager`) | Spotify: everything, position every ~4.5 s, seek supported. Firefox: title, channel, thumbnail, play/pause. Position and length are 0 while playing and correct while paused or after a seek. Next/previous were off for a single video. |
| Per-app volume and mute | Windows audio sessions (Volume Mixer) | Spotify and Firefox each have their own entry. Firefox has entries on 2 devices. FxSound is in the audio path and must never be touched. |
| Linking a media session to its audio entry | Spotify: package name in both. Firefox: registry `HKCU\Software\Mozilla\Firefox\TaskBarIDs` maps the install folder to `308046B0AF4A39CB`. | |
| Which site Firefox is playing | UI Automation: read Firefox's tab titles, find the one containing the media title, read the site from the title | Works: found the YouTube tab among 28 tabs in 194 ms. Firefox exposes no "playing" indicator, so matching is by title only. |

## 3. Which session is shown: always the one playing

- **One session only:** the widget always shows the session that's playing. There's no session list and no manual choice.
- **More than one playing:** the one that started playing most recently wins.
- **Nothing playing:** the last session that played stays shown, paused, for `hide_after_pause_s` (120 s), then the strip hides. If its app or tab is closed, the strip hides at once.
- **Hidden strip:** it takes no space, and the other widgets slide over. That's already handled by the dock, which only counts visible windows.

## 4. The windows

### 4.1 Strip, full
`[cover + ring] Title… / Artist   ⏮ ⏯ ⏭`

- **Cover:** sits in the tray-icon box (`draw::icon_box`). If there's no cover, the fallback image from 4.6 is used.
- **Progress ring:** a thin ring around the cover (4.7).
- **Text:** two lines, title over artist. The text is as wide as the longer line, up to `max_width` (200 px at 100% scale), then cut with "…". Changed 2026-10-11 (Block plan step 9): the column is exactly as wide as the text (no fixed width, no minimum), and a line longer than `max_width` scrolls like the panels' (4.8; "…" with animations off).
  - **Alignment with the tray clock:** the title sits on the same baseline as the clock's time line, and the artist on the same baseline as its date line, using the shared `draw::two_line_tops`. The font and size are the clock's (`dock.font`, `dock.font_size`).
  - **Checked on screen in stage 1:** a taskbar screenshot at your current scale, comparing the text rows of the widget and the clock pixel by pixel. If they're off by even 1 px, the fix goes into the shared library, so the bus and weather widgets stay aligned too.
- **Buttons:** Segoe Fluent Icons glyphs at tray-icon size, each with its own 4 px-radius hover highlight. Buttons an app doesn't support do nothing.
  - Apps with next/previous (Spotify): ⏮ ⏯ ⏭.
  - Apps without them (single YouTube videos): −15 s / ⏯ / +15 s.
  - ⏯ shows play or pause to match the current state.

### 4.2 Strip, compact (`compact: true`)
`[cover + ring] ⏮ ⏯ ⏭`: the same as the full strip without the text.

### 4.3 Hover panel (glance)
- **Size:** fixed width, `hover_panel_width` (300 px at 100% scale). It never resizes to fit the text.
- **Content:**
  - Cover, 64 px.
  - The title on one line. If it's longer than the space, it scrolls (4.8).
  - Artist · album, on one line; scrolls the same way if too long.
  - The source, e.g. "Spotify" or "YouTube · Firefox".
  - A progress bar with times, display-only.
- **No buttons:** the strip's buttons are just below it.
- **Opening:** it doesn't take focus, opening and closing by the shared `PanelBehaviour` rules.
- **Click to expand (option A):** clicking a hover-opened panel pins it and grows it into the click panel.

### 4.4 Click panel (full control, focused)
- **Size:** fixed and larger than the hover panel: `click_panel_width` (360 px at 100% scale). It never resizes to fit the text. A fixed height too, so it doesn't jump when the track changes. The volume row keeps its space even when hidden.
- **Details:** cover (96 px), title, artist, album and source. Long text scrolls (4.8).
- **Progress bar:** drag to seek where the app supports it (Spotify). Display-only otherwise.
- **Buttons:** larger playback buttons, the same set as the strip.
- **Volume:** a slider and a mute button (the speaker icon).
  - It changes the app's Volume Mixer volume, on every one of that app's entries.
  - It's hidden if the app has no mixer entry.
  - Changes made elsewhere show up live.

### 4.5 Right-click: standard Windows menu
- Open *(app)*: brings Spotify or Firefox to the front.
- ✓ Compact mode
- ✓ Start with Windows: on the shared `startup` list in settings.json, started at sign-in by `taskbar-dock-start.exe` (2026-10-09: Windows skipped the old `HKCU\…\Run` value)
- Edit settings…
- Quit

### 4.6 When there's no cover art
Fallback order:
1. The **cover or thumbnail** from the app.
2. The **app or site logo**, from the logo cache:
   - **Sites:** the `logo` column of `media_brands.txt` holds the site's own icon URL (the ones found during the colour research: apple-touch-icon or manifest icon, preferring the largest PNG).
     - It's downloaded once in the background into `logos\<name>.png` and refreshed after 30 days.
     - Only PNG and ICO are accepted (no SVG), up to 1 MB, and only if the file decodes.
     - If a download fails, the widget keeps any old copy and retries the next day.
   - **Apps:** `logo` is `app`, meaning the icon is taken from the app on this PC (exe icon or Store package logo), with nothing downloaded.
   - **Scaling:** the logo is scaled to fit the cover box, like `draw::fit_icon`.
3. A **brand-coloured badge** with a short label (`badge` module, the same style as the bus line numbers).
   - The label comes automatically from the name's capitals ("YouTube" → "YT", "SoundCloud" → "SC") or its first two letters.
   - A `label` can be set per entry to override it.
4. A **♪ glyph**, for an unknown source.

### 4.7 Progress ring (strip, full and compact)
- **Shape:** 2 px wide at 100% scale, following the cover's rounded square. It starts at top centre and fills clockwise. The cover keeps its 16 px tray-icon size, and the ring goes around it (about 4 px wider).
- **Filled part:** white on a dark taskbar. On a light taskbar white would be invisible, so it uses the taskbar text colour (near-black) there.
- **Unfilled part:** the same colour, faint (about 25%).
- **Live stream:** the whole ring in `live_color` (`#FF4E45`), not animated.
- **Paused:** the ring stays, showing where playback stopped, until the strip hides.
- **No timeline** (Firefox while playing): no ring.
- **Redrawing:** only when a pixel actually changes. For a 4-minute song that's roughly every 4 s.
- **Drawing:** smooth edges come from a new shared `draw::progress_ring` in the library (9).

### 4.8 Scrolling text (panels)
- **Short text:** text that fits stays still.
- **Long text:** waits 1.5 s, then scrolls left at about 30 px/s in a loop: the start of the text follows 40 px after its end, and it keeps going until it's back where it began, then waits again (changed after review, 2026-10-09; it used to pause at the end and jump back). Soft fades at both edges.
- **Animations off in Windows:** no scrolling; the text is cut with "…".
- **Redrawing:** only while the panel is open.
- **Library:** this lives in the shared library as a new `marquee` module (9), so other widgets can use it.

### Clicks on the strip

| Where | Left click | Hover | Drag |
|---|---|---|---|
| Button | That action only; the panel doesn't open | Button highlight; the panel still opens after the hover delay | Starts a drag past 3 px, as elsewhere |
| Cover or text | Opens or closes the panel (shared rules) | Strip highlight, then the hover panel | Reorders the widget |

There's no scrolling and no middle-click.

## 5. Progress data
- **When it counts as known:** only when the app reports a real length (greater than 0).
- **Between updates:** the widget counts forward locally between the app's updates (Spotify sends one about every 4.5 s), and snaps to each new value.
- **Firefox while playing:** it reports 0, so there's no ring or bar. After a seek, Firefox reports a valid position; use it if it's greater than 0 and consistent.
- **Live streams:** no length. The ring turns red and the panels show "LIVE" instead of a bar.

## 6. ±15 s skip (`skip_seconds`, default 15)
- **If the position is known:** seek directly to position ± 15 s, clamped to the start and end.
- **If not (Firefox while playing):** pause → wait until the paused state reports a position (36 ms in the test) → seek → play.
  - Tested on Firefox: 41:27.9 became 41:42.9.
  - The expected gap is about 50–100 ms.

## 7. Tint
- **Colour lookup, in order:**
  1. The site: Firefox tab title → `[sites]` in `media_brands.txt`. If several entries match, the one appearing last in the title wins; on a tie, the earlier line wins.
  2. The app: `[apps]`, where the first match wins.
  3. The app icon's main colour, as a fallback for unknown apps.
  4. `neutral`, meaning grey.
- **Strengths:**
  - Strip: a rounded rectangle at 20% (dark theme) or 15% (light theme) × `tint_strength`.
  - Panels: a 10% wash.
  - Panel progress and volume fills: the solid colour. The strip's ring stays white, as in 4.7.
- **Only while playing:** the tint fades in on play and out on pause (`fade_ms`; instant when animations are off). The compact strip gets it too.
- **Off:** in high contrast mode, or with `tint: "off"`. `tint: "cover"` takes the colour from the cover art instead.
- **Site lookup timing:** it runs in the background only when the track or session changes.
- **Reloading:** `media_brands.txt` is reloaded when it changes.

### media_brands.txt format
`name | match | colour | logo | source`, plus an optional `label=XX` at the end of `source`.

It's the agreed `media_colors.txt` with a `logo` column added:
- `app` for the `[apps]` entries
- the site's icon URL for the `[sites]` entries, from the colour research. Entries without a usable icon URL get it filled in during stage 4.

## 8. Settings (`media` section, defaults)

```json
"media": {
  "compact": false,
  "max_width": 136,
  "menu_panel_width": 300,
  "click_panel_width": 360,
  "hide_after_pause_s": 120,
  "fade_ms": 300,
  "skip_seconds": 15,
  "tint": "brand",
  "tint_strength": 1.0,
  "live_color": "#FF4E45",
  "start_with_windows": false
}
```

The shared `dock` settings (font, gap, timings, hover delays, and so on) apply as in the other widgets.

## 9. Library changes (taskbar-dock), each with tests
- **`panel`:** a way to report "this click was on a strip button, don't toggle the panel". Also whatever a pinned hover panel needs to grow into the click panel, if anything.
- **`marquee` (new):** the scrolling-text rule as plain functions. Given text width, box width, time and the look, it returns the offset and edge fades. Plus a helper to draw it into a DIB.
- **`draw::progress_ring` (new):** a smooth ring along a rounded square, for a fraction from 0 to 1, with fill and track colours.

## 10. Build milestones (each ends with a check by you)
1. **Strip:** docking, now playing (title, artist, cover), the progress ring, the three buttons including ±15 s, following the playing session, hide after a pause, compact mode, the right-click menu.
2. **Panels:** the hover panel, the click panel, scrolling text, the progress bar and seeking, and hover-to-click growth.
3. **Volume:** linking a session to its audio entry, the slider and mute, live updates, and skipping FxSound.
4. **Tint and logos:** the brand file, Firefox site detection, fade on play and pause, the theme and high contrast handling, the logo cache and the badge fallback.
5. **Polish:** Start with Windows, Explorer restart recovery, the fallback window, the README, and a cleanup pass.

## 11. Tests
- **Unit tests (no windows):**
  - which session is shown
  - hide-after-pause timing
  - timeline validity and counting forward
  - choosing the skip method
  - the brand file parser and the "last match wins" rule
  - colour lookup order and the cover fallback order
  - badge labels
  - strip layout and which button was hit
  - the button-click event in `PanelBehaviour`
  - marquee timing
  - progress ring geometry
- **On screen, with Spotify and Firefox:** measure with UI Automation and screenshots, including the text rows against the tray clock (4.1) and both panels' fixed sizes. Clicks go only on the widget, its panels and its own menu. System keys are never simulated.
- **Not re-tested:** live web data and logos (cached copies are used). The probe scripts from this chat (`probe.ps1`, `seektest.ps1`, `audiotest.ps1`, `tabtest.ps1`) are kept in `tests\` for manual checks.

## 12. Known limits
- **No ring or progress bar** for Firefox while playing, unless a seek has just happened.
- **Firefox volume** changes all Firefox sound, not just one tab.
- **Spotify volume** is Windows' volume for Spotify, separate from Spotify's own slider.
- **Same title in two tabs:** the site is still found, but which tab is playing can't be told.
- **Site title patterns:** only YouTube's was verified live. Spotify's web player may not show its name in the tab title.
- **Discord** is not supported.
- **Resuming a stream in a minimised Firefox window:** Firefox accepts "play" (and "toggle") but the page doesn't play. Twitch pauses itself while its page is hidden (Page Visibility API), so it likely won't play until the window is visible again. Seen 2026-10-09; the widget logs it ("play not done after 2 s").
