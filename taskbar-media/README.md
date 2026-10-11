# Taskbar Media

A "now playing" widget in the Windows 11 taskbar, written in Rust (Win32 + GDI, no web view;
about 600 KB). It docks next to the other taskbar widgets (bus, weather) through the shared
library in `..\taskbar-dock`, and looks and behaves like them. Design notes and history: PLAN.md.

## What it shows

- **Strip** (in the taskbar, left of the tray): the cover with a progress ring around it, the
  title and artist, and three buttons: previous / play-pause / next, or −15 s / play-pause /
  +15 s for a single video. It follows whatever is playing, keeps a paused track for 2 minutes,
  then fades out (and fades in when something plays). `compact: true` shows only the cover and
  buttons. Crowded, its text column narrows first.
- **Ring**: the position when the app reports it live (Spotify). Full and slowly pulsing when it
  doesn't (Firefox while playing). Red for a live stream (a Twitch/Kick tab, or a track without a
  length).
- **Click panel**: hover the strip (opens without focus, closes when the mouse leaves) or click it
  (stays open; Esc closes it). Cover, title, artist, album, app, a progress bar you can click to
  seek, the buttons, and the app's volume and mute (its Volume Mixer entries on every device).
  A click in a hover-opened panel pins it.
- **Right-click panel** (the one every widget shares): the track as its header, Open *app*, Copy
  "Artist – Title", then Show on taskbar, Compact mode, Start with Windows, Edit settings…, Quit.
- **Tray icon** (a play logo): shown while the strip isn't (Show on taskbar off, or nothing
  playing). Left click opens the click panel ("Nothing playing" when that's so), right click the
  right-click panel.
- **Tint**: the strip and panels take the app's or site's brand colour while playing (YouTube red,
  Spotify green, …): stronger while playing, quieter while paused, strongest under the mouse (the same levels as discord; set in the `dock` section: `tint_dark`, `tint_light`, `tint_fixed`). For browsers the playing tab is found by UI Automation,
  so the panel says "YouTube · Firefox".
- **No cover**: the site's or app's logo, else a badge in the brand colour, else ♪.

Supported players: Spotify (desktop app) and Firefox are tested; anything that reports to
Windows' media controls should work. Discord doesn't report to Windows, so it isn't shown.

## Files

| | |
|---|---|
| `taskbar-media.exe` | the copy that runs (dev.ps1 replaces it), started at sign-in when Start with Windows is on |
| `taskbar-media.log` | events and errors, also crashes ("crashed: …") |
| `media_brands.txt` | brand colours and logos for apps and sites; edits apply within a second (format in its header) |
| `progress.txt` | learned per app: does it report its position while playing? |
| `logos\` | downloaded site logos (refreshed after 30 days) |
| `tests\*.ps1` | probes for manual checks (geometry, clicks on the widget only, seeking, volume) |

## Settings

Section `media` in `%APPDATA%\TaskbarDock\settings.json` (Edit settings… in the menu). Changes
apply within a second, without a restart. Missing keys are added with these defaults:

```json
"media": {
  "compact": false,
  "show_on_taskbar": true,
  "max_width": 136,
  "fixed_width": false,
  "click_panel_width": 360,
  "hide_after_pause_s": 120,
  "skip_seconds": 15,
  "tint": "brand",
  "tint_strength": 1.0,
  "live_color": "#FF4E45"
}
```

`compact` and `show_on_taskbar` are the right-click panel's switches (the library's keys). Since
the move to the library (2026-10-11) `menu_panel_width`, `fade_ms` (now the shared `glide_ms`)
and `start_with_windows` (now only the shared startup list) are gone; they're removed from the
file on start.

- `max_width`: the text column's widest (px at 100%); a longer title or artist scrolls, as in
  the panel (waits 1.5 s, then loops; cut with "…" when Windows' animations are off). The column
  is as wide as the longer of title and artist, the buttons right after it, so the strip changes width
  with the track: it glides there at once, the neighbouring widgets following.
- `fixed_width`: true keeps the text column at `max_width` (the strip never changes width
  between tracks).
- `tint`: `"brand"` (media_brands.txt, else the app icon's colour), `"cover"` (the cover's main
  colour) or `"off"`. Off anyway in high contrast.
- `skip_seconds`: the −/+ buttons for single videos.

The shared `dock` section (font, gap, timings, hover delays) applies as in the other widgets.

## Build and run

```powershell
.\dev.ps1            # build, test, copy to taskbar-media.exe, restart it if it was running
.\dev.ps1 -Fast -Start
```

Start with Windows uses the shared startup list (`taskbar-dock-start.exe`), like the other
widgets. Only one copy runs at a time.

## Taskbar embedding and recovery

The strip is a layered child window of the taskbar (`Shell_TrayWnd`), placed in the shared dock
stack against the tray. Drag it to reorder the widgets.

- **Explorer restarts** destroy the taskbar and the strip with it. The widget checks every
  second (and on `TaskbarCreated`) and creates the strip again in the new taskbar.
- **Fallback**: if attaching to the taskbar fails, or there has been no taskbar for 10 seconds
  (another shell, Explorer not running), the strip becomes a floating topmost window left of
  the tray and the docked widgets (bottom right without a taskbar), with a taskbar-coloured backdrop. It hides while
  a full-screen app is in front, can't be dragged, and attaches to the taskbar as soon as one is
  back (after a failed attach: retried every minute). `taskbar-media.exe --float` forces this
  mode, for testing (or `TASKBAR_MEDIA_FLOAT=1` in the environment of `dev.ps1`).

## Known limits

- Firefox reports no position while playing, so its ring pulses instead of moving, and its
  volume is all of Firefox, not one tab.
- Spotify's volume here is Windows' volume for Spotify, separate from Spotify's own slider.
- After a widget restart during a stream, Firefox only reports it as live after pause and play.
- Play in a minimised Firefox may not take; the widget brings the window up and retries.
  Twitch pauses itself while its page is hidden.
