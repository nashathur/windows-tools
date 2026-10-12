# Step 12 build, part 3: session prompt

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).
Written 2026-10-12, after the second build session: jobs 1 and 2a are done and pushed
(`788d0b4`); the job 2b brief is written and checked against the code; no 2b run has changed
anything.

---

Continue building plan step 12 of the taskbar widgets: the settings app. The plan is decided and
approved: `taskbar-dock\plan\step12.md` (decisions, contracts 0 and A–J, jobs, "Build notes",
acceptance checks). Don't reopen decisions; where the plan has a gap, choose what fits the
contracts, add it to step12.md "Build notes" and tell me in plain words.

Where we are:
- Job 1 (contract 0) and job 2a (contracts A–F) are done, checked live and pushed (`788d0b4`).
  The four widgets are registered in the real settings.json (`widgets` list) with plain exe
  paths; the `startup` list is unchanged.
- Job 2b (contract G, dockctl `status` / `start` / `restart`) is next. Its brief is written and
  checked against the code at `788d0b4`: `codex\runs\step12-job2b.md` (steps a–e, what to leave
  alone, the done-condition). Read it once more, then run it (`-Role build -Worktree`).
- Jobs 3 and 4 have briefs too: `codex\runs\step12-job3.md`, `codex\runs\step12-job4.md`. Before
  each run, check every library name they use against the code as it is then (a Haiku agent can
  list them) and correct the brief.
- Nothing is left over: no Codex worktree, no `codex exec` running.

Read first: `taskbar-dock\plan\step12.md` (all, especially "Build notes"),
`taskbar-dock\CLAUDE.md`, `taskbar-dock\BLOCK.md`; the code each job touches by section, as the
plan and the brief name it.

Talk to me in plain, non-technical words (I'm not a programmer): what I'll see, what changed,
what you need me to decide or do.

How we work:
1. Jobs in order, one at a time (each builds on the last): 2b, 3, 4. Luna builds each brief in a
   worktree; the worktree branches from HEAD, so commit the previous job first, with my OK. Read
   every diff yourself before `-Accept`; send fix lists back (`-Role build -Sandbox
   workspace-write` in the live checkout after `-Accept`), or a `-Role quick` run for mechanical
   tidying, or fix small things yourself.
2. Hand-offs follow my CLAUDE.md "Subagents" rules: Sonnet runs the five crates' builds and tests
   and reports only failures (give it the exact commands; `taskbar-dock` has no `src\main.rs`),
   Haiku does lookups, you keep decisions and live checks.
3. After each job: build with each widget's `dev.ps1` (its smoke check must pass), install the
   exes (dockctl and the launcher: `cargo build --release --bin dockctl --bin
   taskbar-dock-start` in `taskbar-dock`, copy both from `target\release` into `taskbar-dock\`;
   after job 3 also the app), check live, then ask me to commit and push. After job 3, run the
   app and drive it with `dockctl settings` before starting job 4.
4. One Sol review over the whole step (`-Review custom` on this step's files), report each
   finding with your verdict, then the acceptance checks in step12.md (the last one: I sign out
   and in once). Ask me before every commit.

What the last session learned:
- Luna packed code many statements to a line with no doc comments; 2a was sent back for it. The
  briefs now carry a strict formatting rule. Check every diff for packed lines (e.g. grep added
  lines for `)->`, `){`, `:&`, `;…;`) before accepting.
- Luna changed the DPAPI test to skip when its sandbox couldn't run it; the briefs now forbid
  that. Rerun `auth::tests::dpapi_round_trip` here: it passes.
- dockctl refuses commands it would run in its own process (stopped-widget settings, `describe`,
  `startup`, `widgets`, and from 2b `start` / `restart`) when `settings::redirected()` says it
  sees the Claude app's private copy, which it always does from this session. Run those through
  a scheduled task: `_tmp\real.ps1 '& C:\Users\thurs\tools\_tmp\<script>.ps1'` with the commands
  in a script file (PS 5.1 strips quotes from inline commands). Examples:
  `_tmp\seed_registry.ps1`, `_tmp\peek_startup.ps1`. Commands to a running widget work directly.
- A Codex run stopped before it finishes leaves a worktree ask.ps1 has no record of
  (`-Discard` refuses it): check `git -C <it> status` is empty, then `git worktree remove` it and
  delete the empty folder under `~\.codex\worktrees\`.
- Live checks for 2b: `dockctl all status` (all four running); then quit, start and restart
  **weather** only, through the scheduled task; its status follows within 2 s. The give-up record
  is proven by unit tests (don't crash a widget on purpose). The launcher isn't running between
  sign-ins; installing its new exe changes nothing until the next sign-in (the last acceptance
  check).

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
