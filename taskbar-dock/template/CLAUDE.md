# __NAME__: __DESC__

@../taskbar-dock/CLAUDE.md

Rust widget using Block. The settings section is `__SECTION__`; the log is
`%APPDATA%\TaskbarDock\__NAME__.log`.

## Files

- `src/main.rs`: `App` (`Content`), `Spec`, `describe()`, drawing and `main`.
- `dev.ps1`: wrapper for the shared build, test and scheduled-task start helper.
- `README.md`: user-facing overview.
- `app.manifest` and `build.rs`: DPI awareness and Windows compatibility manifest.
- Live-check extra: `..\taskbar-dock\scripts\live\extra\__SECTION__.ps1` (README.md "Live checks").
