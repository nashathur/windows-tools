# Plan: open ideas

The Block rebuild (steps 0–11) is done (2026-10-11): all four widgets run on Block, take their
look from `theme`, and dockctl configures every widget. The step notes, inventories, parity
checklist and token measurements were removed from this folder; they are in git history
(`git log -- taskbar-dock/plan`).

## 12. Settings app (decided 2026-10-12)

The control panel for every registered widget, with a Dock page and a Developer tab; dockctl
reaches every setting through the same library path. Decisions, contracts, jobs and checks:
`step12.md`.

## 12a. Windows Settings look (built, committed)

The settings app rebuilt with real Windows controls (WinUI 3, crate `settings-app\`):
`step12.md` "12a" and "12a build notes".

## 12b. Changes after trying the new app (next)

Widgets page on/off text, an Idle status, Windows Settings motion, highlight box settings for
every widget, "Bus stop", discord's avatar places, neighbours glide instead of jumping: `step12b.md`; session prompt
`step12b-session.md`.

## 13. One tray icon (idea)

The per-widget tray icons merge into one, owned by the launcher (`taskbar-dock-start.exe`,
always running): open the settings app, and a way back to a widget whose Show on taskbar is off.
Starts only once the settings app (step 12) has proven robust.
