# Taskbar Weather

A small native Windows 11 app written in Rust (Win32 + GDI, no web view). It is about 600 KB and uses about 17 MB of RAM. It shows the current weather directly on the taskbar:

- **Taskbar label**: an icon, the temperature over the humidity (dimmer), with the conditions beside them, embedded in the taskbar, similar to the built-in Widgets weather. With left-aligned taskbar icons it sits on the right, next to the tray. It moves aside for other widgets docked into the taskbar.
- **Dock stack**: with the label on the right, it forms a stack against the tray boundary with other widgets that support it (the bus widget in `C:\Users\thurs\tools\buswidget`). Drag a widget to reorder: while you drag, the others close up; on release it snaps into the stack on the side where you let go of the mouse. The order is saved in `%APPDATA%\TaskbarDock\order.txt` (window class names, tray side first) and shared by both apps. Members mark their taskbar window with the window properties `TaskbarDock.Member` and, while dragged, `TaskbarDock.Dragging`.
- **Tray icon**: shown only when the taskbar label is turned off. A cloud logo in the taskbar's text colour; its tooltip shows the current weather.
- **Flyout**: left-click the label or the tray icon. It shows current conditions, the next 6 hours and the next 5 days with temperature ranges and rain chance.
- **Right-click panel** (on the label, the tray icon or the flyout): Refresh now; switches for Fahrenheit, Show on taskbar, Compact mode and Start with Windows (they flip and the panel stays open); Edit settings… and Quit.
- **Compact mode**: the label shows the symbol with the temperature over the humidity.

The app follows the Windows light/dark theme and per-monitor DPI. It restores itself when Explorer restarts.

## Data

[Open-Meteo](https://open-meteo.com) needs no API key or account. It is free for non-commercial use (under 10,000 calls a day). The data is licensed CC BY 4.0, and the flyout shows the attribution. The app makes one call every 15 minutes.
The location is detected from your IP address (ip-api.com) unless you set one.

**Forecast model.** By default Open-Meteo picks the best model for the location (`best_match`). Set `model` to choose one yourself. For example, `knmi_seamless` uses KNMI's HARMONIE-AROME (2 km grid, updated hourly) for the first 2.5 days and ECMWF after that. Outside the Netherlands it falls back to ECMWF.

**Measured "now" values from KNMI (optional, Netherlands only).** With a KNMI API key, the current temperature, humidity and wind come from the latest 10-minute measurement at the nearest KNMI station (within 50 km). The weather symbol, "feels like" and all forecasts still come from Open-Meteo. The flyout then shows "Measured De Bilt 13:50", and the tooltip shows the station.

To get a key:
1. Register at the [KNMI Developer Portal](https://developer.dataplatform.knmi.nl/apis) and request an **EDR API** key. It's free; the limit is 1,000 requests an hour, and the app uses about 4 an hour.
2. Paste it into `knmi_api_key`.
The shared anonymous Open Data API key does **not** work for EDR. If KNMI rejects the key or has no recent data, the flyout shows a ⚠ note and the app keeps using the model values.

## Build

```
cargo build --release
```
The output is `target\release\taskbar-weather.exe`. The build needs the MSVC toolchain. Keep the project at a short path, because MSVC `link.exe` fails on paths over 260 characters.

## Settings

Settings are in `%APPDATA%\TaskbarDock\settings.json`, one file shared with the bus widget. Open it with right-click → *Edit settings…* (either widget). Changes apply as soon as you save the file. It has three sections:

- `dock`: the shared look and behaviour for all taskbar widgets: gap, font, corner radius, animation timings, hover delays, and switches to follow Windows' time format, animation effects and hover time. These come from the shared `taskbar-dock` library (`C:\Users\thurs\tools\taskbar-dock`); its README lists every key.
- `weather`: this app's settings (table below).
- `bus`: the bus widget's settings (`agency`, `line`, `stop_code`, `label`, `dest`, `badge_color`, `badge_text`).

Each app only rewrites its own section. The dock order is kept separately in `%APPDATA%\TaskbarDock\order.txt`, because it is written automatically on every drop. The old `%APPDATA%\TaskbarWeather\config.json` was moved in on first run and kept as `config.json.migrated`.

Keys in the `weather` section:

| key | default | meaning |
|---|---|---|
| `city` | `""` | City name, e.g. `"Jakarta"`. Empty means detect by IP. |
| `latitude`, `longitude` | `null` | Exact coordinates. If set, they override `city`. |
| `location_name` | `""` | Name shown together with coordinates. |
| `units` | `"celsius"` | `"celsius"` or `"fahrenheit"` (Fahrenheit also switches wind to mph). |
| `refresh_minutes` | `15` | Update interval in minutes (minimum 5). |
| `model` | `"best_match"` | Open-Meteo model, e.g. `"knmi_seamless"`, `"dmi_seamless"`, `"ecmwf_ifs025"`, `"icon_seamless"`. |
| `knmi_api_key` | `""` | KNMI EDR API key. When set, measured current values are shown (see above). |
| `knmi_station` | `""` | KNMI station, e.g. `"06260"` (De Bilt) or `"06240"` (Schiphol). Empty means the nearest station. |
| `show_on_taskbar` | `true` | Show the label in the taskbar. If off, only the tray icon is shown. Also in the right-click menu. (Was `taskbar_label`, renamed once on start.) |
| `label_position` | `"auto"` | `"auto"`, `"left"` or `"right"`. |
| `compact` | `false` | Compact label: symbol, temperature and humidity. Also in the right-click menu. |

Tip: if you turn the label off, Windows 11 may hide the tray icon in the overflow menu (^). To keep it visible, turn on *taskbar-weather* in Settings → Personalization → Taskbar → Other system tray icons.

## How the taskbar embedding works

Windows 11 removed taskbar "deskbands", so the label is a layered child window parented to `Shell_TrayWnd`. TrafficMonitor uses the same approach. The label's pixels are drawn with per-pixel alpha, so it blends with the Mica taskbar. It follows its neighbours instantly: a WinEvent hook on every process that owns taskbar child windows (Explorer for the tray, other docked widgets such as the bus widget) repositions it within milliseconds when one of them moves, resizes, appears or disappears. It slides (150 ms ease-out) when moving right and jumps when it has to make room on the left. A 2-second watchdog remains as a backup and keeps it on top. It re-attaches after Explorer restarts.
