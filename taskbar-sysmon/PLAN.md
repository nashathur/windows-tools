# taskbar-sysmon: plan

A CPU / GPU / RAM / network monitor docked in the Windows 11 taskbar, built on the shared `taskbar-dock` library (`C:\Users\thurs\tools\taskbar-dock`, see its README "Building a new widget" for the 16-step recipe and CLAUDE.md for the rules: five parts, corners, button highlights). It looks and behaves like the bus, weather, media and discord widgets.

Status: plan agreed in chat on 2026-10-10. Nothing built yet. **Don't start building until the user says go**; build in stages and let the user check each on screen before the next.

Visual reference: `mockup.html` in this folder (open it in a browser; random data, live updating). It's the agreed look for the strip, compact strip, threshold colours, panel and right-click panel.

## 1. Decisions

| Topic | Decision |
|---|---|
| Name | `taskbar-sysmon`; window class / dock name `sysmon`; settings section `sysmon`; startup-list name `sysmon` |
| Language | Rust, like the other widgets (no Python, no conda env) |
| Metrics | CPU, GPU, RAM, network. **No disk I/O.** No process lists, no WMI |
| Network units | Bytes: KB/s, MB/s (as Task Manager's Performance tab) |
| Strip width | Full mode about as wide as media and discord |
| CPU temperature | Read through **PawnIO** by an admin helper (section 4). Panel only, never on the strip |
| Stat switches | None: all four stats are always shown |

## 2. Look

Colours (one per metric, used on the strip and in the panel):

| Metric | Colour |
|---|---|
| CPU | blue `#85B7EB` |
| GPU | green `#97C459` |
| RAM | purple `#AFA9EC` |
| Network ↓ | teal `#5DCAA5` |
| Network ↑ | coral `#F0997B` |
| Warning | amber `#EF9F27` |
| Critical | red `#E24B4A` (lighter `#F0605F` on dark) |

Light theme: pick matching darker stops from `look.light`, like the other widgets. Text colours, fonts, radii and the hover highlight come from `Look` / `draw` as usual; nothing hard-coded beyond the metric colours.

### 2.1 Full strip
Four columns, two text rows matching the tray clock's baselines:

```
CPU 23%     GPU 41%     RAM 62%     ↓ 1.2 MB/s
[▁▂▅▃▂ ]    [▁▁▇▇▆ ]    [▅▅▅▅▅ ]    ↑ 80 KB/s
```

- Top row: grey label + white value (percent, whole numbers).
- Bottom row (CPU, GPU, RAM): a **sparkline inside a box** that always spans 0–100%, so the line's height reads as a share of the maximum. Box: dark fill, 1 px border, small radius (badge/chip role, via `draw`); faint dashed 50% line; area under the line filled at ~28% of the metric colour; line on top. Last 60 s, one point per second.
- Network column: no box; ↓ over ↑ with coloured arrows, auto units (KB/s below 1 MB/s, MB/s with one decimal).

### 2.2 Compact strip
Two columns, two rows, numbers with %:

```
C 23%   R 62%
G 41%   ↓1.2M
```
CPU top left, GPU bottom left, RAM top right, network ↓ bottom right (MB/s, not a percentage: a percentage of the Wi-Fi link speed would sit near 0–5% even at full internet speed).

### 2.3 Threshold colours
- CPU, GPU, RAM utilization: amber above **80%**, red above **95%**. The value **and** its sparkline (line + fill) change colour together.
- GPU temperature also turns the GPU value amber above its limit (suggested 80 °C).
- CPU temperature (panel only): amber at **80 °C**, red at **90 °C** (the 5600X's Tjmax is 90 °C).
- All thresholds in settings.

### 2.4 Panel (hover and left-click are the same panel)
Hover opens it without focus, a click pins it (`panel::PanelBehaviour`). Same content either way.

- **Top row:** title "System" on the left; on the right the **1m / 5m / 1h** range toggle and, beside it, the **Task Manager** button (opens `taskmgr.exe`).
- **Four cards** in a 2 × 2 grid, each with a boxed graph (0–100%, same style as the strip) over the chosen range:
  - **CPU · Ryzen 5 5600X**: utilization %, graph, per-thread bars (12), clock (GHz), **Temp** (°C, or "—" without the helper).
  - **GPU · RTX 3070**: utilization %, graph, VRAM used / total, temp · power, clock · fan.
  - **Memory**: in use %, graph, in use / total GB, committed / limit GB, cached GB.
  - **Network · <adapter>**: ↓ speed, ↓ and ↑ lines in one graph (auto-scaled, not 0–100%), link speed, session totals ↓ / ↑.
- **Nothing below the four cards** (no top processes, no extra tabs).
- 5m and 1h graphs downsample the history to the graph's pixel width (max or mean per pixel; max for the line so spikes stay visible).

### 2.5 Right-click panel (shared `menu_panel`)
- Compact mode (switch)
- Start with Windows (switch)
- separator
- Open Task Manager
- Edit settings…
- Quit

## 3. Data sources

| Metric | Source | Notes |
|---|---|---|
| CPU % | PDH `\Processor Information(_Total)\% Processor Utility` | Task Manager's number on Win11; fallback `GetSystemTimes` |
| Per-thread % | PDH `\Processor Information(*)\% Processor Utility` | 12 instances on the 5600X |
| CPU clock | PDH `% Processor Performance` × base MHz (`~MHz` from registry `HARDWARE\DESCRIPTION\System\CentralProcessor\0`) | |
| CPU temp | Shared memory from `sysmon-temp.exe` (section 4) | "—" if missing or older than 3 s |
| GPU util, VRAM, temp, power, clock, fan | NVML (`nvml.dll` from the NVIDIA driver, `LoadLibrary` at runtime) | Read-only queries of values the driver keeps; doesn't wake the GPU |
| GPU fallback | PDH `\GPU Engine(*)\Utilization Percentage` (3D engines summed per adapter) | Utilization only, any vendor, if NVML is missing |
| RAM | `GlobalMemoryStatusEx`; committed / cache from `GetPerformanceInfo` | |
| Network | `GetIfTable2`: `InOctets` / `OutOctets` deltas | Sum of adapters that are up; skip loopback, tunnel and virtual (Hyper-V / WSL / VPN) unless `include_virtual`; handle counter resets and adapters appearing/disappearing (no negative spikes) |
| Adapter name, link speed | `GetIfTable2` (`Alias`, `ReceiveLinkSpeed`) of the busiest adapter | |

Every source returns `Option`: a missing GPU or a failed counter shows "—" for that value instead of failing.

## 4. CPU temperature: PawnIO helper

Why a helper: Ryzen's Tctl is only readable from the kernel (SMN register `0x59800`, `THM_TCON_CUR_TMP`, through PCI config space). CPU-Z, HWiNFO64, AIDA64 etc. ship their own signed drivers; WinRing0 (old shared driver) is on Microsoft's vulnerable-driver blocklist. **PawnIO** is the signed, open-source replacement (used by LibreHardwareMonitor and FanControl) and has an AMD Zen module. The widget itself stays non-admin (an elevated window parented to Explorer's taskbar hits UIPI problems, and the Startup folder can't elevate).

- **Install PawnIO once**: official installer from the PawnIO site / GitHub. Ask the user before downloading; the user runs the installer (it installs a driver).
- **`sysmon-temp.exe`** (a second binary in this crate, `src/bin/sysmon-temp.rs`, no window):
  - loads PawnIO (`PawnIOLib.dll`) and its AMD family 17h/19h module;
  - reads Tctl once a second (5600X: Tctl = Tdie, no offset; keep the offset table for other Ryzens);
  - writes `{ temp_c: f32, time: u64, status }` to a named shared-memory block (`Local\TaskbarSysmon.Temp`, with a security descriptor that lets the non-admin widget read it);
  - one instance (named mutex); quits when the user signs out; logs to `sysmon-temp.log`;
  - target: ~1–2 MB RAM, ~0 CPU.
- **Start at sign-in**: a scheduled task "Taskbar sysmon temp", trigger at logon, "run with highest privileges" (user approves once when it's created). Not the Startup folder, not the Run key.
- **Widget side**: opens the block read-only every second; value older than 3 s, missing block or bad status → "Temp —". No error dialogs.
- **Check**: within 1 °C of HWiNFO64's "CPU (Tctl/Tdie)", idle and under load.
- HWiNFO shared-memory fallback: not planned (dropped 2026-10-10); easy to add later.

## 5. Architecture

Same structure as taskbar-discord / taskbar-weather (`windows` 0.58 crate, `taskbar-dock` dependency, `app.manifest` + `build.rs`, `dev.ps1`, `[profile.fast]`).

Files (src/):
- `main.rs`: `App` state, window procedures (strip, panel), timers, `main`.
- `sampler.rs`: worker thread, samples every 1 s into a history ring (3600 points per series = 1 h) behind a lock; the UI timer reads it.
- `pdh.rs` (CPU, per-thread, clock, GPU fallback) · `nvml.rs` (GPU) · `mem.rs` (RAM) · `net.rs` (adapters, deltas) · `temp.rs` (reads the helper's shared memory).
- `history.rs`: ring buffer, downsampling for 5m / 1h (tested).
- `format.rs`: percent, MB/s / KB/s, GB, °C formatting (tested).
- `render.rs`: strip drawing (columns, boxed sparklines, threshold colours), panel drawing (cards, graphs, toggle, button), panel layout + hit test.
- `label.rs`: show/hide, dock slot, glide/drag/zoom (shared `strip::Motion`).
- `flyout.rs`: the panel (hover/click/pin, fade, range toggle, Task Manager button).
- `menu.rs`: right-click panel (shared `menu_panel`) and settings watcher.
- `config.rs`: settings section `sysmon`.
- `bin/sysmon-temp.rs`: the PawnIO helper.

Settings (`sysmon` section, defaults):
```json
{
  "compact": false,
  "range": "1m",
  "warn_pct": 80,
  "crit_pct": 95,
  "gpu_temp_warn": 80,
  "cpu_temp_warn": 80,
  "cpu_temp_crit": 90,
  "include_virtual": false
}
```

## 6. Cost budget

| Reading | Cost per second |
|---|---|
| PDH query (CPU, per-thread, clock) | well under 1 ms |
| `GlobalMemoryStatusEx` / `GetPerformanceInfo` | microseconds |
| NVML (~5 calls) | ~1 ms |
| `GetIfTable2` | well under 1 ms |
| Temp (shared-memory read) | microseconds |

- History: 1 h × ~8 series × 4 bytes ≈ 100 KB.
- Strip redraws at most once a second, only when a value or sparkline pixel changes; panel graphs are drawn only while the panel is open.
- Targets: widget ~8–12 MB RAM and under 0.1% CPU on average (for reference, measured 2026-10-10: media 3.7 MB, discord 8.2 MB, weather 13 MB, bus 13.9 MB working set; discord ≈ 0.08% CPU). Helper ~1–2 MB. Measure after stage 2 and stage 4.

## 7. Stages (user checks each on screen before saying go)

1. **Data layer**: samplers, history, formatting; unit tests (deltas, counter wrap/reset, downsampling, units). A `--dump` flag prints a sample per second; compare against Task Manager side by side (CPU within ~3%, network within ~5%, RAM exact).
2. **Strip**: full and compact modes, boxed sparklines, threshold colours, dock slot, drag, glide, theme, Explorer restart, one instance. Test aid `TASKBAR_SYSMON_TEST=1`: fake values with spikes past both thresholds.
3. **Panels**: hover/click panel (cards, graphs, range toggle, Task Manager button, fade, placement), right-click panel, settings watcher, Start with Windows.
4. **CPU temperature**: user installs PawnIO; `sysmon-temp.exe`, shared memory, scheduled task; Temp row in the CPU card; check against HWiNFO64.
5. **Ship**: full-LTO release build, add `sysmon` to the `startup` list (through a one-off scheduled task, because Claude sessions see a virtualized `%APPDATA%`), README.md and CLAUDE.md for this widget, add it to taskbar-dock's widget list. Measure RAM/CPU against the budget.

## 8. Open points
- Exact strip width in px: match media/discord's measured width when building stage 2.
- GPU temperature limit for amber: 80 °C suggested, confirm when checking stage 2.
- Light-theme stops for the metric colours: pick during stage 2 and show the user.
