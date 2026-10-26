# Next session (optional): sysmon GPU readings without NVML

Paste the block below into a new Claude Code session (working folder `C:\Tools`).

```
taskbar-sysmon uses ~22 MB private memory; NVML (NVIDIA's library) costs ~15 MB of that once
nvmlInit_v2 runs, and nvmlShutdown returns none of it (taskbar-sysmon\PLAN.md, "Open"). Replace
NVML with what Task Manager uses, if it holds up: D3DKMTQueryAdapterInfo (KMTQAITYPE_ADAPTERPERFDATA:
temperature, fan RPM, power as per-mille of the limit; NODEPERFDATA: engine clock) and PDH
"GPU Adapter Memory" for VRAM. GPU % keeps PDH's 3D-engine counters (pdh.rs already has them).

Ask me first about the panel changes this brings: power as % of the card's limit instead of
watts, fan in RPM instead of %, and whether to keep NVML as a fallback.

Steps:
1. Read taskbar-sysmon\CLAUDE.md, PLAN.md, src\nvml.rs, src\pdh.rs, src\sampler.rs and the
   `live_sources` test (sampler.rs, run with `cargo test --profile fast -- --ignored --nocapture
   live_sources`).
2. Prototype the D3DKMT reads first (a new ignored test printing every value) and compare each
   with HWiNFO64 (D:\HWiNFO64.exe, Sensors) on this PC's RTX 3070 before replacing anything.
3. Codex builds the swap once the values check out (brief: files, decisions, done-condition; it
   writes only taskbar-sysmon\src, UTF-8, no cargo fmt elsewhere).
4. Check: `taskbar-sysmon\dev.ps1`, `taskbar-dock\scripts\live\run.ps1 -Widgets sysmon`,
   `run.ps1 soak -Minutes 10 -Widgets sysmon` (report private memory before and after), status.ps1.
5. One Codex review, then ask me to commit and push.

Talk to me in plain, non-technical words.
```
