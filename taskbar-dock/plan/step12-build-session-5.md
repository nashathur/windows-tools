# Step 12 build, part 5: session prompt

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).
Written 2026-10-12, after the fourth build session: jobs 1, 2a, 2b and 3 are done, checked live
and pushed (`e4e111d`); the job 4 brief is checked against the code at `e4e111d`; no job 4 run
has changed anything.

---

Continue building plan step 12 of the taskbar widgets: the settings app. The plan is decided and
approved: `taskbar-dock\plan\step12.md` (decisions, contracts 0 and A–J, jobs, "Build notes",
acceptance checks). Don't reopen decisions; where the plan has a gap, choose what fits the
contracts, add it to step12.md "Build notes" and tell me in plain words.

Where we are:
- Jobs 1 (contract 0), 2a (A–F), 2b (G) and 3 (the app's window, Widgets page, widget pages,
  `dockctl settings`: H without Dock and Developer, and I) are done, checked live and pushed
  (`e4e111d`). The installed exes in `taskbar-dock\` (dockctl, launcher, settings app) and the
  four widgets are built from `e4e111d`. The new launcher only runs at the next sign-in (the last
  acceptance check).
- Job 4 (Dock page, Developer tab, Edit settings…, the smoke script, docs: H rest, J, I's smoke)
  is next and is the last job. Its brief `codex\runs\step12-job4.md` is checked against the code
  at `e4e111d` (every library name exists; it now also asks for: finishing the Auto control left
  over from job 3, card positions computed in one `layout` function, controls always drawn as
  controls, `ctl::mask_with` for the raw view, and a summary that starts "complete" or lists
  what isn't). Read it once more, then run it (`-Role build -Worktree`).
- Nothing is left over: no Codex worktree, no `codex exec` running, the settings app is closed.

Read first: `taskbar-dock\plan\step12.md` (all, especially "Build notes"),
`taskbar-dock\CLAUDE.md`, `taskbar-dock\BLOCK.md`; the code job 4 touches by section, as the plan
and the brief name it (`taskbar-dock\src\settings_app\`, `block\win.rs` `Route::Settings`).

Talk to me in plain, non-technical words (I'm not a programmer): what I'll see, what changed,
what you need me to decide or do.

How we work:
1. Luna builds the job 4 brief in a worktree. Read the whole diff yourself before `-Accept`
   (packed lines, the Win32 rules, settings writes only through `control`); send fix lists back
   (`-Role build -Sandbox workspace-write` in the live checkout after `-Accept`), or fix small
   things yourself.
2. Hand-offs follow my CLAUDE.md "Subagents" rules: Sonnet runs the five crates' builds and tests
   and reports only failures (give it the exact commands; `taskbar-dock` has no `src\main.rs`),
   Haiku does lookups, you keep decisions and live checks. While Luna builds, prepare the live
   checks.
3. After job 4: build with each widget's `dev.ps1 -NoTest` (its smoke check must pass), install
   the exes (`cargo build --release --bin dockctl --bin taskbar-dock-start --bin
   taskbar-dock-settings` in `taskbar-dock`, copy them from `target\release` into
   `taskbar-dock\`), run `scripts\settings_smoke.ps1`, check live **with screenshots** of every
   page (Dock, Developer, a widget page, Edit settings… from a widget's menu), then ask me to
   commit and push.
4. One Sol review over the whole step (`-Review custom` on this step's files: every file changed
   since `1dba28b` (the plan commit), i.e. jobs 1–4), report each finding with your verdict, fix the real ones,
   then the acceptance checks in step12.md (the last one: I sign out and in once). Ask me before
   every commit.

What earlier sessions learned:
- **ask.ps1 hangs after Codex finishes**: an MSVC link in Codex's cargo build leaves
  `vctip.exe` running, and ask.ps1 (`Start-Process -Wait`) waits for it. Signs: the
  `.answer.md` exists but the background run hasn't ended. Fix: `Stop-Process -Name vctip`;
  ask.ps1 then finishes and records the worktree. Easiest: start a watcher next to each run
  (bash: wait until `<prompt>.answer.md` is non-empty, wait 20 s, `taskkill //F //IM vctip.exe`).
- **Luna may stop half-way and still exit 0**: job 3's first run (57 min) built about half and
  said so only under "residual risks". Read the summary's first line and the risks before
  anything else; continue in the same worktree with `-Dir <worktree> -Role build -Sandbox
  workspace-write` and an exact gap list (job 3 took three Luna passes plus my own fixes).
- **Unit tests don't catch the app's real bugs; the live check does.** Job 3's tests were green
  while the app crashed on every widget page (an empty string's dangling pointer to DrawTextW),
  controls were drawn as plain words, and drawing and click positions drifted after scrolling.
  Always look at screenshots of each page.
- Live-check scripts (in `_tmp\`): `s3_app_start.ps1 [-AppArgs '--widget weather']` starts the
  installed app through a scheduled task (real `%APPDATA%`) and waits until `dockctl settings
  state` answers; `s3_shot.ps1 <out.png>` captures the app window even when covered. Crashes of
  the app: Windows Application event log (Id 1000, `taskbar-dock-settings.exe`); no debugger or
  dump is available.
- `dockctl settings key esc` with nothing being edited closes the app (by design, H).
- `control::quit` now waits for the widget's process to end (a Start right after Quit used to
  find the closing process and start nothing).
- Luna packs code if not told otherwise; the briefs carry a strict formatting rule. Grep added
  lines for `)->`, `){`, `:&`, `;…;` and lines over 100 chars before accepting.
- Luna's sandbox can't always run `auth::tests::dpapi_round_trip` (taskbar-discord); the briefs
  forbid changing it. It passes here.
- From this session dockctl sees the Claude app's private copy of the settings
  (`settings::redirected()`): it refuses own-process commands (stopped-widget settings,
  `describe`, `startup`, `widgets`, `start`, `restart`) and falls back to its built-in widget
  table. Run those through a scheduled task: `_tmp\real.ps1 '& C:\Users\thurs\tools\_tmp\<script>.ps1'`
  with the commands in a script file (PS 5.1 strips quotes from inline commands). Commands to a
  running widget, and `dockctl settings …` to the running app, work directly.
- A Codex run stopped before it finishes leaves a worktree ask.ps1 has no record of: check
  `git -C <it> status` is empty, then `git worktree remove` it and delete the empty folder under
  `~\.codex\worktrees\`.
- Open minor points for the Sol review: from 2b, a failed process snapshot reads as "stopped",
  and `start` reports an error when only clearing the crash record failed after the widget did
  start; from 3, typing a number into a slider shows the typed text beside the slider (the caret
  sits apart from it), `dockctl settings row <id> set` can't set empty text, a dropdown opened
  on a stored value that is no option (choice without `other`) has no highlighted item.

Rules worth repeating to every Luna brief (the briefs already carry them):
- Formatting as rustfmt would, plain-English `///` docs; rustfmt only on new or already
  rustfmt-formatted files (the app's `src\settings_app\*` are rustfmt-formatted).
- Win32: WM_COPYDATA data is valid only during the message; UI state lives on the UI thread
  (workers post back; boxed posts are freed if the post fails); restore a DC's old GDI object
  before deleting yours; never pass an empty Vec's pointer to a Win32 text call. Colours, fonts
  and sizes only from `theme` (`tests\theme_lint.rs`).
- Settings writes only through the library's settings functions and only the keys being changed
  (contract 0).
- Builders run only `cargo build` / `cargo test`: never dev.ps1, a widget, the settings app,
  dockctl or the network; tests never hold the real settings mutex. PowerShell 5.1 strips double
  quotes in native arguments: commit with `git commit -F`.

Live checks: careful, they touch real things.
- Claude desktop sessions see a private `%APPDATA%`: start widgets and the app only through a
  scheduled task (dev.ps1 and `_tmp\s3_app_start.ps1` do this); read the real settings through
  dockctl with the widget running.
- Before changing a setting for real, note the old value (`dockctl <w> get`, `dockctl <w> get
  --dock` for dock keys) and put it back.
- Bus: never trigger a timetable download or extra OVapi requests. Media and Discord: ask me
  before anything that acts on playback or a call.
