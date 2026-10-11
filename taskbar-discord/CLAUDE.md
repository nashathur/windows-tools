# taskbar-discord: Discord voice chat in the Windows 11 taskbar

@../taskbar-dock/CLAUDE.md

Rust, `windows` crate. README.md has setup and settings.
Runs straight from `target\release\taskbar-discord.exe` (dev.ps1 stops it before building).
Settings section `discord`; start with Windows via the shared `startup` list (name "discord").
Log: `%APPDATA%\TaskbarDock\taskbar-discord.log` (never tokens or the secret).

Data: Discord's official local RPC (`\\.\pipe\discord-ipc-N`), with the user's own Discord app
(client_id/secret), not TaskbarWidgets' way (UI Automation of Discord's window
+ the `discord-webrtc_0` log): that log only gets stats every 10 s here, so speaking was ~10 s late.
The strip shows during a call, or when the user must act (setup, sign-in, error), while Block's
Show on taskbar switch is on (`Content::visible`); otherwise Block shows the tray icon (headset).

## Files (src/)
Everything about the strip window, dock, panels, tray icon and the standard menu rows is Block's
(`..\taskbar-dock\BLOCK.md`); discord supplies the `Content`.
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
  `settings_changed` (new credentials reconnect; media's `max_width` caps the avatar column).
- render.rs: strip layout + drawing into Block's canvas (`strip`, `StripLayout::block_hit`/`cover`;
  the avatar column as wide as everyone in the call (`stack_width`; `fixed_width`: always the max),
  wider when Block's `fit_width` asks (a width held or gliding: `fit_column` widens up to the max);
  the avatar stack sits at the left end of its column, by the logo; honours `Frame::fit_width`
  in a call: `fit_column` narrows the column, then the gaps, and the full stack is cut off at the
  column's right end and faded by how much of the column is left (`over`), so a compact switch keeps
  the buttons right and fades evenly both ways),
  `tint`, `tray_logo`, panel layout + hit test, `paint_panel`.
- config.rs: the `discord` section; `save` merges (Block owns `compact` and `show_on_taskbar`).
- open.rs: brings Discord's window to the front (the logo's click).

## Testing
- `TASKBAR_DISCORD_TEST=4` in the environment: no Discord needed; someone speaks in turn, the
  buttons work, Leave rejoins after 6 s; every ~9 s people leave or join (n, 1, about half, n, …:
  the strip's width glides with them). The third person is muted. dev.ps1 passes every `TASKBAR_*` variable on to the
  scheduled task that starts the widget (`$env:TASKBAR_DISCORD_TEST='4'; .\dev.ps1 -Fast`).
- dockctl: `dockctl discord state|panel open|menu rows|menu click "Leave call"|tray click left|…`.
  Probe (mouse on the strip / panel, put back): `tools\_tmp\discord_probe.ps1 strip|panel x y|shot`;
  the tray icon in the overflow: `_tmp\overflow_test.ps1 -Widget discord -Match Discord`.
- Tested 2026-10-09 with the user's app: Authorize popup → token → AUTHENTICATE, real call (7 people),
  avatars, live speaking rings.
  2026-10-11 (on Block): Mute/Deafen/Leave against real Discord (rpc.voice.write) work.
  Not tried yet: token refresh after 7 days.
