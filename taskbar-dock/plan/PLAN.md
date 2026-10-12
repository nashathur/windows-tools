# Plan: open ideas

The Block rebuild (steps 0–11) is done (2026-10-11): all four widgets run on Block, take their
look from `theme`, and dockctl configures every widget. The step notes, inventories, parity
checklist and token measurements were removed from this folder; they are in git history
(`git log -- taskbar-dock/plan`).

## 12. Settings app (decided 2026-10-12)

The control panel for every registered widget, with a Dock page and a Developer tab; dockctl
reaches every setting through the same library path. Decisions, contracts, jobs and checks:
`step12.md`.

## 12a. Windows Settings look (next)

The settings app made to look like Windows 11 Settings (three-entry nav, drop-downs, Windows
accent); a WinUI 3 test decides between real Windows controls and our own redrawn with Direct2D:
`step12.md` "12a".

## 13. One tray icon (idea)

The per-widget tray icons merge into one, owned by the launcher (`taskbar-dock-start.exe`,
always running): open the settings app, and a way back to a widget whose Show on taskbar is off.
Starts only once the settings app (step 12) has proven robust.
