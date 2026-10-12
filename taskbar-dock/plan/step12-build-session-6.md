# Step 12, part 6: session prompt

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).
Written 2026-10-12, after the fifth session: all four jobs and the Sol review's fixes are done,
checked live and pushed (`b5926ed`); sub-step 12a (four UI fixes) and two
acceptance checks remain.

---

Finish plan step 12 of the taskbar widgets: the settings app. The plan is decided and approved:
`taskbar-dock\plan\step12.md` (decisions, contracts, "Build notes", acceptance checks). Don't
reopen decisions; where the plan has a gap, choose what fits the contracts, add it to step12.md
"Build notes" and tell me in plain words.

Where we are:
- Jobs 1–4 and the fixes from the one Sol review over the whole step are done and pushed
  (`b5926ed`). The installed exes in `taskbar-dock\` (dockctl, launcher, settings app) and the
  four widgets are built from it. `scripts\settings_smoke.ps1` passes.
- Acceptance checks passed live: Edit settings… from all four widgets; Widgets-page status and
  Quit/Start/Restart of weather; rows match the descriptions and `get`; Compact flip followed
  within a second; a bad number shows the red line and writes nothing; Dock time format (Windows
  is set to 12-hour: "Follow Windows" shows PM, 24h shows 17:35); Developer raw view masks
  `client_secret` and `knmi_api_key`, details show the log tail; light mode redraws correctly
  (I switched Windows to light; I'll switch it back myself).
- Left to check: moving the window to a monitor of different scale (ask me whether I have one),
  and the sign-out/sign-in check (I do it; then check every widget on `startup` runs).

First, sub-step 12a (UI fixes): `step12.md` section "12a" lists the four faults and its
done-condition. Small and visual: fix them yourself or with one Luna brief, and check each
with screenshots in dark and light mode.

Then: ask me to commit; mark steps 12 and 12a done in `plan\PLAN.md` (one line, current
facts), remove the step12 session prompt files; ask me to commit. Step 13 (one tray icon) starts with a
grilling only after the app has been used for a few days.

Read first: `taskbar-dock\plan\step12.md`, `taskbar-dock\CLAUDE.md`, the app's code by section
(`src\settings_app\paint.rs` nav and segment drawing, `win.rs` focus handling, `layout.rs`).
Talk to me in plain, non-technical words.

Live-check scripts (in `_tmp\`): `s3_app_start.ps1 [-AppArgs '--widget weather']` starts the
installed app as at sign-in; `s3_shot.ps1 <out.png>` captures its window; `s4_wheel.ps1 <notches>
[x y]` scrolls it, `s7_click.ps1 <x> <y>` clicks it and `s7_backspace.ps1` sends a Backspace,
all by window messages (the real mouse doesn't move; x/y are screenshot pixels).
Learned: a thread that hasn't checked its messages for 5 s counts as hung and replies sent to it
with SMTO_ABORTIFHUNG are dropped (fixed in `client::reply_window`); writes now answer at once
even after a pause. Before changing a setting for real, note the old value and put it back.
