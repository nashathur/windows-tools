# Step 12 build, part 2: session prompt

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).
Written 2026-10-12, after the first build session: job 1 is done and pushed (`dbf384b`); the
job 2a Codex run was started and stopped before it changed anything.

---

Continue building plan step 12 of the taskbar widgets: the settings app. The plan is decided and
approved: `taskbar-dock\plan\step12.md` (decisions, contracts 0 and A–J, jobs, "Build notes",
acceptance checks). Don't reopen decisions; where the plan has a gap, choose what fits the
contracts, add it to step12.md "Build notes" and tell me in plain words.

Where we are:
- Job 1 (contract 0, safe settings) is done, checked live and pushed (`dbf384b`).
- Job 2 is split into two Codex builds (step12.md "Build notes"): **2a** (contracts A–F) and
  **2b** (G, dockctl `status` / `start` / `restart`). Their briefs are written:
  `codex\runs\step12-job2a.md` and `codex\runs\step12-job2b.md`. Read both before running them
  and correct anything that no longer matches the code.
- The first 2a run was stopped before it changed anything. It left an empty worktree,
  `C:\Users\thurs\.codex\worktrees\aeda`: check that `git -C <it>\tools status` is empty and
  drop it (`ask.ps1 -Discard`, or remove the worktree if ask.ps1 doesn't know it). Check that
  no old `codex exec` from that run is still going before starting a new one.

Read first: `taskbar-dock\plan\step12.md` (all), `taskbar-dock\CLAUDE.md`,
`taskbar-dock\BLOCK.md`; the code each job touches by section, as the plan names it.

Talk to me in plain, non-technical words (I'm not a programmer): what I'll see, what changed,
what you need me to decide or do.

How we work:
1. Jobs in order, one at a time (each builds on the last): 2a, 2b, 3, 4. One self-contained
   Luna brief per job in `codex\runs\step12-job<n>.md` (`-Role build -Worktree`); jobs 3 and 4
   still need their briefs: write them from step12.md the same way (paste the job's contracts;
   the worktree branches from HEAD, so commit the previous job first, with my OK). Read every
   diff yourself, `-Accept`, send fix lists back or fix small things. Codex's sandbox can't run
   discord's DPAPI test (`auth::tests::dpapi_round_trip`): rerun it here, it passes.
2. After each job: build with each widget's `dev.ps1` (its smoke check must pass), install the
   exes (widgets, dockctl, the launcher, the app: copy the release exes into `taskbar-dock\` as
   the README says), check live, then ask me to commit. After job 2a, seed the registry for the
   four release exes (`dockctl widgets register <exe>` run through a scheduled task, since
   dockctl refuses inside the Claude app). After job 3, run the app and drive it with
   `dockctl settings` before starting job 4.
3. One Sol review over the whole step (`-Review custom` on this step's files), report each
   finding with your verdict, then the acceptance checks in step12.md (the last one: I sign out
   and in once). Ask me before every commit.

Rules worth repeating to every Luna brief:
- Normally formatted Rust like the surrounding code (one statement per line, plain-English doc
  comments); don't rustfmt hand-formatted files.
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
