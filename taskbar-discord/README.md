# taskbar-discord

Your Discord voice call on the Windows 11 taskbar, docked with the bus, weather and media widgets
(`taskbar-dock`). It appears when you join a voice channel and goes away when you leave.

- **Strip**: avatars of the people in the call (green ring = speaking, red dot = muted or
  deafened, "+N" past `max_avatars`), with the channel name and "N in call · Muted".
- **Compact**: a headset (green while anyone speaks, red dot while you're muted) and the count.
- **Hover / left click**: the panel lists everyone with their mute/deafen state, and has
  Mute, Deafen and Leave buttons.
- **Right click**: Mute, Deafen, Leave call, Compact mode, Show when not in a call, Start with
  Windows, Sign in to Discord, Reconnect, Edit settings…, Quit.

## Setup (once)

Discord only gives voice information to an application you registered yourself. It's free:

1. Open https://discord.com/developers/applications and choose **New Application** (any name).
2. **OAuth2**: copy the **Client ID**; **Reset Secret** and copy the **Client Secret**.
3. **OAuth2 → Redirects**: add `http://localhost` and save.
4. Right-click the widget → **Edit settings…** and fill in the `discord` section:
   ```json
   "discord": { "client_id": "…", "client_secret": "…", ... }
   ```
   Saving the file is enough; the widget reconnects.
5. Discord shows an **Authorize** popup once: click Authorize.

The token is stored encrypted for your Windows account (`%APPDATA%\TaskbarDock\discord-token.bin`)
and refreshed automatically. Only you (the app's owner) can sign in to such an app over RPC,
which is all this needs.

## Settings (`discord` section of `%APPDATA%\TaskbarDock\settings.json`)

| Key | Default | Meaning |
|---|---|---|
| `client_id`, `client_secret` | "" | Your Discord application's OAuth2 credentials. |
| `redirect_uri` | `http://localhost` | Must be one of the app's OAuth2 redirects. |
| `compact` | false | Compact strip. |
| `show_when_idle` | false | Keep the strip when you're not in a call. |
| `max_avatars` | 5 | Avatars on the strip before "+N". |

## Building

`dev.ps1` (see `taskbar-dock\CLAUDE.md`): build, test, restart. `-Fast` while editing.
