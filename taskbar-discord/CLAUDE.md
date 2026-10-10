# taskbar-discord: Discord voice chat in the Windows 11 taskbar

@../taskbar-dock/CLAUDE.md

Rust, `windows` crate (structure copied from taskbar-weather). README.md has setup and settings.
Runs straight from `target\release\taskbar-discord.exe` (dev.ps1 stops it before building).
Settings section `discord`; start with Windows via the shared `startup` list (name "discord").
Log: `%APPDATA%\TaskbarDock\taskbar-discord.log` (never tokens or the secret).

Data: Discord's official local RPC (`\\.\pipe\discord-ipc-N`), with the user's own Discord app
(client_id/secret). Chosen 2026-10-09 over TaskbarWidgets' way (UI Automation of Discord's window
+ the `discord-webrtc_0` log): that log only gets stats every 10 s here, so speaking was ~10 s late.
The strip exists only during a call, or when the user must act (setup, sign-in, error), or with
`show_when_idle`.

## Files (src/)
- main.rs: `App` state, constants, window procedures (main, strip, panel), `on_voice`, `main`.
- rpc.rs: the worker thread: pipe frames, sign-in state machine, subscriptions, commands
  (`Cmd`: Mute, Deaf, Leave, SignIn, Reconnect), test aid `TASKBAR_DISCORD_TEST=<n>` (fake call).
- voice.rs: `Voice`/`Channel`/`Member`/`Status` and applying RPC payloads (tested).
- auth.rs: OAuth exchange/refresh (discord.com/api/oauth2/token), token in
  `discord-token.bin` encrypted with DPAPI. Scopes rpc, rpc.voice.read, rpc.voice.write.
- avatars.rs: CDN download → `discord-avatars\` cache → 64 px decode; circle/ring drawing.
- label.rs: show/hide, drawing, dock slot, glide/drag/zoom (shared `strip::Motion`).
- flyout.rs: the panel (hover/click/pin, fade, resize with the call, Mute/Deafen/Leave buttons).
- menu.rs: right-click panel (shared `menu_panel`) and settings watcher.
- render.rs: strip and panel drawing, panel layout + hit test.

## Testing
- `TASKBAR_DISCORD_TEST=4` in the environment: no Discord needed; someone speaks in turn, the
  buttons work, Leave rejoins after 6 s. Probe: `tools\_tmp\discord_probe.ps1 strip|panel x y|shot`.
- Tested 2026-10-09 with the user's app: Authorize popup → token → AUTHENTICATE, real call (7 people),
  avatars, live speaking rings.
  Not tried yet: Mute/Deafen/Leave against real Discord (rpc.voice.write), token refresh after 7 days.
