# taskbar-discord

Your Discord voice call on the Windows 11 taskbar, docked with the bus, weather and media widgets
(`taskbar-dock`). It appears when you join a voice channel and goes away when you leave; outside a
call (or with Show on taskbar switched off) a headset icon in the tray opens the same panels.

- Strip: up to `max_avatars` avatars (green ring = speaking), "+N" for everyone else. When
  everyone fits, everyone has an avatar, muted or not. In a bigger call the places are competed
  for: muted and deafened people don't get one; whoever speaks takes the place of the one quiet
  longest, and the places keep their order (no reshuffling). The strip is as wide as the
  avatars of everyone in the call (gliding as people join or leave), at most as wide as the
  media strip at its widest (media's `max_width`), which it reaches only with a "+N".
- Compact: the server logo with its ring and the Mute, Deafen and Leave buttons (no avatars).
- Hover / left click: the panel lists everyone with their mute/deafen state, and has
  Mute, Deafen and Leave buttons.
- Right click: Mute, Deafen, Leave call, Reconnect, Show on taskbar, Compact mode, Start with
  Windows, Edit settings…, Quit; Sign in to Discord only while signed out.
- Tray icon (shown whenever the strip isn't): left click the panel, right click the menu.

## Setup (once)

Discord only gives voice information to an application you registered yourself. It's free:

1. Open https://discord.com/developers/applications and choose New Application (any name).
2. OAuth2: copy the Client ID; Reset Secret and copy the Client Secret.
3. OAuth2 → Redirects: add `http://localhost` and save.
4. Right-click the widget → Edit settings… and fill in the `discord` section:
   ```json
   "discord": { "client_id": "…", "client_secret": "…", ... }
   ```
   Saving the file is enough; the widget reconnects.
5. Discord shows an Authorize popup once: click Authorize.

The token is stored encrypted for your Windows account (`%APPDATA%\TaskbarDock\discord-token.bin`)
and refreshed automatically. Only you (the app's owner) can sign in to such an app over RPC,
which is all this needs.

## Settings (`discord` section of `%APPDATA%\TaskbarDock\settings.json`)

| Key | Default | Meaning |
|---|---|---|
| `client_id`, `client_secret` | "" | Your Discord application's OAuth2 credentials. |
| `redirect_uri` | `http://localhost` | Must be one of the app's OAuth2 redirects. |
| `compact` | false | Compact strip. |
| `show_on_taskbar` | true | The strip on the taskbar during a call; off: only the tray icon. |
| `max_avatars` | 5 | Avatar places on the strip (see Strip above); the rest is "+N". |
| `fixed_width` | false | true: the strip is always its widest (media's `max_width`), never changing width as people join or leave. |

## Building

`dev.ps1` (see `taskbar-dock\CLAUDE.md`): build, test, restart. `-Fast` while editing.
