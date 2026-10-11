# taskbar-media: now-playing widget in the Windows 11 taskbar

@../taskbar-dock/CLAUDE.md

Rust, `windows` 0.58 crate (WinRT GSMTC + Win32). Design and status: PLAN.md (long; read the
section you need). Built in 5 stages; the user checks each on screen before saying go.
Runs from a copy: `taskbar-media.exe` here (dev.ps1 stops, copies, restarts). Log:
`taskbar-media.log` (also panics). Learned real-time table: `progress.txt`. Logo cache: `logos\`.

## Files (src/)
Everything about the strip window, dock, panels, tray icon and the standard menu rows is Block's
(`..\taskbar-dock\BLOCK.md`); media supplies the `Content`. Floating: `--float` or
`TASKBAR_MEDIA_FLOAT=1`.
- main.rs: `App` (the `block::Content`: strip `measure`/`draw` (cached `Geo`), `hit` (buttons,
  cover), `cover`, `tint` (brand colour, fading with playback), `visible` (a session shown),
  worker posts in `on_message` (`WM_MEDIA` → `on_media` → `reselect`; `WM_LOGO`), content timers
  (`watch` every second: live confirm, brand file reload, hide after pause; ring pulse; tint fade;
  panel frames), right-click rows (Open app, Copy), tray logo + tooltip), the `Spec`, `main`.
  README.md: user docs.
- track.rs: what the strip shows: `identity` (source, picture, badge, tint colour),
  `strip_content`, `strip_tint`, `ring`, live rule (`is_live`, `LIVE_SITES`), `buttons`, `button_cmd`.
- media.rs: worker thread (MTA) over GSMTC: sessions, `one_per_app`, snapshots, per-object
  event hooks, cover cache with rechecks, commands (play/pause with Firefox retry, ±skip).
- timeline.rs (`Progress`, skip maths) · learned.rs (real-time yes/no per app) · select.rs
  (which session shows; hide after pause).
- render.rs: strip drawing into Block's canvas (`geo`, `strip_layout` incl. `Frame::fit_width`,
  `draw_strip`: text, buttons, cover/badge, ring, `text_width`), `tray_logo`, GDI helpers ·
  layout.rs: strip geometry (`at_width`: the text column narrows first, or widens up to
  `max_width`; `BUTTON_GAP` after the text). The text column follows the track (`App::text_column`, plan step 9;
  `fixed_width`: always `max_width`); Block glides the width (`Spec::glide_width`). A line
  longer than its column scrolls (`panel_view::paint_line`, shared with the panel; content timer
  `TIMER_SCROLL` 33 ms while one is too long, started / stopped in `sync`).
- panel.rs: the click panel's content on Block's panel: size ("Nothing playing" without a
  session), mouse (buttons, seek drag, volume drag, mute), volume kept current, frame timer.
- panel_view.rs: click panel layout and painting, and the "Nothing playing" panel.
- audio.rs (per-app volume via WASAPI) · apps.rs (AUMID → name, window, activate) ·
  sites.rs (browser tab title via UI Automation) · brands.rs (`media_brands.txt`) ·
  logos.rs (logo download/cache, app icons) · iconurl.rs · cover.rs (square, dominant colour) ·
  config.rs (settings section `media`).
- tests/*.ps1: geometry/click probes (clicks only inside our own windows).

## Firefox / GSMTC quirks (don't re-diagnose)
- While playing, Firefox reports position 0 (or tiny) and no length; real values only when
  paused; a stale report right after play. So progress is trusted only for apps learned
  real-time (`progress.txt`: Firefox no, Spotify yes).
- Live = Twitch/Kick tab, or (non-real-time apps) paused with no length for 2 s after having
  played (Firefox reports paused + no length briefly while a video loads); per track key,
  dropped again if the track reports a real length while paused (`track::update_live`).
- Firefox AUMID `308046B0AF4A39CB`; several sessions under one id; tab switches create new
  session objects; title arrives before the picture (rechecks at 1.5 s and 4 s).
- Play in a minimized Firefox may not take: bring the window up and retry (done in media.rs).
- Twitch pauses itself when hidden.
- DrawTextW crashes on empty strings: never draw or measure empty text.

## Testing etiquette
Don't start or stop the user's music without reason; ask them to press play when needed.
