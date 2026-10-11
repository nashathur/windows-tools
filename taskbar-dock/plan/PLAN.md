# Plan: open ideas

The Block rebuild (steps 0–11) is done (2026-10-11): all four widgets run on Block, take their
look from `theme`, and dockctl configures every widget. The step notes, inventories, parity
checklist and token measurements were removed from this folder; they are in git history
(`git log -- taskbar-dock/plan`).

## 12. Settings app (idea, 2026-10-10)

Edit settings… opens the settings app instead of notepad.

- The per-widget tray icons go and merge into one tray icon for the settings app, which manages
  every widget.
- One settings app for every widget, like a library: a page per widget where the user can
  configure anything. Each widget describes its settings (label, type: switch / number with
  range / text / choice, help line); the app draws a generic page from that, so a new widget
  appears on its own and its page always matches its real settings. The standard ones (Show on
  taskbar, Compact mode, Start with Windows) look the same on every page.
- A Rust exe in taskbar-dock using the `theme` module (looks like the panels). "Edit settings…"
  (Block's standard row, one change) opens it at that widget's page, e.g.
  `taskbar-dock-settings.exe --widget weather`; notepad stays as a fallback.
- Writes go through `settings::update` (lock); widgets already follow file edits live (Block's
  watch). dockctl's `keys` already lists each widget's settings, a starting point for the
  descriptions.
