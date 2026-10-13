# taskbar-sysmon: CPU, GPU, RAM and network taskbar monitor

@../taskbar-dock/CLAUDE.md

Rust with the `windows` 0.58 crate. `PLAN.md` records the settled product spec; `mockup.html` is
the approved layout. Settings section and startup name are `sysmon`. CPU temperature is read from
the separate PawnIO helper's shared memory; this widget does not manage that helper.

## Files (src/)

Block owns the strip window and shared panel/menu shell; sysmon supplies Block's `Content`.
- `main.rs`: `App`, strip/panel/menu actions, dockctl `now`, `fake`, `reset`, `Spec`, startup.
- `sampler.rs`: one worker thread, pause/idle/speed condition variable, sample snapshots, history
  synchronization and `WM_APP` notification.
- `pdh.rs`, `nvml.rs`, `mem.rs`, `net.rs`, `temp.rs`: CPU/GPU/memory/network/temperature sources.
- `history.rs`: timestamped one-hour series and pixel-width downsampling with gap breaks.
- `format.rs`: user-facing rates/percentages, threshold levels and CPU name shortening.
- `config.rs`: settings defaults, descriptions and reads/writes.
- `render.rs`: measured full and compact strips, graph cards, panel layout and tray glyph.

Run `cargo build --profile fast`, `cargo test --profile fast`, and from `taskbar-dock`,
`cargo test --profile fast --test theme_lint`.
