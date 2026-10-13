# Next session: step 13, one tray icon

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).

```
Step 13: one tray icon for all the taskbar widgets (taskbar-dock\plan\PLAN.md "13"), owned by the
launcher (taskbar-dock-start.exe, src\bin\taskbar-dock-start.rs, always running after sign-in).
Grill me on the open choices before building: what the one icon's left click, right click and
tooltip do; how it lists the widgets (each with Show on taskbar, open its panel, wake it); what
happens to each widget's own tray icon; and what happens if the launcher isn't running.

Where we are: taskbar-sysmon is done and pushed (commit 39935eb); status.ps1 reports nothing out
of date. Today each of the five widgets has its own tray icon, shown only while that widget's
strip is hidden (BLOCK.md "Show on taskbar + tray icon"). That icon is also how a hidden widget
is reached: its tray click opens the widget's panel and wakes it from Idle (BLOCK.md "Idle"), so
the one icon must take over both jobs before the per-widget icons can go.

How to build it:
1. Read first: taskbar-dock\CLAUDE.md, BLOCK.md ("Show on taskbar + tray icon", "Idle"),
   README.md ("Building a new widget" step 7 on the launcher, "Testing", "Live checks"),
   plan\PLAN.md, src\block\tray.rs, src\bin\taskbar-dock-start.rs, and how dockctl reaches a
   widget (src\ctl.rs, src\client.rs, src\block\remote.rs): the launcher can drive widgets the
   same way.
2. Before changing the library: commit the last good state (ask me), then
   `scripts\live\run.ps1 -Baseline` so `-Compare` afterwards shows any widget whose look changed.
3. Codex builds once the plan is settled (brief: exact files, the decisions, the done-condition;
   it must write only the files named, never run cargo fmt or rustfmt outside them, write UTF-8,
   and use no typed pixel numbers). Review its diff and look at the result on screen before
   accepting it.
4. Check: `scripts\build_all.ps1` (every widget: the tray code is shared), `scripts\live\run.ps1
   -Compare` (the idle check covers tray wake), `run.ps1 soak -Minutes 10`, `status.ps1`, and a
   real sign-out / sign-in (the launcher starts at sign-in).
5. One Codex review of the whole change, then ask me to commit and push.

Talk to me in plain, non-technical words.
```
