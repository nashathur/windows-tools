# Step 12 build, part 4: session prompt

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).
Written 2026-10-12, after the third build session: jobs 1, 2a and 2b are done and pushed
(`6bc4ca8`); the job 3 brief is checked against the code at `6bc4ca8`; no job 3 run has changed
anything.

---

Continue building plan step 12 of the taskbar widgets: the settings app. The plan is decided and
approved: `taskbar-dock\plan\step12.md` (decisions, contracts 0 and A–J, jobs, "Build notes",
acceptance checks). Don't reopen decisions; where the plan has a gap, choose what fits the
contracts, add it to step12.md "Build notes" and tell me in plain words.

Where we are:
- Jobs 1 (contract 0), 2a (A–F) and 2b (G, dockctl `status` / `start` / `restart`) are done,
  checked live and pushed (`6bc4ca8`). The four widgets are registered in the real settings.json
  (`widgets` list); the `startup` list is unchanged. The new launcher exe is installed but only
  runs at the next sign-in (the last acceptance check).
- Job 3 (the window and the widget pages: H without Dock and Developer, and I) is next. Its brief
  `codex\runs\step12-job3.md` is checked against the code at `6bc4ca8` (names from 2b, status
  colours, theme_lint roots corrected). Read it once more, then run it (`-Role build
  -Worktree`).
- Job 4's brief `codex\runs\step12-job4.md`: before its run, check every library name it uses
  against the code as it is then (a Haiku agent can list them; it confuses "added by the job
  before" with "missing", so verify what it flags) and correct the brief.
- Nothing is left over: no Codex worktree, no `codex exec` running.

Read first: `taskbar-dock\plan\step12.md` (all, especially "Build notes"),
`taskbar-dock\CLAUDE.md`, `taskbar-dock\BLOCK.md`; the code each job touches by section, as the
plan and the brief name it.

Talk to me in plain, non-technical words (I'm not a programmer): what I'll see, what changed,
what you need me to decide or do.

How we work:
1. Jobs in order, one at a time (each builds on the last): 3, then 4. Luna builds each brief in a
   worktree; the worktree branches from HEAD, so commit the previous job first, with my OK. Read
   every diff yourself before `-Accept`; send fix lists back (`-Role build -Sandbox
   workspace-write` in the live checkout after `-Accept`), or a `-Role quick` run for mechanical
   tidying, or fix small things yourself.
2. Hand-offs follow my CLAUDE.md "Subagents" rules: Sonnet runs the five crates' builds and tests
   and reports only failures (give it the exact commands; `taskbar-dock` has no `src\main.rs`),
   Haiku does lookups, you keep decisions and live checks. While Luna builds, use the wait for
   independent work (e.g. checking the next brief, preparing live-check scripts).
3. After each job: build with each widget's `dev.ps1 -NoTest` (tests already ran; its smoke
   check must pass), install the exes (`cargo build --release --bin dockctl --bin
   taskbar-dock-start --bin taskbar-dock-settings` in `taskbar-dock`, copy them from
   `target\release` into `taskbar-dock\`), check live, then ask me to commit and push. After
   job 3, run the app (through a scheduled task) and drive it with `dockctl settings` before
   starting job 4.
4. One Sol review over the whole step (`-Review custom` on this step's files), report each
   finding with your verdict, then the acceptance checks in step12.md (the last one: I sign out
   and in once). Ask me before every commit.

What earlier sessions learned:
- Luna packs code many statements to a line if not told otherwise; the briefs carry a strict
  formatting rule. Check every diff for packed lines (grep added lines for `)->`, `){`, `:&`,
  `;…;`) before accepting. 2b came back clean.
- Luna's sandbox can't run `auth::tests::dpapi_round_trip` (taskbar-discord); the briefs forbid
  changing it. It passes here.
- From this session dockctl sees the Claude app's private copy of the settings
  (`settings::redirected()`): it refuses own-process commands (stopped-widget settings,
  `describe`, `startup`, `widgets`, `start`, `restart`), and it doesn't see the `widgets` registry,
  so it falls back to its built-in widget table (`status` then finds the running exe by file
  name; crash records are hidden). Run own-process commands through a scheduled task:
  `_tmp\real.ps1 '& C:\Users\thurs\tools\_tmp\<script>.ps1'` with the commands in a script file
  (PS 5.1 strips quotes from inline commands); real.ps1 waits at most 20 s. Examples:
  `_tmp\s2b_status.ps1`, `_tmp\s2b_restart.ps1`, `_tmp\seed_registry.ps1`. Commands to a running
  widget work directly.
- A widget's quit takes over 1.5 s to finish (window gone first, process a moment later), so a
  status right after Quit can read "not responding" before "stopped".
- A Codex run stopped before it finishes leaves a worktree ask.ps1 has no record of
  (`-Discard` refuses it): check `git -C <it> status` is empty, then `git worktree remove` it and
  delete the empty folder under `~\.codex\worktrees\`.
- Open from 2b (Codex's suggestions, judged minor; raise in the Sol review): a failed process
  snapshot reads as "stopped"; `start` reports an error when only clearing the crash record
  failed after the widget did start.

Rules worth repeating to every Luna brief (the briefs already carry them):
- Formatting as rustfmt would, plain-English `///` docs; rustfmt only on new or already
  rustfmt-formatted files.
- Win32: WM_COPYDATA data is valid only during the message; UI state lives on the UI thread
  (workers post back; boxed posts are freed if the post fails); restore a DC's old GDI object
  before deleting yours. Colours, fonts and sizes only from `theme` (`tests\theme_lint.rs`).
- Settings writes only through the library's settings functions and only the keys being changed
  (contract 0).
- Builders run only `cargo build` / `cargo test`: never dev.ps1, a widget, the settings app,
  dockctl or the network; tests never hold the real settings mutex (use a test-only name, as
  `tests\settings_patch.rs` does). PowerShell 5.1 strips double quotes in native arguments:
  commit with `git commit -F`.

Live checks: careful, they touch real things.
- Claude desktop sessions see a private `%APPDATA%`: start widgets and the app only through a
  scheduled task (dev.ps1 does this for widgets); read the real settings through dockctl with the
  widget running.
- Before changing a setting for real, note the old value (`dockctl <w> get`) and put it back.
- Bus: never trigger a timetable download or extra OVapi requests. Media and Discord: ask me
  before anything that acts on playback or a call.
