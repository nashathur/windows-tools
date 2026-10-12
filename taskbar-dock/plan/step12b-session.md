# Step 12b: session prompt

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).
Written 2026-10-13, at the end of the 12a session.

---

Build plan sub-step 12b of the taskbar widgets: seven changes I asked for after trying the new
settings app. The list, what was found in the code, my answers and the order of work are in
`taskbar-dock\plan\step12b.md`. My answers are under "Decided"; don't reopen them. Where the plan
has a gap, choose what fits, add it to step12b.md and tell me in plain words.

Where we are: 12a (the settings app rebuilt with real Windows controls, crate
`taskbar-dock\settings-app\`) is committed and pushed. The installed
`taskbar-dock\taskbar-dock-settings.exe` is its latest build and `scripts\settings_smoke.ps1`
passes. 12a's build notes are in `plan\step12.md` "12a build notes"; the reactor patches are
listed in `settings-app\vendor\PATCHES.md`.

Order of work (step12b.md): show me the weather colours (one per weather description) before
building them; Codex Luna briefs in parallel worktrees for the independent pieces; the
Block-wide pieces (Idle, highlight box settings, neighbours glide) one at a time, after you've
looked at the code yourself; live checks through dockctl and scheduled tasks; app screenshots in
dark and light (you switch Windows' mode and put it back to dark); one Codex Sol review of the
whole change, then ask me to commit.

Read first: `taskbar-dock\plan\step12b.md`, `taskbar-dock\CLAUDE.md` ("Where a rule lives",
"Every widget has these parts"), `BLOCK.md`, `UI-RULES.md`, then the code by section.
Talk to me in plain, non-technical words. Before changing a setting for real, note the old value
and put it back.

Live-check scripts (in `_tmp\`): `s3_app_start.ps1 [-Exe <path>]` starts the app as at sign-in;
`s3_shot.ps1 <out.png>` captures its window; `s12_pages.ps1 <prefix> <pages…>` screenshots pages
to `ui12\`; `theme_mode.ps1 light|dark` switches Windows' mode. Claude sessions see a private
`%APPDATA%`: start the app and widgets only through scheduled tasks. Discord without a real call:
`$env:TASKBAR_DISCORD_TEST='4'; .\dev.ps1 -Fast` in `taskbar-discord`.
