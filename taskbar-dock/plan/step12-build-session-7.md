# Step 12a: session prompt

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).
Written 2026-10-12, after the planning session that turned 12a into "Windows Settings look".

---

Build plan sub-step 12a of the taskbar widgets: the settings app's Windows Settings look. The
plan is decided and approved: `taskbar-dock\plan\step12.md` section "12a" (decisions, the test
and its seven checks, order of work, done-condition). Don't reopen decisions; where the plan has
a gap, choose what fits, add it to step12.md "Build notes" and tell me in plain words.
References: `plan\step12a-rating.md` (Sol's element table: WinUI sizes, radii, colours, fonts,
control mapping) and `plan\step12a-native.md` (real Windows controls: WinUI 3, runtime, Rust
routes, icon-font codepoints, accent palette API, Mica).

Where we are:
- Step 12 (jobs 1–4 and the Sol review fixes) is done and pushed; the installed exes in
  `taskbar-dock\` are built from it and `scripts\settings_smoke.ps1` passes. All acceptance checks
  passed live except the monitor-of-another-scale one (no such screen: mark it so) and the
  sign-out/sign-in check (I do it; then check every widget on `startup` runs).
- Windows App Runtime packages on this PC: 1.2, 1.5, 1.7, 1.8, 2 (2.5.1) and the ones serviced
  with Windows (`Microsoft.WindowsAppRuntime.CBS.1.6`, `.CBS.2`).

First, the test (step12.md "The test"): one Codex Luna build brief for the throwaway WinUI 3
window (the app untouched), then run the seven checks yourself and give me the results in plain
words with your recommendation; I choose (a) real Windows controls or (b) our own controls
redrawn with Direct2D. Then the order of work in step12.md, with Luna briefs where the pieces are
independent, screenshots in dark and light (you switch Windows' mode and put it back to dark),
and one Sol review of the whole change before telling me it's done.

Then: ask me to commit; mark steps 12 and 12a done in `plan\PLAN.md` (one line, current facts),
remove the step12 session prompt files; ask me to commit. Step 13 (one tray icon) starts with a
grilling only after the app has been used for a few days.

Read first: `taskbar-dock\plan\step12.md` ("12a", then contracts H and I for what the app and
dockctl must keep doing), `taskbar-dock\CLAUDE.md`, `UI-RULES.md` (the widgets' rules; the app now
follows Windows' fixed-radius look instead), the app's code by section (`src\settings_app\`).
Talk to me in plain, non-technical words.

Live-check scripts (in `_tmp\`): `s3_app_start.ps1 [-AppArgs '--widget weather']` starts the
installed app as at sign-in; `s3_shot.ps1 <out.png>` captures its window; `s4_pages.ps1`
screenshots every page; `s4_wheel.ps1 <notches> [x y]` scrolls it, `s7_click.ps1 <x> <y>` clicks
it and `s7_backspace.ps1` sends a Backspace, all by window messages (the real mouse doesn't move;
x/y are screenshot pixels). Dark-mode screenshots of the current look: `_tmp\ui\dark_*.png`.
Learned: a thread that hasn't checked its messages for 5 s counts as hung and replies sent to it
with SMTO_ABORTIFHUNG are dropped (fixed in `client::reply_window`). Before changing a setting for
real, note the old value and put it back.
