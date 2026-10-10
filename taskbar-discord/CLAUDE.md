# taskbar-discord: Discord voice chat in the Windows 11 taskbar

@../taskbar-dock/CLAUDE.md

Rust, `windows` crate (structure copied from taskbar-weather). README.md has setup and settings.
Runs straight from `target\release\taskbar-discord.exe` (dev.ps1 stops it before building).
Settings section `discord`; start with Windows via the shared `startup` list (name "discord").
Log: `%APPDATA%\TaskbarDock\taskbar-discord.log` (never tokens or the secret).

Data: Discord's official local RPC (`\\.\pipe\discord-ipc-N`), with the user's own Discord app
(client_id/secret). Chosen 2026-10-09 over TaskbarWidgets' way (UI Automation of Discord's window
+ the `discord-webrtc_0` log): that log only gets stats every 10 s here, so speaking was ~10 s late.
The strip shows during a call, or when the user must act (setup, sign-in, error), while Block's
Show on taskbar switch is on (`Content::visible`); otherwise Block shows the tray icon (headset).

## Files (src/)
The strip window, dock, drag/zoom/glide, the panel's window (hover/click/pin, fade, placement), the
right-click panel's standard rows, the tray icon and following the settings file are the library's
`block` (plan step 5); discord supplies the `Content`.
- main.rs: `App` (the `block::Content`: strip `measure`/`draw`, `hit` (Mute/Deafen/Leave buttons,
  logo), `cover`, `tint` (blurple boxes), `visible`, ring pulse timer, worker posts in `on_message`
  (`on_voice`: redraw + `resize_panel`), panel size/draw/mouse, tray logo + tooltip), the `Spec`, `main`.
- rpc.rs: the worker thread: pipe frames, sign-in state machine, subscriptions, commands
  (`Cmd`: Mute, Deaf, Leave, SignIn, Reconnect), test aid `TASKBAR_DISCORD_TEST=<n>` (fake call).
- voice.rs: `Voice`/`Channel`/`Member`/`Status` and applying RPC payloads (tested).
- auth.rs: OAuth exchange/refresh (discord.com/api/oauth2/token), token in
  `discord-token.bin` encrypted with DPAPI. Scopes rpc, rpc.voice.read, rpc.voice.write.
- avatars.rs: CDN download → `discord-avatars\` cache → 64 px decode; circle/ring drawing.
- menu.rs: the right-click panel's own rows (Mute, Deafen, Leave; Reconnect; Sign in only while
  signed out) and
  `settings_changed` (new credentials reconnect; media's `max_width` sets the text column).
- render.rs: strip layout + drawing into Block's canvas (`strip`, `StripLayout::block_hit`/`cover`),
  `tint`, `tray_logo`, panel layout + hit test, `paint_panel`.
- config.rs: the `discord` section; `save` merges (Block owns `compact` and `show_on_taskbar`) and
  drops the old `show_when_idle` key.
- open.rs: brings Discord's window to the front (the logo's click).

## Testing
- `TASKBAR_DISCORD_TEST=4` in the environment: no Discord needed; someone speaks in turn, the
  buttons work, Leave rejoins after 6 s. dev.ps1 passes every `TASKBAR_*` variable on to the
  scheduled task that starts the widget (`$env:TASKBAR_DISCORD_TEST='4'; .\dev.ps1 -Fast`).
- dockctl: `dockctl discord state|panel open|menu rows|menu click "Leave call"|tray click left|…`.
  Probe (mouse on the strip / panel, put back): `tools\_tmp\discord_probe.ps1 strip|panel x y|shot`;
  the tray icon in the overflow: `_tmp\overflow_test.ps1 -Widget discord -Match Discord`.
- Tested 2026-10-09 with the user's app: Authorize popup → token → AUTHENTICATE, real call (7 people),
  avatars, live speaking rings.
  2026-10-11 (on Block): Mute/Deafen/Leave against real Discord (rpc.voice.write) work.
  Not tried yet: token refresh after 7 days.
- Block port parity (2026-10-11, fake call): strip 258×48 at the same place, panel 320×290 and menu
  280×380 at the same rects as before; panel pixels identical apart from who speaks.
