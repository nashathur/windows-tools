# Step 12: session prompt

Paste the block below into a new Claude Code session (working folder `C:\Users\thurs\tools`).
Written 2026-10-12, after step 11 (78fd156), the dev.ps1 smoke check (e410f91) and the clean-up
commits (72780bf, 25eee76). `main` is pushed; only `main` exists.

---

Start plan step 12 of the taskbar widgets: the settings app (`taskbar-dock\plan\PLAN.md`, "12").
It is still an idea. This session decides it and writes the plan; building comes after I approve
the plan (this session if time allows, else the next).

Read first: `taskbar-dock\plan\PLAN.md` (step 12), `taskbar-dock\CLAUDE.md`, `taskbar-dock\BLOCK.md`
(Spec, Content, tray icon, the standard menu rows, settings Block owns), `taskbar-dock\README.md`
("The shared `dock` settings", "Testing" / dockctl incl. `keys`), and by section as the questions
reach them: `src\settings.rs` (`update` with its cross-process lock, `write_atomic`),
`src\startup.rs` and `src\bin\taskbar-dock-start.rs` (startup list, launcher + watchdog),
`src\block\win.rs` (the Edit settings… row opens notepad on settings.json, ~line 854),
`src\block\tray.rs`, `src\block\remote.rs` (`keys`, `unset`), `src\ctl.rs` (`is_secret`,
`mask_setting`, `unset_default`), `src\style.rs` (the dock section's JSON form), `src\theme.rs`,
`src\text_edit.rs` (the picker's text box), and each widget's `Config` (its section and
`Spec::defaults`).

Facts to build on (checked 2026-10-12; verify before relying on one):
- Every widget has its own section in `%APPDATA%\TaskbarDock\settings.json`; Block owns `compact`
  and `show_on_taskbar` in each, plus the shared `dock` section; `startup` is the list the
  launcher `taskbar-dock-start.exe` starts at sign-in (it then stays as the crash watchdog).
- Widgets follow file edits live (Block's settings watch, `Content::settings_changed`).
- `Spec::defaults` + dockctl `keys` give each key's current and default value, but no label, type,
  range or help line; secrets are recognised by name (`ctl::is_secret`).
- Each widget has its own tray icon, owned by Block (`Content::tray_icon`), shown while the strip
  is switched off or idle: with Show on taskbar off it is the widget's only way back.
- Edit settings… (Block's standard row) opens notepad on the whole settings.json.

How we work:
1. Decide with me first: load the `grill-me` skill and grill me on step 12, one question at a time,
   each with your recommendation. Cover at least:
   - where each widget's setting descriptions live (a `Spec` field? next to its `Config`?) and
     how the app gets them for a widget that isn't running (compiled into the app from the
     library, a file each widget writes, asking running widgets through dockctl?);
   - types and validation (switch, number with range, text, choice, secret; who checks ranges:
     the app, the widget, both; does dockctl `set` use the same descriptions?);
   - the `dock` section, Block's own keys, Start with Windows, and the per-widget actions that
     aren't settings (bus Change stop…, Discord Sign in): in the app or not;
   - the tray icons: one icon for the app replacing the widgets' (then what brings back a widget
     whose Show on taskbar is off, who keeps the app's icon alive: the launcher, the app itself),
     or keep both for now;
   - the window: a normal top-level window or a Block-style panel; drawn with `theme` / `draw` /
     `text_edit` (no UI framework); dark/light, DPI, keyboard navigation;
   - when changes apply (on change vs a Save button), errors shown where, secrets masked;
   - Edit settings… → `taskbar-dock-settings.exe --widget <name>`, notepad as the fallback;
   - testing: how dockctl (or a dockctl-like path) drives the app, what a smoke check looks like;
   - how the build splits into Codex jobs.
2. Write the decisions to `taskbar-dock\plan\step12.md` (decisions, contracts precise enough for a
   builder that guesses on gaps, the job split, acceptance checks), then replace PLAN.md's
   step 12 idea with a pointer to it.
3. Load the `codex` skill and get a plan critique from Codex Sol (`-Role review`): the plan's
   weakest points and what it would do differently. Weigh each point against the code, tell me
   your verdict on each, update step12.md with what we accept, and wait for my go.
4. Then build as in step 11: one self-contained Luna brief per job in `codex\runs\step12-*.md`
   (`-Role build -Worktree`, parallel when the jobs touch different files, library jobs one at a
   time); read every diff yourself, `-Accept`, send fix lists back or fix small things; build
   with each widget's `dev.ps1` (its smoke check must pass) and check live; one Sol review over
   the whole step before you report it done; ask me before every commit.

Rules worth repeating to every Luna brief:
- Normally formatted Rust like the surrounding code (one statement per line, plain-English doc
  comments); don't rustfmt hand-formatted files.
- Win32: WM_COPYDATA data is valid only during the message; UI state lives on the UI thread
  (workers post back; boxed posts are freed if the post fails); restore a DC's old GDI object
  before deleting yours. Colours, fonts and sizes only from `theme` (`tests\theme_lint.rs`).
- Settings writes only through `settings::update` and only the keys being changed (a whole-section
  write puts back stale values: a bug found in 11b).
- Builders run only `cargo build` / `cargo test`: never dev.ps1, a widget, the settings app,
  dockctl or the network. PowerShell 5.1 strips double quotes in native arguments: commit with
  `git commit -F`.

Live checks: careful, they touch real things.
- Claude desktop sessions see a private `%APPDATA%`: start widgets and the app only through a
  scheduled task (dev.ps1 does this for widgets); read the real settings through dockctl.
- Before changing a setting for real, note the old value (`dockctl <w> get`) and put it back.
- Bus: never trigger a timetable download or extra OVapi requests. Media and Discord: ask me
  before anything that acts on playback or a call.
