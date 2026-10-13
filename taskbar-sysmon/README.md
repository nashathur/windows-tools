# taskbar-sysmon

System monitor for the Windows taskbar, built on taskbar-dock's Block. The full strip shows CPU,
GPU, RAM and download/upload rates with 60-second sparklines. Compact mode shows CPU, GPU, RAM and
download rate. The hover and click panel shows four graphs, system details, adapter/session totals,
and CPU temperature when the optional helper is running.

## Settings

Settings live in the shared `sysmon` section. Update speed is High (500 ms), Normal (1 s, default),
or Low (4 s); Paused stops sampling until resumed and is never saved. Graph range is 1m, 5m, or 1h.
Usage warning/critical defaults are 80%/95%; GPU temperature warning is 80 °C; CPU temperature
warning/critical defaults are 80 °C/90 °C. Virtual network adapters are skipped by default.
Maximum strip width (default 254 px at 100%, media's widest strip): the strip has one fixed width,
measured for its longest values, and tightens its spacing to stay within it.
Block owns compact mode, Show on taskbar, and highlight box settings.

## Data and CPU temperature

CPU utility comes from PDH with `GetSystemTimes` fallback. GPU readings use NVML when `nvml.dll` is
available; without it, GPU use comes from PDH's 3D-engine counters (other GPU values `—`). Memory
comes from Windows memory APIs; network rates count hardware interfaces only (never NDIS filter
layers, which would double the traffic) and ignore counter resets. Unavailable readings display
as `—`.

CPU temperature is read-only from `Local\TaskbarSysmon.Temp`, written by the separate
`sysmon-temp.exe` PawnIO helper (`temp\README.md`: install, contract, checks). Without that helper,
the panel displays `—`. This widget does not start it or create its scheduled task.

## Commands

`dockctl sysmon do now` returns current values and state as JSON. `do fake on|off` enables synthetic
threshold values for screenshots (not saved). `do reset` clears graph history and network session
totals while keeping the selected speed.
