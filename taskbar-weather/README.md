# Taskbar Weather

The current weather on the Windows 11 taskbar, on the shared `taskbar-dock` library (Block owns
the strip, dock, panels and tray icon; `..\taskbar-dock\BLOCK.md`).

- Strip: an icon, the temperature over the humidity (dimmer), the conditions beside them.
  Compact: the icon with temperature over humidity. `label_position` "auto" puts it next to the
  tray when the taskbar icons are left-aligned, else on the left side (not a dock member there).
- Flyout (hover on the strip, or left click on the strip or tray icon; the tray icon's hover
  shows only its tooltip): current conditions, the next 6
  hours, the next 5 days with temperature ranges and rain chance (shown from 10%, coloured from
  50%).
- Right-click panel: Refresh now, Fahrenheit, then Block's standard rows.
- Tray icon (a cloud logo) while the strip is off; its tooltip shows the weather.

## Data

- [Open-Meteo](https://open-meteo.com): no key, free for non-commercial use, CC BY 4.0 (the
  flyout shows the attribution). One call per `refresh_minutes`. Location by IP (ip-api.com)
  unless `city` or coordinates are set. `model` picks the forecast model (`best_match` by
  default; `knmi_seamless` = KNMI HARMONIE-AROME 2 km for 2.5 days, then ECMWF).
- KNMI (optional, Netherlands): with `knmi_api_key` (an EDR API key from the KNMI
  Developer Portal; the anonymous Open Data key doesn't work for EDR), current temperature,
  humidity and wind are the latest 10-minute measurement at the nearest station within 50 km
  (~4 requests/hour). Symbol, "feels like" and forecasts stay Open-Meteo. The flyout shows
  "Measured <station> <time>"; a rejected key or stale data shows a ⚠ note and falls back to the
  model.
- dockctl: `do now`, `do places <query>`, `do place <n | lat,lon | auto>` (taskbar-dock README).

## Settings (`weather` section of `%APPDATA%\TaskbarDock\settings.json`)

| key | default | meaning |
|---|---|---|
| `city` | `""` | City name, e.g. `"Jakarta"`. Empty: detect by IP. |
| `latitude`, `longitude` | `null` | Exact coordinates; override `city`. |
| `location_name` | `""` | Name shown with coordinates. |
| `units` | `"celsius"` | `"celsius"` or `"fahrenheit"` (also switches wind to mph). |
| `refresh_minutes` | `15` | Update interval (minimum 5). |
| `model` | `"best_match"` | Open-Meteo model, e.g. `"knmi_seamless"`, `"ecmwf_ifs025"`, `"icon_seamless"`. |
| `knmi_api_key` | `""` | KNMI EDR API key (secret: masked in logs and dockctl). |
| `knmi_station` | `""` | KNMI station, e.g. `"06260"` (De Bilt). Empty: nearest. |
| `label_position` | `"auto"` | `"auto"`, `"left"` or `"right"`. |
| `compact`, `show_on_taskbar` | `false`, `true` | Block's switches (right-click panel). |
