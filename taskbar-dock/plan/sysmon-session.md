# Next session: taskbar-sysmon

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).
Written 2026-10-13, at the end of the 12c session.

```
Build a new widget, taskbar-sysmon (a system monitor in the taskbar). I have a plan for it: ask
me for it first, then grill me on the open choices (what it shows in full and compact mode, the
panels, the menu actions, refresh rate, its automatic Highlight box colour) before building.

Where we are: 12c is committed and pushed. Every widget, dockctl, the launcher and the settings
app are installed and up to date (`taskbar-dock\scripts\status.ps1` reports nothing out of date).

How to build it:
1. Start from the starter: `taskbar-dock\scripts\new_widget.ps1 -Name taskbar-sysmon
   -Title "System monitor" -Description "..."` (README "Building a new widget"). This is the
   starter's first real use: check the starter itself as you go and fix the template for whatever
   it misses, so the next widget starts better.
2. Replace the template's counter with sysmon's content; every widget has the five parts, Idle and
   the Highlight box (taskbar-dock\CLAUDE.md "Every widget has these five parts", BLOCK.md).
3. Check with `taskbar-dock\scripts\build_all.ps1 -Only lib taskbar-sysmon`, then
   `scripts\live\run.ps1 -Widgets sysmon`, `run.ps1 soak -Minutes 10 -Widgets sysmon` (a monitor
   must not leak or use noticeable CPU itself), and `status.ps1`.
4. Codex builds once the plan is settled and reviews the whole change once at the end; then ask
   me to commit.

Read first: `taskbar-dock\CLAUDE.md`, `BLOCK.md`, `UI-RULES.md`, `README.md` ("Building a new
widget", "Building", "Live checks"), `plan\PLAN.md`, then the template
(`taskbar-dock\template\`) and the smallest widget (`taskbar-weather\`).
Talk to me in plain, non-technical words.
```
