# taskbar-media: now-playing widget in the Windows 11 taskbar

@../taskbar-dock/CLAUDE.md

Rust, `windows` 0.58 crate (WinRT GSMTC + Win32). Design and status: PLAN.md (long; read the
section you need). Built in 5 stages; the user checks each on screen before saying go.
Runs from a copy: `taskbar-media.exe` here (dev.ps1 stops, copies, restarts). Log:
`taskbar-media.log` (also panics). Learned real-time table: `progress.txt`. Logo cache: `logos\`.

## Files (src/)
- main.rs: `App` state, `on_media`, strip fade, `create_strip` (attached, or the floating
  fallback; `--float` forces it), `redraw_strip`, `watch` (every second: look, config, brand file
  reload, Explorer restart, re-attach, full-screen hide), window procs, `main`. README.md: user docs.
- track.rs: what the strip shows: `identity` (source, picture, badge, tint colour),
  `strip_content`, live rule (`is_live`/`live_now`, `LIVE_SITES`), `buttons`, `button_cmd`.
- placement.rs: dock position, WinEvent hooks, and the wiring of the shared `taskbar_dock::strip::Motion` (glide, drag, zoom; screen x).
- media.rs: worker thread (MTA) over GSMTC: sessions, `one_per_app`, snapshots, per-object
  event hooks, cover cache with rechecks, commands (play/pause with Firefox retry, ±skip).
- timeline.rs (`Progress`, skip maths) · learned.rs (real-time yes/no per app) · select.rs
  (which session shows; hide after pause) · appear.rs (fade value).
- render.rs: strip drawing (text, ring, cover, tint) · layout.rs: strip geometry.
- popup.rs: the panel window (click panel and menu panel), hover/pin, volume, menu rows.
- panel_view.rs: panel layout and painting.
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
