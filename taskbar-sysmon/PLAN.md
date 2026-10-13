# taskbar-sysmon: plan

A CPU / GPU / RAM / network monitor docked in the Windows 11 taskbar, on Block
(`..\taskbar-dock`: CLAUDE.md "Every widget has these five parts", BLOCK.md, UI-RULES.md, README.md
"Building a new widget"). Started from `taskbar-dock\scripts\new_widget.ps1`; Block owns the
window, panels, right-click panel, glide, tray icon, Idle, Highlight box and settings watching;
sysmon supplies the `Content`.

Status: built 2026-10-13 (strip, panels, temperature helper); live checks, 10-minute soak and a
comparison with Windows' counters passed. Visual reference: `mockup.html` (agreed look).
Measured: private memory ~22 MB (over the 8–12 MB target), CPU 0.0 %. Sign-in start of widget
and helper checked 2026-10-13.

Open (user to decide): NVML costs ~15 MB private once `nvmlInit_v2` runs (measured in a fresh
process: load nvml.dll, then `nvmlInit_v2`; `nvmlShutdown` returns none of it); without it sysmon would be
~7 MB. Alternative, as Task Manager does: `D3DKMTQueryAdapterInfo` (KMTQAITYPE_ADAPTERPERFDATA:
temperature, fan RPM, power as ‰ of the limit; NODEPERFDATA: engine clock) plus PDH `GPU Adapter
Memory` for VRAM; any GPU vendor. Panel changes: power as % of limit instead of W, fan in RPM.

## 1. Decisions

| Topic | Decision |
|---|---|
| Names | crate/exe `taskbar-sysmon`; class `TaskbarSysmonStrip`; section and startup name `sysmon`; title "System monitor" |
| Metrics | CPU, GPU, RAM, network (bytes: KB/s, MB/s). No disk, no process lists, no WMI; no stat switches |
| CPU temperature | panel only, from the PawnIO helper (section 4); "—" when the helper isn't running |
| Update speed | High ½ s, Normal 1 s (default), Low 4 s, Paused. Saved: `speed` = high/normal/low only; Paused is never saved (a restart resumes the last real speed) |
| Paused | stops measuring; the strip keeps its last values drawn in the dim colour; tray tooltip says paused |
| Idle (Show on taskbar off) | stops measuring (`Content::idle` pauses the sampler; graphs get a gap); Block's paused tooltip |
| Highlight box (`Content::tint`) | None (neutral) normally; theme warn / danger, active, while any value is over its warning / critical limit |
| Colours | metric colours in theme `BRANDS` (dark + light stop each): `sysmon.cpu` #85B7EB, `sysmon.gpu` #97C459, `sysmon.ram` #AFA9EC, `sysmon.down` #5DCAA5, `sysmon.up` #F0997B (light stops: darker, picked at build, shown to the user). Warning / critical: theme `status.warn` / `status.danger`. Nothing typed in the widget (`theme_lint`) |
| Strip width | fixed, measured for the longest values ("CPU 100%", "↓ 999.9 MB/s"; a width following the values twitched every second); at most `max_width` (setting, default 254 px at 100% = media's widest strip), the end and column spacing giving way first; regular-weight text only; label–value gap `space::XS`, arrow–number gap `space::XS / 2`, column gap and ends `space::TIGHT` |
| Crowded | `compact_when_crowded` on |
| CPU name / adapter name | read from Windows (registry `ProcessorNameString`, `GetIfTable2` alias), never typed |

## 2. Look (mockup.html)

### 2.1 Full strip
Four columns, two text rows on the tray clock's baselines:
```
CPU 23%     GPU 41%     RAM 62%     ↓ 1.2 MB/s
[▁▂▅▃▂ ]    [▁▁▇▇▆ ]    [▅▅▅▅▅ ]    ↑ 80 KB/s
```
- Top row: dim label + value (whole percent).
- Bottom row (CPU, GPU, RAM): sparkline in a box spanning 0–100% (dark fill, 1 px border, radius by
  role via `draw`); faint dashed 50% line; area under the line at ~28% of the metric colour; line on
  top. Last 60 s whatever the speed.
- Network: no box; ↓ over ↑ with coloured arrows; KB/s below 1 MB/s, MB/s with one decimal.

### 2.2 Compact strip
```
C 23%   R 62%
G 41%   ↓1.2M
```

### 2.3 Threshold colours
- CPU, GPU, RAM: warn above `warn_pct` (80), critical above `crit_pct` (95); value and sparkline
  change colour together.
- GPU temperature above `gpu_temp_warn` (80 °C) turns the GPU value warn.
- CPU temperature (panel): warn at `cpu_temp_warn` (80 °C), critical at `cpu_temp_crit` (90 °C).

### 2.4 Main panel (hover and left click; a click inside pins it, Block's)
- Row 1: "System" left; **Task Manager** button right (opens `taskmgr.exe`).
- Row 2: range **1m / 5m / 1h** (saved as `range`), speed **High / Normal / Low / Paused**,
  **Reset graphs** button.
- Four cards, 2 × 2, each with a boxed graph over the range:
  - **CPU · <name>**: %, graph, per-thread bars, clock (GHz), Temp (°C or "—").
  - **GPU · <name>**: %, graph, VRAM used / total, temp · power, clock · fan.
  - **Memory**: in use %, graph, in use / total GB, committed / limit GB, cached GB.
  - **Network · <adapter>**: ↓ speed, ↓ and ↑ lines in one auto-scaled graph, link speed, session
    totals ↓ / ↑.
- Nothing below the cards. 5m / 1h graphs downsample to the pixel width (max per pixel for the line).
- Buttons use `draw::button_box` geometry and highlights (UI-RULES.md).

### 2.5 Right-click panel
Own rows on top: **Open Task Manager**, **Reset graphs**, **Update speed** (a new shared
three/four-way choice row in `menu_panel`, High / Normal / Low / Paused); then Block's standard
rows (Show on taskbar, Compact mode, Start with Windows, Edit settings…, Quit).

Reset graphs (both panels): clears every history and the network session totals; keeps the speed.

## 3. Data sources

| Metric | Source | Notes |
|---|---|---|
| CPU % | PDH `\Processor Information(_Total)\% Processor Utility` | Task Manager's number; fallback `GetSystemTimes` |
| Per-thread % | PDH `\Processor Information(*)\% Processor Utility` | |
| CPU clock | PDH `% Processor Performance` × `~MHz` (registry `HARDWARE\DESCRIPTION\System\CentralProcessor\0`) | |
| CPU temp | shared memory from `sysmon-temp.exe` | "—" if missing or older than 3 s |
| GPU util, VRAM, temp, power, clock, fan | NVML (`nvml.dll`, `LoadLibrary` at runtime) | read-only |
| GPU fallback | PDH `\GPU Engine(*)\Utilization Percentage` (3D engines per adapter) | utilization only |
| RAM | `GlobalMemoryStatusEx`; committed / cache from `GetPerformanceInfo` | |
| Network | `GetIfTable2` `InOctets` / `OutOctets` deltas | adapters that are up; skip loopback, tunnel, virtual unless `include_virtual`; counter resets and adapters coming and going give no negative spikes |
| Adapter, link speed | `GetIfTable2` (`Alias`, `ReceiveLinkSpeed`) of the busiest adapter | |

Every source returns `Option`; a missing value shows "—".

## 4. CPU temperature helper (PawnIO)

PawnIO 2.2.0 is installed (`C:\Program Files\PawnIO\PawnIOLib.dll`, service `PawnIO`, manual start).
Modules 0.2.11 unpacked in `..\_tmp\pawnio\modules\` (`AMDFamily17.bin` covers Zen 1–4 incl. the
5600X, family 19h); the helper ships its module beside its exe.
- `temp\` crate (`sysmon-temp.exe`), no window, module embedded: loads PawnIOLib + the AMD module, reads Tctl (SMN `0x59800`)
  every second (5600X: no offset; keep the offset table for other Ryzens), writes
  `{ temp_c, time, status }` to `Local\TaskbarSysmon.Temp` (security descriptor readable by the
  non-admin widget); one instance (named mutex); quits at sign-out; logs to `sysmon-temp.log`;
  ~1–2 MB, ~0 CPU.
- Starts at sign-in: scheduled task "Taskbar sysmon temp", at logon, highest privileges (user
  approved; created when the helper works).
- Check: within 1 °C of HWiNFO64's "CPU (Tctl/Tdie)", idle and under load.

## 5. Code

`src/`: `main.rs` (`App`, `Spec`, `main`), `sampler.rs` (worker thread at the chosen speed into the
history; paused while idle or Paused; posts `WM_APP` to Block's main window), `pdh.rs`, `nvml.rs`,
`mem.rs`, `net.rs`, `temp.rs`, `history.rs` (time-stamped ring, 1 h, downsampling; tested),
`format.rs` (tested), `render.rs` (strip, compact, panel cards, hit test), `config.rs`.
The helper is its own crate, `temp\` (`sysmon-temp.exe`, installed to
`C:\Program Files\TaskbarSysmon\` so a normal process can't replace the elevated exe).

Settings (`sysmon`; Block owns `compact`, `show_on_taskbar`, highlight keys), shown on the
settings app page: `speed` "normal", `range` "1m", `warn_pct` 80, `crit_pct` 95, `gpu_temp_warn` 80,
`cpu_temp_warn` 80, `cpu_temp_crit` 90, `include_virtual` false.

dockctl `do`: `now` (all current values as JSON), `fake on|off` (test values with spikes past both
limits), `reset`. Live-check extra: `scripts\live\extra\sysmon.ps1`.

## 6. Cost budget
Each reading well under 1 ms per tick; history ~200 KB at High. Strip redraws only when a value or
sparkline pixel changes; panel graphs only while open. Targets: ~8–12 MB, under 0.1% CPU average at
Normal (`run.ps1 soak -Minutes 10 -Widgets sysmon`); helper ~1–2 MB.

## 7. Checks
`build_all.ps1 -Only lib taskbar-sysmon`; `scripts\live\run.ps1 -Widgets sysmon`;
`run.ps1 soak -Minutes 10 -Widgets sysmon`; `status.ps1`; side by side with Task Manager (CPU within
~3%, network ~5%, RAM exact); temperature against HWiNFO64. Then Codex reviews the whole change.
