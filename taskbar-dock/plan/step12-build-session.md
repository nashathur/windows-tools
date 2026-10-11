# Step 12 build: session prompt

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).
Written 2026-10-12, after the planning session that wrote `step12.md` (decided with the user,
critiqued by Codex Sol, lightened: no launcher watching in step 12).

---

Build plan step 12 of the taskbar widgets: the settings app. The plan is decided and approved:
`taskbar-dock\plan\step12.md` (decisions, contracts 0 and A–J, four jobs, acceptance checks).
Don't reopen decisions; where the plan has a gap, choose what fits the contracts, note it in
step12.md and tell me in plain words.

Read first: `taskbar-dock\plan\step12.md` (all), `taskbar-dock\CLAUDE.md`, `taskbar-dock\BLOCK.md`;
the code each job touches by section, as the plan names it.

Talk to me in plain, non-technical words (I'm not a programmer): what I'll see, what changed, what
you need me to decide or do.

How we work:
1. Before job 1, ask me to commit the plan files (the library rule: the last good state is
   committed before the library changes).
2. Jobs 1–4 in order, one at a time (each builds on the last), as in step 11: one
   self-contained Luna brief per job in `codex\runs\step12-job<n>.md` (`-Role build -Worktree`),
   pasting the job's contracts from step12.md (the worktree branches from HEAD). Read every diff
   yourself, `-Accept`, send fix lists back or fix small things. Build with each widget's
   `dev.ps1` (its smoke check must pass), install the exes (widgets, dockctl, the launcher, the
   app: copy the release exes into `taskbar-dock\` as the README says), check live. After job 3,
   run the app and drive it with `dockctl settings` before starting job 4.
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
  dockctl or the network. PowerShell 5.1 strips double quotes in native arguments: commit with
  `git commit -F`.

Live checks: careful, they touch real things.
- Claude desktop sessions see a private `%APPDATA%`: start widgets and the app only through a
  scheduled task (dev.ps1 does this for widgets); read the real settings through dockctl with the
  widget running.
- Before changing a setting for real, note the old value (`dockctl <w> get`) and put it back.
- Bus: never trigger a timetable download or extra OVapi requests. Media and Discord: ask me
  before anything that acts on playback or a call.
