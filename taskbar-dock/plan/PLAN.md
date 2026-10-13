# Plan: open ideas

The Block rebuild (steps 0–11) is done (2026-10-11): all four widgets run on Block, take their
look from `theme`, and dockctl configures every widget. The step notes, inventories, parity
checklist and token measurements were removed from this folder; they are in git history
(`git log -- taskbar-dock/plan`).

## 12, 12a, 12b. Settings app (done 2026-10-13)

The settings app (WinUI 3, crate `settings-app`) for every registered widget, plus Idle,
Highlight box settings and gliding strips for every widget: `step12.md` (decisions, 12a build
notes), `step12b.md` (the 12b changes).

## 12c. Open checks (done 2026-10-13)

The live checks and small decisions left by 12 and 12b: `step12b.md` "12c". One idea left there
("Still open": media's site colour for every tab).

## taskbar-sysmon (done 2026-10-13)

Spec and measurements: `..\taskbar-sysmon\PLAN.md`. It added the choice row to `menu_panel`
(`Row::choice`, `Content::menu_choice`, dockctl `menu click <row> <option>`).

## 13. One tray icon (idea)

The per-widget tray icons merge into one, owned by the launcher (`taskbar-dock-start.exe`,
always running): open the settings app, and a way back to a widget whose Show on taskbar is off.
Starts only once the settings app (step 12) has proven robust. Session prompt: `step13-session.md`.

Also open: sysmon's GPU readings without NVML (memory), prompt `..\taskbar-sysmon\gpu-session.md`.
