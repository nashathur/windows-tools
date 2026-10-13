# Step 12c: session prompt

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).
Written 2026-10-13, at the end of the 12b session.

---

Plan sub-step 12c of the taskbar widgets: the open checks and small decisions left by the settings
app (12, 12a, 12b). The list is `taskbar-dock\plan\step12b.md` "Still open"; what 12b built and why
is in the same file ("Decided", "Gaps filled while building").

Where we are: 12b is committed and pushed (`c56eda6`). Every widget, `dockctl.exe`,
`taskbar-dock-start.exe` and `taskbar-dock-settings.exe` are installed from that commit and
`scripts\settings_smoke.ps1` passes. New since 12a: Block's Idle state (BLOCK.md "Idle"), the
Highlight box settings (UI-RULES.md "Highlight box colour"), strips always gliding (`strip.rs`
rules), the reactor's Border entrance transition (`settings-app\vendor\PATCHES.md`), and the app's
page width and centring (`PAGE_MAX_WIDTH`, `page_shift` in `settings-app\src\win.rs`).

How to run it:
1. Do the checks you can do yourself first (dockctl, scheduled tasks, screenshots): Edit
   settings… from each widget's right-click menu opens the app on that widget's page; the Dock
   time format reaches bus and weather; an idle widget wakes from a tray click.
2. Then the ones that need me, one at a time, each with plain step-by-step instructions and what
   I should see: signing out and in (every widget on the startup list runs; one with Show on
   taskbar off starts idle), the 125% / 150% display scale, and clicking through the app myself
   (Highlight box rows, slide-ins, centring when maximised). Note every setting you change and
   put it back.
3. Then the open ideas (media site colours from the tab's address, hiding the Colour row unless
   "Chosen colour", the Gemini rating, deleting `tools\winui-test\`): ask me about each in plain
   words, with your recommendation.
4. Fix what the checks find (a failing test first where a test can reach it); Codex reviews the
   whole change once at the end; then ask me to commit.
5. When 12c is done, update `step12b.md` "Still open" and `PLAN.md` to current facts, and tell me
   whether the app now looks robust enough to start step 13 (one tray icon); if I agree, grill
   me on it.

Read first: `taskbar-dock\plan\step12b.md`, `taskbar-dock\CLAUDE.md`, `BLOCK.md` ("Idle"),
`UI-RULES.md`, `README.md` "Settings app", then the code by section.
Talk to me in plain, non-technical words.

Live-check scripts (in `_tmp\`): `s3_app_start.ps1 [-Exe <path>]` starts the app as at sign-in;
`s3_shot.ps1 <out.png>` captures its window; `s12_pages.ps1 <prefix> <pages…>` screenshots pages
to `ui12\`; `g12_anim.ps1 -Page <page>` captures the frames of a page change;
`g12_glide.ps1 -Widget <w> -Value true|false` flips Show on taskbar and prints every strip's
position over time; `shot_strip.ps1 -Class <window class>` captures one strip;
`theme_mode.ps1 light|dark` switches Windows' mode (put it back to dark). Claude sessions see a
private `%APPDATA%`: start the app and widgets only through scheduled tasks (a widget's `dev.ps1`
does; `dockctl <w> restart` from a session may not). Discord without a real call:
`$env:TASKBAR_DISCORD_TEST='8'; .\dev.ps1 -Fast` in `taskbar-discord` (8 people: the avatar places
fill), then `.\dev.ps1 -NoTest` to go back to the real connection.
