"""Instructions for the selected tasks, written as Claude would brief local_patch (what to change and
the names to use, not the finished code). Merges into tasks.json:  python instructions.py"""
import json

from common import HERE

I = {
    "c166": "In the function that moves the strip while it's dragged, the unsafe block calls "
            "`strip::move_strip(raw(h), raw(tb), x.min(max_x(tb, width(h))), raise)`. Make that block "
            "multi-line and, after the call, also report the slot it would drop into: "
            "`let scale = scale_of(tb);` then `dock::track_drag(raw(tb), raw(h), dock::tray_boundary(raw(tb), scale), gap_px(scale));`, "
            "with the comment `// while dragged: the slot it would drop into (does nothing otherwise)`.",
    "c021": "In the tests module, after the test that ends just before line 141, add a test "
            "`a_shrunk_tray_doesnt_turn_its_old_buttons_into_apps`. Comment: scanned with the tray from 1745; then its "
            "first icon (the mic) went away: boundary 1777. Data `let b = [r(0, 55), r(319, 363), r(1745, 1777), r(1781, 1805)];`. "
            "Assert `right_edge(&b, 1777)` == `Some(1777)` with message \"judged by today's boundary alone\", and "
            "`right_edge(&b, 1777.min(1745))` == `Some(363)`.",
    "c103": "In the floating-strip branch (`Some(h) if floating`), after the `if fs != fs_hidden { ... }` block add an "
            "`else if !fs && IsWindowVisible(h).as_bool()` branch: the docked widgets moved, so stay left of them. Take "
            "`dock::rect(raw(h))`, its width and height, compute the position with `float_pos(w, ht, system::scale_for(raw(h)))`, "
            "and if it differs from the current left/top move the window there with "
            "`SetWindowPos(h, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE)` (ignore the result with `let _ =`).",
    "c185": "Remove the helper function `client_x(lp: LPARAM) -> i32` (it is unused), together with the blank line before it.",
    "c241": "In the popup's window procedure, add a match arm for `WM_RBUTTONUP` right after the arm that ends at the shown "
            "`0` / `}`: a right click on the departures switches to the right-click panel by calling `context_menu();` and returns 0. "
            "Comment: `// right click on the departures: switch to the right-click panel`.",
    "c161": "Use theme tokens instead of literal values in this function: in the separator's `fill_round` call use "
            "`s(space::M)` instead of both `s(12.0)` and the colour `pal.line` instead of `pal.track`; build `body` as "
            "`Font::new(s(theme::ROW.px), theme::ROW.weight, &look.font)` and `icons` as "
            "`Font::new(s(theme::ICON.px), theme::ICON.weight, ICON_FONT)`.",
    "c023": "Make the hover/press feel subtler: set `HOVER_GROW` to 1.06 and `PRESS_SHRINK` to 0.96.",
    "c213": "Use the shared size tokens: `PANEL_W` becomes `size::PANEL_STANDARD` and `ROW_H` becomes `space::ROW_HEIGHT_LARGE`.",
    "c234": "In the function that turns the settings into a JSON object from `FIELDS`, also include the `compact` flag: "
            "collect the fields into `let mut m: serde_json::Map<String, serde_json::Value>`, insert `\"compact\"` with "
            "`serde_json::json!(self.compact)`, and return `serde_json::Value::Object(m)`.",
    "c143": "Make the layout `l` (from `match width { ... }`) mutable, and right after the match set "
            "`l.height = height;` (comment: for hit testing: a button is hit inside its highlight box) and `l.scale = scale;`.",
    "c233": "When `label` is empty there is only one line: centre it. `(top1, top2)` becomes `(0, (height - th + 1) / 2)` "
            "if `label.is_empty()`, otherwise the existing `draw::two_line_tops(...)` call.",
    "c079": "Both `SetWindowPos(hwnd, HWND_TOP, ...)` calls in this function must also OR "
            "`SET_WINDOW_POS_FLAGS(dock::raise_flag(raw(hwnd)))` into their flags.",
    "c038": "In the tests, before the test that starts at the shown `#[test]`, add a test `sticky_slot_needs_ten_px_past_halfway`. "
            "Comment: media 258 px: slots 1302 (index 1) and 1484 (index 0), halfway 1393; hold 20 = 10 px. Closure "
            "`let s = |d_left, prev| sticky_index(B, GAP, &[], &[179, 75], d_left, 258, Some(prev), 20);`. Asserts: "
            "s(1400, 1) == 1 (7 px past halfway: stays); s(1404, 1) == 0 (11 px past: takes it); s(1386, 0) == 0 and "
            "s(1382, 0) == 1 (and back the same way). Then: no memory yet, or a memory from a different stack size: the "
            "nearest slot - `sticky_index(B, GAP, &[], &[179, 75], 1400, 258, None, 20)` == 0 and the same with `Some(5)` == 0.",
    "c163": "Replace the literal font sizes with theme text styles: `let click = l.mode == Mode::Click;`, a closure "
            "`let font = |t: theme::TextStyle| Font::new(s(t.px), t.weight, &look.font);`, then title = "
            "`font(if click { theme::TITLE } else { theme::TITLE_SMALL })`, body = BODY_LARGE or CAPTION, small = ROW or "
            "CAPTION (click / not click). Remove `title_px`.",
    "c227": "Members' avatars no longer show their mute state: always draw the avatar circle at alpha 255 (not 150 when "
            "deafened), and remove the red dot drawn for muted or deafened members.",
    "c099": "In the hourly forecast: draw the weather emoji with `draw_emoji(dc, &emoji, describe(hr.code, true).0, "
            "s(theme::EMOJI.px), pal.fg, theme.color.bg, x, s(193.0), col, s(28.0), c)` instead of `draw_text`; show the rain "
            "chance when it is at least `RAIN_SHOWN` (instead of 10.0) and colour it with `rain_color(&pal, p)` instead of `pal.rain`.",
    "c226": "Fix the avatar stack: the `stack` closure returns `-s(4.0) as f32` for 0 avatars and otherwise "
            "`ad + step * (n - 1) as f32`; and take each member from `c.people[i]` instead of `members[i]`.",
    "c205": "In `drag_move`: keep `dock::tray_boundary(raw(tb), scale)` in `let b` and use `b` in `max_x`; after the "
            "`SetWindowPos` call, report the slot it would drop into: "
            "`dock::track_drag(raw(tb), raw(hwnd), b, gap_px(scale)); // the slot it would drop into`.",
    "c278": "Dim the feels-like line too (it is the second description line, only when `two_desc`): add "
            "`let feels = if two_desc { (dx, top2, dx + w4, top2 + h4) } else { (0, 0, 0, 0) };` and a closure "
            "`inside(r, x, y)` (x in r.0..r.2 and y in r.1..r.3); pick `dim` when `inside(hum, x, y) || inside(feels, x, y)`. "
            "Change the comment to `// coverage into colour: the humidity and feels-like line dimmer`.",
    "c255": "Compact mode now shows the next two departures on two lines (`text` holds them separated by '\\n'). Replace the "
            "`(line, label)` computation: `let (label, text) = match text.split_once('\\n') { Some((a, b)) if compact => (a, b), "
            "_ => (if compact { \"\" } else { cfg.label.as_str() }, text) };` and `let line = cfg.line.as_str();`. Comment: "
            "`// compact: the next two departures on two lines beside the badge, no label`.",
    "c254": "In compact mode take 2 departures instead of 1, and join them with \"\\n\" in compact mode instead of two spaces.",
    "c151": "A resize running while the strip is picked up should grow with the zoom: compute "
            "`let draw_w = resize_w.map(|w| (w as f32 * zoom).round() as i32);` (comment: a resize running while picked up "
            "grows with the zoom like everything else) and pass `draw_w` instead of `resize_w` as that argument of "
            "`render::update_strip` (keep `resize_w.is_none()` for the compact argument). In the `with(|a| { ... })` after it, "
            "also set `a.rest_layout = Some(l);` but only when `zoom == 1.0`.",
    "c042": "Use the spacing tokens: `pad` is `s(space::MENU_PADDING)` instead of `s(12.0)`; a separator is pushed at "
            "`y + s(space::SECTION_GAP / 2.0)` instead of `s(4.0)`, and `y` advances by `s(space::SECTION_GAP)` instead of `s(8.0)`.",
    "c171": "Remove the block that drops a paused track with a known timeline from `a.live` (`let r = &s.timeline;` and the "
            "`if !s.playing && ... { a.live.remove(&key); }`). At the end of the function, also prune `has_length` to the "
            "current keys: `a.has_length.retain(|k| keys.contains(k));`.",
    "c080": "In `drag_ended`, save the new order before ending the drag: inside the unsafe block, first "
            "`with(|a| a.panel.drag_ended());`, then (comment: save the new order first, then end the drag: the others keep "
            "the preview till then) `if let Ok(tb) = GetParent(hwnd) { if with(|a| on_right(&a.cfg)) == Some(true) { ...the "
            "existing drop_reorder call with its scale... } }` - no early return - and only then the `dock::set_dragging` call.",
    "c109": "Remove the function `tb_left(tb: HWND) -> i32` and the blank line after it (it moved elsewhere).",
    "c261": "Use the theme for the picker font: the face is `wide(taskbar_dock::theme::font_override::BUS_PICKER)` instead "
            "of \"Segoe UI\", the size `-px(taskbar_dock::theme::BODY.px)` instead of `-px(14.0)`, and the weight "
            "`taskbar_dock::theme::BODY.weight` instead of 400.",
    "c169": "Two changes: (1) when there is no artist (`artist.trim().is_empty()`), draw the title alone centred between "
            "the two lines: `let t1 = if artist.trim().is_empty() { (top1 + top2) / 2 } else { top1 };` and use `t1` instead "
            "of `top1` in that `draw_text` RECT (comment: no artist: the title alone, centred between the two lines). "
            "(2) The disabled button colour comes from the theme: "
            "`let off = (taskbar_dock::theme::Theme::new(look).strip.disabled * 255.0).round() as u8;` and use "
            "`rgb(off, off, off)` instead of `rgb(0x60, 0x60, 0x60)`.",
    "c230": "In the panel header, show the server's logo at the top left (as on the strip) with the channel over the "
            "server beside it: `let ld = s(LOGO_D);`, draw it with "
            "`panel_logo(dc, pad, s(12.0) + (s(40.0) - ld) / 2, ld, logo, &icons, pal.bg, &theme);`, then "
            "`let tx = pad + ld + s(space::M);` and `let tw = w - pad - tx - s(70.0);`, and draw the two text lines "
            "at x `tx` with width `tw` instead of `pad` and `inner - s(70.0)`.",
    "c157": "When the taskbar is recreated, keep a strip that is already re-attached to the new taskbar (forgetting a live "
            "one left an untracked duplicate there, seen in weather, 2026-10-10): `let strip = with(|a| a.strip).flatten();`, "
            "`let current = strip.is_some_and(|h| IsWindow(h).as_bool() && GetParent(h).ok() == taskbar());`, and only when "
            "not floating AND `!current` call `destroy_strip();` (instead of just clearing `a.strip`).",
}

sel = json.loads((HERE / "selected.json").read_text(encoding="utf-8"))
missing = [t["id"] for t in sel["tasks"] if t["id"] not in I]
for t in sel["tasks"]:
    t["instruction"] = I.get(t["id"], "")
sel["tasks"] = [t for t in sel["tasks"] if t["instruction"]]
(HERE / "tasks.json").write_text(json.dumps(sel, indent=1, ensure_ascii=False), encoding="utf-8")
print(f"{len(sel['tasks'])} tasks -> tasks.json; without instruction: {missing}")
