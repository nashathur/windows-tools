# Parity checklist (step 0, 2026-10-10)

Checked before and after each port (steps 3, 5, 6, 7). Sources: `inventory-label.md`,
`inventory-placement.md` (local-llm), spot-checked against each widget's `main.rs`, `label.rs` /
`placement.rs`, `flyout.rs` / `popup.rs` and `menu.rs`. Items marked **(new)** were missing from
the inventories.

How to verify (scripts in `tools\_tmp`, all read-only unless noted; clicks only on our own
windows):
- **strips**: `strips.ps1` (one line per strip: class, rect, visible); `strips.ps1 -Toggle <section> -Ms 600` flips `compact` and samples the width animation.
- **watch**: `watch_strips.ps1 -Seconds N` (prints rect / visibility / icons-edge changes).
- **z**: `watch_z.ps1 -Seconds 30` (restacks; 0 at rest).
- **drag**: `drag_probe.ps1 <Class> [Dx]` (width/x every ~10 ms through a drag); `sticky_probe.ps1 -Class <A> -Other <B>` (slot while dragging, order unchanged on a same-slot drop).
- **fit**: `fit_test.ps1 -Edge <x> [-Widget taskbar-weather]` (restarts the leader, the first strip in order.txt, with `TASKBAR_DOCK_TEST_ICONS_RIGHT`, samples strips; dev.ps1 passes the variable into its scheduled task).
- **real**: `real.ps1 '<command>'` runs a command outside the Claude app's container (real `%APPDATA%`): read the widget's log, `real.ps1 -Compact <section> [-Key <bool key>]` flips a setting in the real settings.json. Widgets started by dev.ps1 use the real files, so session-side edits (`strips.ps1 -Toggle`) don't reach them.
- **rclick**: `rclick.ps1 [-Class <strip>] [-NoClick]` right-clicks a strip and says which widget's menu is open (check before clicking rows with `clickpanel.ps1`: they hit whatever menu is open). `FindWindow` from a session missed open menus; `wins.ps1 <pid>` sees them.
- **hover**: `hover_probe.ps1 <Class> <name>` (crops at rest / hovered).
- **menu**: `menuprobe.ps1 -strip <Class>` (right-click panel screenshot).
- **log**: the widget's log, new lines are printed by `dev.ps1`. Weather/discord: `settings::dir()\taskbar-<name>.log`; media: `taskbar-media.log` next to the exe; bus: `buswidget.log` in its data dir. (From a Claude session `settings::dir()` is the private MSIX copy, see CLAUDE.md.)
- **hand**: the user checks by hand (Explorer restart, theme switch, fullscreen, anything needing real use).

Class names: weather `TaskbarWeatherLabel`, discord `TaskbarDiscordStrip`, media `TaskbarMediaStrip`, bus `BusWidgetBar`.

## Common to all four (Block)

Process and start
- [ ] One instance only: starting a second copy exits at once (named mutex `Local\Taskbar*.SingleInstance` / bus's). Verify: hand (double-click the exe while it runs; `Get-Process` shows one). Never start a widget from a session shell (CLAUDE.md). **(new)**
- [ ] A panic is written to the log before exit (`crash::log_panics`). Verify: log line after a forced panic in a test build, or code check that Block calls it. **(new)**
- [ ] Joins the dock stack against the tray (`dock::mark_member`), in the saved order (`order.txt`). Verify: strips (x order matches order.txt).

Drag and drop
- [ ] Drag starts only after 3 px; a smaller move is a click. Verify: drag (Dx 2 → click; Dx 40 → drag).
- [ ] Picked-up strip zooms to `drag_zoom` 1.15 and stays centred under the mouse; zoom back on drop. Verify: drag (width rises to ~1.15× and returns).
- [ ] Drag limits: never over the taskbar buttons, never past the tray. Verify: drag with large Dx both ways; x clamps.
- [ ] While one strip is dragged the others jump (no slide) to their new slots. Verify: watch during a drag (neighbour x changes in one step).
- [ ] Sticky slot: wiggling and returning to the start drops into the same slot, order unchanged. Verify: sticky_probe.
- [ ] Drop: reorder, order file written, glide into the slot (`glide_ms` 150), zoom back. Verify: drag + order.txt mtime/content.
- [ ] Raised above neighbours while dragged; 0 restacks at rest. Verify: z.
- [ ] Capture lost mid-drag (Esc, another window takes the mouse) ends it like a drop. Verify: hand (press Esc while dragging). **(new)**
- [ ] Starting a drag closes this widget's hover-opened panel (a pinned one stays, `PanelBehaviour::drag_started`; checked 2026-10-10 in step 3) and the right-click panel. Verify: hand / log.

Layout and fit
- [ ] Neighbour moves or resizes → instant reflow via WinEvent hooks (no wait for the 1 s watch timer). Verify: watch while toggling compact on another widget.
- [ ] Watch timer as backup: missed events still correct within ~1 s; also re-applies `raise_flag` when the taskbar's content is above. Verify: watch, z. **(new detail)**
- [ ] Compact on/off (right-click switch or settings.json edit) animates the width and keeps the right edge. Verify: `strips.ps1 -Toggle <section> -Ms 600` (right edge constant, width monotone).
- [ ] Crowded: narrow → compact → hide, and back in reverse as room returns. Verify: fit (`-Edge` values stepping in, then `-Edge 0`).
- [ ] Hidden for lack of room logs it (Block: "no room next to the taskbar's buttons: strip hidden"; weather said "label hidden" before step 3). Verify: log.
- [ ] Strip vertically centred on the taskbar. Verify: strips (top/bottom symmetric to taskbar). **(new for weather/discord)**

Panels
- [ ] Hover opens the panel after the hover time (`hover_open_ms`, Windows setting), without focus; leaving closes it after 300 ms. Verify: hand (or hover + `wins.ps1 <pid>` shows the panel window visible).
- [ ] Left click opens the panel pinned (with focus); click again or clicking elsewhere (focus lost) closes it. Verify: hand / wins.
- [ ] Esc closes the open panel. **(new)** Verify: hand.
- [ ] Right click on the strip opens the right-click panel; right click while the left-click panel is open switches to it; left click while the right-click panel is open switches back. Right click on the left-click panel also opens it. **(new: right click on the panel)** Verify: menu, hand.
- [ ] After the right-click panel closes, no hover-open until the mouse leaves and comes back (the menu "leaves" too: only a real leave ends the block). **(new)** Verify: hand.
- [ ] Only one panel across all widgets: opening one closes the others (`dock::close_panel_message`, `close_other_panels`). Verify: hand (open weather, then bus).
- [ ] Panels fade in (`fade_ms` 150), are placed centred on the strip, `panel_margin` 12 px above the taskbar, inside the work area, DWM round corners. Verify: wins (rect) + hand.
- [ ] Right-click panel standard rows: widget actions, Compact mode, Start with Windows (switches flip and the panel stays open), Edit settings… (opens notepad on settings.json), Quit. Verify: menu.
- [ ] Strip hover highlight while the mouse is over it (whole strip, inset). Verify: hover.

Follows Windows and settings live
- [ ] Theme / scaling / text size / time format / animations changes redraw strip and panels without a restart (`WM_SETTINGCHANGE`, `WM_DISPLAYCHANGE`, `WM_DPICHANGED`, `refresh_look`). Verify: hand (switch light/dark).
- [ ] Animations off in Windows: glides/zooms/fades jump. Verify: hand.
- [ ] Edits to settings.json (Edit settings…) apply when saved (mtime polled by the watch timer). Verify: edit `compact` by hand in settings.json, strips. **(new)**
- [ ] Start with Windows switch edits the `startup` list (not the Run key). Verify: settings.json after toggling.

Explorer restart
- [ ] Explorer restart: old strip gone, new one created and docked in order; widgets keep running. Verify: hand (restart Explorer), then strips.
- [ ] Quit ends the program; `WM_DESTROY` from Explorer taking the taskbar down does not. **(new, bus comment)** Verify: hand.

## Weather

- [ ] Side option: `label_position` left or right of the taskbar buttons; right = in the tray stack. Verify: settings edit + strips.
- [x] **Show on taskbar** switch (Block's `show_on_taskbar` since 3b, was `taskbar_label`): off removes the strip and adds the tray icon, the menu stays open; on brings the strip back and removes the icon. Verified 2026-10-10 (menu switch, file edit, log).
- [ ] Tray icon (`tray.rs`): shows the temperature, left click toggles the flyout, right click opens the right-click panel (placed above the pointer when the strip is off); re-added after Explorer restart (`TaskbarCreated`). **(new)** Verify: hand.
- [ ] Right-click panel header: location name + one-line summary (line1 · line2 · desc). Rows: Refresh now, Show on taskbar, Fahrenheit (°F), Compact mode, Start with Windows, Edit settings…, Quit. **(new)** Verify: menu.
- [ ] Data timer: fetch every `refresh_minutes`; new data redraws strip, tray icon and open flyout; settings changes to city/units/model/KNMI refetch at once. **(new)** Verify: log (fetch lines).
- [ ] Offline/error: strip shows "Offline", "…" while loading. Verify: hand (disconnect) or log.
- [ ] Compact: temperature over humidity (description hidden). Verify: strips -Toggle weather.
- [ ] Strip narrower than its content: right end fades out (12 px fade). Verify: fit.
- [ ] Flyout 360 × 494, fixed. Verify: wins.

## Discord

- [ ] Visible by state: in a call, or status needs the user (sign in, error), or `show_when_idle` and ready. Hidden → the strip is destroyed (closes its panels first), not just hidden. Verify: hand (join/leave a call) + strips.
- [ ] Right-click panel: in a call, Mute microphone / Deafen switches and Leave call; then Compact mode, Show when not in a call, Start with Windows; Sign in to Discord / Reconnect when not connected; Edit settings…, Quit. Title/subtitle = guild/channel. **(new)** Verify: menu.
- [ ] Strip buttons Mute / Deafen / Leave: hit box = `button_box` (24 × 24 at 100%), toggle the state; highlight only the box. Verify: hover + hand.
- [ ] Logo opens Discord on release over the logo (`window::raise`); a drag from the logo still drags. Verify: hand + drag (from logo).
- [ ] Cover feel: logo grows 1.06 on hover, dips 0.96 on press, only while calm. Verify: hover (crop sizes).
- [ ] Ring: pulses (25 fps, `TIMER_BEAT`) while unmuted in a call; green when speaking, dim when self-muted, red when deafened. Verify: hand / hover crops.
- [ ] Attention dot (orange) when status needs the user. **(new)** Verify: hand.
- [ ] Avatars of call members (loaded on a worker thread, `WM_AVATAR` redraws); overlap stack; "+N" pill when crowded. Verify: hand.
- [ ] Rest of the strip toggles the panel. Panel: header, member rows (40 px), Mute/Deafen/Leave buttons with hover highlight; updates live on voice events (`refresh_popup`); button click acts via RPC. **(new: panel buttons)** Verify: hand.
- [ ] Settings edits with new app credentials reconnect RPC. **(new)** Verify: log.

## Bus

- [ ] Floating fallback: if the taskbar can't be joined, a topmost window at the x saved in `pos.txt`; a drag while floating moves freely and writes `pos.txt`. Log: "strip attached…" / "floating…". Verify: log + pos.txt.
- [ ] Floating: re-attaches when the taskbar comes back (on_tick). **(new)** Verify: hand (Explorer restart).
- [ ] Floating: hides while a full-screen app is in front (attached: the taskbar does that). **(new)** Verify: hand.
- [ ] Data: live poll every 60 s only when a bus is due within 90 min; live data older than 10 min ignored; strip redraws when times/line/colour change. **(new)** Verify: log.
- [ ] Line badge colour: settings `badge_color`/`badge_text` → `data\line_colors.txt` (reloaded on change) → GTFS colour → turquoise. **(new)** Verify: hand (edit line_colors.txt).
- [ ] Compact: next two departures. Verify: strips -Toggle bus.
- [ ] Popup: title, departure rows (time, destination, minutes, status: cancelled red, live green, scheduled grey), footer; size from content. Verify: hand.
- [ ] Right-click panel incl. "Change stop…" opening the picker window; picker: Tab / Enter / Esc work. **(new)** Verify: menu + hand.
- [ ] Bus is the dock's leader (reads taskbar icons via UIA; `fit_test.ps1` restarts it). **(new, context)** Verify: fit.

## Media

- [ ] Visible while something plays: fade in when playing starts, fade out (then hide, others close up) when it stops; playing again mid-fade turns back from where it is. Verify: hand + watch.
- [ ] Floating fallback: attaching fails, no taskbar for 10 s (`FLOAT_AFTER`), or `--float` → topmost window left of the tray and the docked widgets (bottom-right of the primary screen without a taskbar); re-attaches when possible; logs "strip floating above the taskbar" / "strip attached to the taskbar again". **(new: media floats too, not only bus)** Verify: log.
- [ ] Floating: hides while a full-screen app is in front; stays left of docked widgets when they move. **(new)** Verify: hand.
- [ ] Width capped by `max_width` (136); compact mode one line. Verify: strips.
- [ ] Cover / brand logo opens the playing app on release (`window::raise`); cover feel 1.06 / 0.96; drag from cover still drags. Verify: drag_probe (default class), hover.
- [ ] Strip buttons (prev/play/next): `button_box` hit boxes; unsupported ones dimmed. Verify: hover + hand.
- [ ] Progress ring around the cover; LIVE in `live_color` (#FF4E45) for live streams (learned per app: `progress.txt`). Verify: hand.
- [ ] Own right-click panel (`panel_view.rs` Menu mode, 300 wide): header with cover, rows, Compact / Start with Windows switches, two half-width buttons at the bottom. Becomes `menu_panel` in step 8 (user-visible change, planned). Verify: menu.
- [ ] Click panel (360 wide): cover, title/artist/album/source, seek bar (click to seek when seekable), times, prev/play/next, volume slider + mute + percent; copy text. Panel colours tinted by the cover (`wash`, accent bar). **(new: tint)** Verify: hand.
- [ ] Brand file (`brands`) edits reload logos live. **(new)** Verify: hand.
- [ ] Does not move while the mouse is held on it (check_fit / reposition skip while pressed). Verify: drag_probe (no jump at press).
