//! The right-click panel: a widget's menu drawn as a panel in the same look as its other
//! panels (and taskbar-media's menu panel), in place of a Windows menu.
//!
//! ```text
//! Title                                   (optional header)
//! Subtitle
//! ───────────────────────────────────
//! ↻  Refresh now                          Item::Row, an action: closes, returns its id
//! ───────────────────────────────────     Item::Sep
//!    Compact mode              [ on ]     a switch: flips, the panel stays open
//!    Start with Windows        [off ]
//!    Update speed   [High|Normal|Low]     a choice: the segment clicked is selected, stays open
//! ───────────────────────────────────
//! ⚙  Edit settings…    ⏻  Quit            Item::Pair: two actions side by side
//! ```
//!
//! [`run`] works like `TrackPopupMenu` with `TPM_RETURNCMD`: it shows the panel, runs its own
//! message loop until the panel closes and returns the action chosen. Wiring, in a widget:
//! - right click on the strip: `panel.right_click(..)`, then [`run`], then `panel.menu_closed()`.
//! - the shared "close your panel" message (dock::close_panel_message) → also [`close`].
//! It closes on a click elsewhere (losing focus), Esc, or an action. A right click on the strip
//! while it's open closes it (it doesn't reopen straight away), a left click there closes it and
//! does what a left click does.

use crate::panel;
use crate::style::Look;
use crate::theme::{size, space, Colors, TextStyle, Theme, CAPTION, ICON, ROW, TITLE_SMALL};
use crate::wide;
use std::cell::RefCell;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Panel width at 100%, px.
pub const WIDTH: f32 = size::PANEL_NARROW;
const ROW_H: f32 = space::ROW_HEIGHT;
const INSET: f32 = space::XS;
const WM_MOUSELEAVE: u32 = 0x02A3;
const VK_ESCAPE: usize = 0x1B;
const TIMER_FADE: usize = 1;
/// A right click that closed the panel (by taking focus away) shouldn't reopen it.
const REOPEN_GUARD: Duration = Duration::from_millis(400);

/// A choice row's segmented picker: the options and the one selected.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub options: Vec<String>,
    pub selected: usize,
}

/// One row: an action, a switch when `switch` is Some(on), or a choice when `choice` is Some.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: usize,
    /// A Segoe Fluent Icons glyph left of the label (actions; switches and choices have none).
    pub glyph: Option<char>,
    pub label: String,
    pub switch: Option<bool>,
    pub choice: Option<Choice>,
}

impl Row {
    pub fn action(id: usize, glyph: char, label: &str) -> Item {
        Item::Row(Row::act(id, glyph, label))
    }

    pub fn switch(id: usize, label: &str, on: bool) -> Item {
        Item::Row(Row {
            id,
            glyph: None,
            label: label.into(),
            switch: Some(on),
            choice: None,
        })
    }

    /// A label with a segmented picker on the right; a click on a segment selects it and the
    /// panel stays open (`Content::menu_choice`).
    pub fn choice(id: usize, label: &str, options: &[&str], selected: usize) -> Item {
        Item::Row(Row {
            id,
            glyph: None,
            label: label.into(),
            switch: None,
            choice: Some(Choice {
                options: options.iter().map(|o| o.to_string()).collect(),
                selected,
            }),
        })
    }

    fn act(id: usize, glyph: char, label: &str) -> Row {
        Row {
            id,
            glyph: Some(glyph),
            label: label.into(),
            switch: None,
            choice: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Row(Row),
    /// Two actions side by side (e.g. Edit settings… and Quit).
    Pair(Row, Row),
    Sep,
}

/// The usual last line: Edit settings… and Quit.
pub fn settings_and_quit(settings_id: usize, quit_id: usize) -> Item {
    Item::Pair(
        Row::act(settings_id, '\u{E713}', "Edit settings…"),
        Row::act(quit_id, '\u{E7E8}', "Quit"),
    )
}

/// Glyphs for common actions (Segoe Fluent Icons).
pub mod glyph {
    pub const REFRESH: char = '\u{E72C}';
    pub const PIN: char = '\u{E707}';
    pub const CALENDAR: char = '\u{E787}';
    pub const OPEN: char = '\u{E8A7}';
    pub const COPY: char = '\u{E8C8}';
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Menu {
    /// Optional title and subtitle above the rows (what the widget shows right now).
    pub title: String,
    pub subtitle: String,
    pub items: Vec<Item>,
}

/// A rectangle in panel px: left, top, width, height.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rc {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rc {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    fn rect(&self) -> RECT {
        RECT {
            left: self.x,
            top: self.y,
            right: self.x + self.w,
            bottom: self.y + self.h,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub w: i32,
    pub h: i32,
    /// Header lines (title, subtitle); zero-sized without a header.
    pub title: Rc,
    pub subtitle: Rc,
    /// Each row's rectangle and the row (cloned from the menu).
    pub rows: Vec<(Rc, Row)>,
    /// Separator lines' y.
    pub seps: Vec<i32>,
    /// A choice row's segments: (row id, option index, rect), in option order.
    pub segments: Vec<(usize, usize, Rc)>,
}

impl Layout {
    /// The row under (x, y). A choice row is never returned: only its segments take clicks.
    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.rows
            .iter()
            .find(|(r, row)| row.choice.is_none() && r.contains(x, y))
            .map(|(_, row)| row.id)
    }

    /// The choice segment under (x, y): (row id, option index).
    pub fn hit_segment(&self, x: i32, y: i32) -> Option<(usize, usize)> {
        self.segments
            .iter()
            .find(|(_, _, r)| r.contains(x, y))
            .map(|&(id, option, _)| (id, option))
    }
}

/// A segment's width at `scale`, px: the text's estimated width plus padding. Layout can't
/// measure text (it is pure), so this is a fixed average advance per character.
fn segment_width(option: &str, scale: f32) -> i32 {
    const ADVANCE_EM: f32 = 0.5;
    let text = option.chars().count() as f32 * CAPTION.px * ADVANCE_EM;
    ((text + 2.0 * space::S) * scale).round() as i32
}

/// The gap between a choice's track edge and its segments, px at 100%.
const SEGMENT_GAP: f32 = space::XS / 2.0;

pub fn layout(menu: &Menu, scale: f32) -> Layout {
    let s = |v: f32| (v * scale).round() as i32;
    let w = s(WIDTH);
    let (inset, rh, pad) = (s(INSET), s(ROW_H), s(space::MENU_PADDING));
    let mut l = Layout {
        w,
        h: 0,
        title: Rc::default(),
        subtitle: Rc::default(),
        rows: Vec::new(),
        seps: Vec::new(),
        segments: Vec::new(),
    };
    let mut y = s(6.0);
    if !menu.title.is_empty() {
        l.title = Rc {
            x: pad,
            y: s(10.0),
            w: w - 2 * pad,
            h: s(20.0),
        };
        l.subtitle = Rc {
            x: pad,
            y: s(30.0),
            w: w - 2 * pad,
            h: s(18.0),
        };
        y = s(54.0);
        l.seps.push(y + s(space::SECTION_GAP / 2.0));
        y += s(space::SECTION_GAP);
    }
    for item in &menu.items {
        match item {
            Item::Row(r) => {
                let rc = Rc {
                    x: inset,
                    y,
                    w: w - 2 * inset,
                    h: rh,
                };
                if let Some(c) = &r.choice {
                    // right-aligned where a switch sits, centred vertically, packed edge to edge
                    let widths: Vec<i32> =
                        c.options.iter().map(|o| segment_width(o, scale)).collect();
                    let gap = s(SEGMENT_GAP);
                    let mut x = rc.x + rc.w - s(10.0) - gap - widths.iter().sum::<i32>();
                    let (sy, sh) = (rc.y + s(space::XS), rc.h - 2 * s(space::XS));
                    for (i, sw) in widths.into_iter().enumerate() {
                        l.segments.push((
                            r.id,
                            i,
                            Rc {
                                x,
                                y: sy,
                                w: sw,
                                h: sh,
                            },
                        ));
                        x += sw;
                    }
                }
                l.rows.push((rc, r.clone()));
                y += rh;
            }
            Item::Pair(a, b) => {
                let half = (w - 2 * inset) / 2;
                l.rows.push((
                    Rc {
                        x: inset,
                        y,
                        w: half,
                        h: rh,
                    },
                    a.clone(),
                ));
                l.rows.push((
                    Rc {
                        x: inset + half,
                        y,
                        w: w - 2 * inset - half,
                        h: rh,
                    },
                    b.clone(),
                ));
                y += rh;
            }
            Item::Sep => {
                l.seps.push(y + s(space::SECTION_GAP / 2.0));
                y += s(space::SECTION_GAP);
            }
        }
    }
    l.h = y + s(6.0);
    l
}

// ---------------------------------------------------------------- drawing

/// Fills a rounded rectangle (fractional px) with `c`, anti-aliased, over the opaque pixels there.
fn fill_round(
    buf: &mut [u32],
    width: i32,
    rect: (f32, f32, f32, f32),
    radius: f32,
    c: (u32, u32, u32),
) {
    crate::draw::fill_round(buf, width, rect, radius, c, 1.0);
}

/// A Windows 11 style on/off switch with its right edge at `right`, centred on `cy`.
fn switch(buf: &mut [u32], width: i32, right: f32, cy: f32, on: bool, pal: &Colors, scale: f32) {
    let (w, h) = (40.0 * scale, 20.0 * scale);
    let (l, t) = (right - w, cy - h / 2.0);
    let knob = if on { 6.0 } else { 5.0 } * scale;
    let kx = if on { right - h / 2.0 } else { l + h / 2.0 };
    if on {
        fill_round(buf, width, (l, t, right, t + h), h / 2.0, pal.fill.tuple());
        fill_round(
            buf,
            width,
            (kx - knob, cy - knob, kx + knob, cy + knob),
            knob,
            pal.bg.tuple(),
        );
    } else {
        let b = 1.0 * scale;
        fill_round(buf, width, (l, t, right, t + h), h / 2.0, pal.knob.tuple());
        fill_round(
            buf,
            width,
            (l + b, t + b, right - b, t + h - b),
            h / 2.0 - b,
            pal.bg.tuple(),
        );
        fill_round(
            buf,
            width,
            (kx - knob, cy - knob, kx + knob, cy + knob),
            knob,
            pal.knob.tuple(),
        );
    }
}

unsafe fn font(t: TextStyle, scale: f32, face: &str) -> HFONT {
    let face = wide(face);
    CreateFontW(
        -t.px_at(scale),
        0,
        0,
        0,
        t.weight,
        0,
        0,
        0,
        1,
        0,
        0,
        5, /* CLEARTYPE_QUALITY */
        0,
        face.as_ptr(),
    )
}

/// Draws `text` in `r` (never empty text: DrawTextW crashes on it).
unsafe fn text(dc: HDC, f: HFONT, s: &str, color: u32, r: RECT, flags: u32) {
    if s.is_empty() || r.right <= r.left {
        return;
    }
    let mut r = r;
    let w: Vec<u16> = s.encode_utf16().collect();
    SelectObject(dc, f);
    SetTextColor(dc, color);
    DrawTextW(
        dc,
        w.as_ptr(),
        w.len() as i32,
        &mut r,
        flags | DT_NOPREFIX | DT_SINGLELINE,
    );
}

/// Paints the panel into `dc` (its client area).
unsafe fn paint(
    dc: HDC,
    l: &Layout,
    menu: &Menu,
    look: &Look,
    scale: f32,
    hover: Option<usize>,
    hover_seg: Option<(usize, usize)>,
) {
    let s = |v: f32| (v * scale).round() as i32;
    let theme = Theme::new(look);
    let pal = &theme.color;
    let mem = CreateCompatibleDC(dc);
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = l.w;
    bmi.bmiHeader.biHeight = -l.h;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    let mut bits = std::ptr::null_mut();
    let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, 0, 0);
    if dib == 0 || bits.is_null() {
        DeleteDC(mem);
        return;
    }
    let old = SelectObject(mem, dib);
    let buf = std::slice::from_raw_parts_mut(bits as *mut u32, (l.w * l.h) as usize);
    buf.fill(pal.bg.pixel());
    for &y in &l.seps {
        fill_round(
            buf,
            l.w,
            (
                s(space::M) as f32,
                y as f32,
                (l.w - s(space::M)) as f32,
                (y + s(1.0).max(1)) as f32,
            ),
            0.0,
            pal.line.tuple(),
        );
    }
    // rows sit `rc.x` px inside the panel: concentric with its corner, never below the control radius
    let row_radius = |inset: i32| {
        crate::draw::concentric(
            crate::draw::PANEL_RADIUS * scale,
            inset as f32,
            (look.control_radius * scale).round(),
        )
    };
    for (rc, row) in &l.rows {
        if hover == Some(row.id) {
            fill_round(
                buf,
                l.w,
                (
                    rc.x as f32,
                    rc.y as f32 + 1.0,
                    (rc.x + rc.w) as f32,
                    (rc.y + rc.h) as f32 - 1.0,
                ),
                row_radius(rc.x),
                pal.hover.tuple(),
            );
        }
        if let Some(on) = row.switch {
            switch(
                buf,
                l.w,
                (rc.x + rc.w - s(10.0)) as f32,
                rc.y as f32 + rc.h as f32 / 2.0,
                on,
                pal,
                scale,
            );
        }
        if let Some(c) = &row.choice {
            let segs: Vec<&(usize, usize, Rc)> =
                l.segments.iter().filter(|(id, _, _)| *id == row.id).collect();
            if let (Some(first), Some(last)) = (segs.first(), segs.last()) {
                let gap = s(SEGMENT_GAP) as f32;
                let track_r = (look.control_radius * scale).round();
                fill_round(
                    buf,
                    l.w,
                    (
                        first.2.x as f32 - gap,
                        first.2.y as f32 - gap,
                        (last.2.x + last.2.w) as f32 + gap,
                        (last.2.y + last.2.h) as f32 + gap,
                    ),
                    track_r,
                    pal.button.tuple(),
                );
                let seg_r = crate::draw::concentric(track_r, gap, 0.0);
                for &&(_, i, r) in &segs {
                    let fill = if i == c.selected {
                        Some(pal.button_hover)
                    } else if hover_seg == Some((row.id, i)) {
                        Some(pal.hover)
                    } else {
                        None
                    };
                    if let Some(fill) = fill {
                        fill_round(
                            buf,
                            l.w,
                            (
                                r.x as f32,
                                r.y as f32,
                                (r.x + r.w) as f32,
                                (r.y + r.h) as f32,
                            ),
                            seg_r,
                            fill.tuple(),
                        );
                    }
                }
            }
        }
    }
    // the text over the shapes
    SetBkMode(mem, TRANSPARENT as _);
    let f = &theme.font;
    let (title_f, sub_f, body_f, icon_f) = (
        font(TITLE_SMALL, scale, &f.text),
        font(CAPTION, scale, &f.text),
        font(ROW, scale, &f.text),
        font(ICON, scale, f.icon),
    );
    text(
        mem,
        title_f,
        &menu.title,
        pal.fg.colorref(),
        l.title.rect(),
        DT_LEFT | DT_TOP | DT_END_ELLIPSIS,
    );
    text(
        mem,
        sub_f,
        &menu.subtitle,
        pal.dim.colorref(),
        l.subtitle.rect(),
        DT_LEFT | DT_TOP | DT_END_ELLIPSIS,
    );
    for (rc, row) in &l.rows {
        if let Some(g) = row.glyph {
            let r = RECT {
                left: rc.x + s(8.0),
                top: rc.y,
                right: rc.x + s(32.0),
                bottom: rc.y + rc.h,
            };
            text(
                mem,
                icon_f,
                &g.to_string(),
                pal.fg.colorref(),
                r,
                DT_CENTER | DT_VCENTER,
            );
        }
        let left = if row.glyph.is_some() {
            rc.x + s(38.0)
        } else {
            rc.x + s(10.0)
        };
        let right = match l.segments.iter().find(|(id, _, _)| *id == row.id) {
            Some((_, _, first)) => first.x - s(SEGMENT_GAP) - s(space::S),
            None => {
                rc.x + rc.w
                    - if row.switch.is_some() {
                        s(56.0)
                    } else {
                        s(8.0)
                    }
            }
        };
        text(
            mem,
            body_f,
            &row.label,
            pal.fg.colorref(),
            RECT {
                left,
                top: rc.y,
                right,
                bottom: rc.y + rc.h,
            },
            DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS,
        );
        if let Some(c) = &row.choice {
            for (_, i, r) in l.segments.iter().filter(|(id, _, _)| *id == row.id) {
                let color = if *i == c.selected { pal.fg } else { pal.dim };
                text(
                    mem,
                    sub_f,
                    c.options.get(*i).map_or("", |o| o.as_str()),
                    color.colorref(),
                    r.rect(),
                    DT_CENTER | DT_VCENTER,
                );
            }
        }
    }
    GdiFlush();
    BitBlt(dc, 0, 0, l.w, l.h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    for f in [title_f, sub_f, body_f, icon_f] {
        DeleteObject(f);
    }
    DeleteObject(dib);
    DeleteDC(mem);
}

// ---------------------------------------------------------------- the window

struct State {
    hwnd: HWND,
    menu: Menu,
    layout: Layout,
    look: Look,
    scale: f32,
    hover: Option<usize>,
    /// The hovered choice segment (row id, option).
    hover_seg: Option<(usize, usize)>,
    pressed: Option<usize>,
    pressed_seg: Option<(usize, usize)>,
    tracking: bool,
    fade: panel::Fade,
    /// An action was chosen.
    chosen: Option<usize>,
    /// A switch was clicked: `run` flips it (outside the window procedure).
    flip: Option<usize>,
    /// A choice segment was clicked (row id, option): `run` hands it to `on_choice`.
    pick: Option<(usize, usize)>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = RefCell::new(None);
    static CLOSED_BY_FOCUS: RefCell<Option<Instant>> = RefCell::new(None);
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Is a menu panel open (on this thread)?
pub fn is_open() -> bool {
    with(|_| ()).is_some()
}

/// Closes an open menu panel (e.g. another widget opened its panel); [`run`] then returns None.
pub fn close() {
    if let Some(h) = with(|s| s.hwnd) {
        unsafe { DestroyWindow(h) };
    }
}

/// The open menu panel's window and rows (for dockctl).
pub fn rows() -> Option<(HWND, Vec<Row>)> {
    with(|s| {
        (
            s.hwnd,
            s.layout.rows.iter().map(|(_, r)| r.clone()).collect(),
        )
    })
}

/// Clicks a row of the open menu panel as the mouse does: a switch flips (the panel stays
/// open), an action is chosen and closes it. False: no panel, or no such row.
pub fn click(id: usize) -> bool {
    let Some(Some((h, is_switch))) = with(|s| {
        s.layout
            .rows
            .iter()
            .find(|(_, r)| r.id == id)
            .map(|(_, r)| (s.hwnd, r.switch.is_some()))
    }) else {
        return false;
    };
    unsafe {
        if is_switch {
            with(|s| s.flip = Some(id));
            PostMessageW(h, WM_NULL, 0, 0); // wakes `run`'s loop, which flips it
        } else {
            with(|s| s.chosen = Some(id));
            DestroyWindow(h);
        }
    }
    true
}

/// Picks an option of a choice row of the open menu panel as a click on its segment does (the
/// panel stays open). False: no panel, no such choice row, or `option` out of range.
pub fn choose(id: usize, option: usize) -> bool {
    let Some(Some(h)) = with(|s| {
        s.layout
            .rows
            .iter()
            .find(|(_, r)| r.id == id)
            .and_then(|(_, r)| r.choice.as_ref())
            .filter(|c| option < c.options.len())
            .map(|_| s.hwnd)
    }) else {
        return false;
    };
    with(|s| s.pick = Some((id, option)));
    unsafe { PostMessageW(h, WM_NULL, 0, 0) }; // wakes `run`'s loop, which does it
    true
}

fn class() -> *const u16 {
    static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    let name = NAME.get_or_init(|| {
        let name = wide("TaskbarDockMenuPanel");
        unsafe {
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(proc_),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(std::ptr::null()),
                hIcon: 0,
                hCursor: LoadCursorW(0, IDC_ARROW),
                hbrBackground: 0,
                lpszMenuName: std::ptr::null(),
                lpszClassName: name.as_ptr(),
            };
            RegisterClassW(&wc);
        }
        name
    });
    name.as_ptr()
}

/// Shows the menu panel above (or next to) `anchor` (screen rect: the strip, or a small rect
/// at the cursor for a tray icon) and returns when it closes: the action row chosen, or None.
/// A switch row calls `on_switch(id)`, which does the change and returns the switch's new state;
/// the panel stays open. A choice segment calls `on_choice(id, option)`, which does the change
/// and returns the option now selected; the panel stays open. Like a menu, it runs its own
/// message loop meanwhile.
/// A right click that closed the panel a moment ago (the click on the strip took its focus)
/// returns None at once, so that right click closes it rather than reopening it.
pub unsafe fn run(
    anchor: RECT,
    menu: Menu,
    look: &Look,
    scale: f32,
    on_switch: &mut dyn FnMut(usize) -> bool,
    on_choice: &mut dyn FnMut(usize, usize) -> usize,
) -> Option<usize> {
    if is_open() {
        close();
        return None;
    }
    if CLOSED_BY_FOCUS
        .with(|c| c.borrow_mut().take())
        .is_some_and(|t| t.elapsed() < REOPEN_GUARD)
    {
        return None;
    }
    let lay = layout(&menu, scale);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST), &mut mi);
    let (x, y) = panel::position(
        anchor,
        lay.w,
        lay.h,
        mi.rcWork,
        (look.panel_margin * scale).round() as i32,
    );
    let title = wide("Menu");
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
        class(),
        title.as_ptr(),
        WS_POPUP,
        x,
        y,
        lay.w,
        lay.h,
        0,
        0,
        GetModuleHandleW(std::ptr::null()),
        std::ptr::null(),
    );
    if hwnd == 0 {
        return None;
    }
    let round = DWMWCP_ROUND;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE as u32,
        &round as *const _ as *const _,
        4,
    );
    let dark: BOOL = (!look.light).into();
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
        &dark as *const _ as *const _,
        4,
    );
    SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA); // fades in
    let mut fade = panel::Fade::default();
    fade.start(Instant::now());
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            hwnd,
            menu,
            layout: lay,
            look: look.clone(),
            scale,
            hover: None,
            hover_seg: None,
            pressed: None,
            pressed_seg: None,
            tracking: false,
            fade,
            chosen: None,
            flip: None,
            pick: None,
        })
    });
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetTimer(hwnd, TIMER_FADE, 10, None);

    let mut msg: MSG = std::mem::zeroed();
    let mut chosen = None;
    while IsWindow(hwnd) != 0 {
        if GetMessageW(&mut msg, 0, 0, 0) <= 0 {
            PostQuitMessage(msg.wParam as i32); // leave WM_QUIT for the widget's own loop
            break;
        }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
        if let Some(id) = with(|s| s.flip.take()).flatten() {
            let on = on_switch(id);
            with(|s| {
                for item in s.menu.items.iter_mut() {
                    if let Item::Row(r) = item {
                        if r.id == id {
                            r.switch = Some(on);
                        }
                    }
                }
                s.layout = layout(&s.menu, s.scale);
            });
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
        if let Some((id, option)) = with(|s| s.pick.take()).flatten() {
            let now = on_choice(id, option);
            with(|s| {
                for item in s.menu.items.iter_mut() {
                    if let Item::Row(r) = item {
                        if r.id == id {
                            if let Some(c) = r.choice.as_mut() {
                                c.selected = now;
                            }
                        }
                    }
                }
                s.layout = layout(&s.menu, s.scale);
            });
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
        if let Some(c) = with(|s| s.chosen).flatten() {
            chosen = Some(c);
        }
    }
    STATE.with(|s| {
        if s.borrow().as_ref().is_some_and(|st| st.hwnd == hwnd) {
            *s.borrow_mut() = None;
        }
    });
    chosen
}

fn client_pt(lp: LPARAM) -> (i32, i32) {
    (
        (lp & 0xFFFF) as i16 as i32,
        ((lp >> 16) & 0xFFFF) as i16 as i32,
    )
}

unsafe extern "system" fn proc_(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(h, &mut ps);
            if let Some((layout, menu, look, scale, hover, hover_seg)) = with(|s| {
                (
                    s.layout.clone(),
                    s.menu.clone(),
                    s.look.clone(),
                    s.scale,
                    s.hover,
                    s.hover_seg,
                )
            }) {
                paint(dc, &layout, &menu, &look, scale, hover, hover_seg);
            }
            EndPaint(h, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_TIMER if w == TIMER_FADE => {
            match with(|s| s.fade.alpha(Instant::now(), &s.look)).flatten() {
                Some(a) => {
                    SetLayeredWindowAttributes(h, 0, a, LWA_ALPHA);
                }
                None => {
                    KillTimer(h, TIMER_FADE);
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = client_pt(l);
            let (changed, track) = with(|s| {
                let hit = s.layout.hit(x, y);
                let seg = s.layout.hit_segment(x, y);
                let changed = s.hover != hit || s.hover_seg != seg;
                s.hover = hit;
                s.hover_seg = seg;
                (changed, !std::mem::replace(&mut s.tracking, true))
            })
            .unwrap_or((false, false));
            if track {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: h,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut tme);
            }
            if changed {
                InvalidateRect(h, std::ptr::null(), 0);
            }
            0
        }
        WM_MOUSELEAVE => {
            with(|s| {
                s.hover = None;
                s.hover_seg = None;
                s.tracking = false;
            });
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = client_pt(l);
            with(|s| {
                s.pressed = s.layout.hit(x, y);
                s.pressed_seg = s.layout.hit_segment(x, y);
            });
            0
        }
        WM_LBUTTONUP => {
            // a row acts when released where it was pressed
            let (x, y) = client_pt(l);
            let act = with(|s| {
                let seg = s.layout.hit_segment(x, y);
                if let Some(seg) = seg.filter(|_| seg == s.pressed_seg.take()) {
                    s.pick = Some(seg);
                    PostMessageW(h, WM_NULL, 0, 0); // wakes `run`'s loop
                    return None;
                }
                let here = s.layout.hit(x, y);
                let pressed = s.pressed.take();
                let id = here.filter(|_| here == pressed)?;
                let is_switch = s
                    .layout
                    .rows
                    .iter()
                    .any(|(_, r)| r.id == id && r.switch.is_some());
                if is_switch {
                    s.flip = Some(id);
                    None
                } else {
                    s.chosen = Some(id);
                    Some(())
                }
            })
            .flatten();
            if act.is_some() {
                DestroyWindow(h);
            }
            0
        }
        WM_KEYDOWN if w == VK_ESCAPE => {
            DestroyWindow(h);
            0
        }
        WM_ACTIVATE => {
            if (w & 0xFFFF) as u32 == WA_INACTIVE && is_open() {
                CLOSED_BY_FOCUS.with(|c| *c.borrow_mut() = Some(Instant::now()));
                DestroyWindow(h);
            }
            0
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Menu {
        Menu {
            title: "Overvecht → Science Park".into(),
            subtitle: "Line 30".into(),
            items: vec![
                Row::action(1, glyph::REFRESH, "Refresh"),
                Item::Sep,
                Row::switch(2, "Compact mode", false),
                Row::switch(3, "Start with Windows", true),
                Item::Sep,
                settings_and_quit(4, 5),
            ],
        }
    }

    #[test]
    fn rows_stack_under_the_header_with_separators() {
        let l = layout(&menu(), 1.0);
        assert_eq!(l.w, 280);
        let ids: Vec<usize> = l.rows.iter().map(|(_, r)| r.id).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5]);
        // header 62, row 32, sep 8, two rows, sep, pair row, bottom 6
        assert_eq!(
            l.rows[0].0,
            Rc {
                x: 4,
                y: 62,
                w: 272,
                h: 32
            }
        );
        assert_eq!(l.rows[1].0.y, 102);
        assert_eq!(l.rows[2].0.y, 134);
        assert_eq!(l.seps, [58, 98, 170]);
        // the pair: two halves on one line
        assert_eq!(
            (l.rows[3].0.y, l.rows[3].0.w, l.rows[4].0.x),
            (174, 136, 140)
        );
        assert_eq!(l.h, 212);
    }

    #[test]
    fn hits_rows_but_not_separators_or_the_header() {
        let l = layout(&menu(), 1.0);
        assert_eq!(l.hit(100, 70), Some(1));
        assert_eq!(l.hit(100, 98), None);
        assert_eq!(l.hit(100, 20), None);
        assert_eq!(l.hit(200, 180), Some(5));
        assert_eq!(l.hit(1, 70), None); // the inset
    }

    fn choice_menu() -> Menu {
        Menu {
            items: vec![
                Row::action(1, glyph::REFRESH, "Refresh"),
                Row::choice(7, "Update speed", &["High", "Normal", "Low", "Paused"], 1),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_choice_rows_segments_sit_right_aligned_inside_the_row_in_order() {
        for scale in [1.0, 1.5] {
            let l = layout(&choice_menu(), scale);
            let row = l.rows[1].0;
            assert_eq!(l.segments.len(), 4);
            let mut prev_right = None;
            for (n, (id, i, r)) in l.segments.iter().enumerate() {
                assert_eq!((*id, *i), (7, n));
                assert!(r.x >= row.x && r.x + r.w <= row.x + row.w);
                assert!(r.y >= row.y && r.y + r.h <= row.y + row.h);
                if let Some(p) = prev_right {
                    assert!(r.x >= p, "segments overlap");
                }
                prev_right = Some(r.x + r.w);
            }
            // the last segment ends where a switch would, give or take the track gap
            let right = row.x + row.w;
            let end = prev_right.unwrap();
            assert!(end < right && right - end < (14.0 * scale) as i32 + 2);
            // the label has room on the left
            assert!(l.segments[0].2.x > row.x + (70.0 * scale) as i32);
        }
    }

    #[test]
    fn hit_segment_finds_each_segment_and_not_the_label() {
        let l = layout(&choice_menu(), 1.0);
        let row = l.rows[1].0;
        for (id, i, r) in &l.segments {
            assert_eq!(
                l.hit_segment(r.x + r.w / 2, r.y + r.h / 2),
                Some((*id, *i))
            );
            // a choice row is never a plain row hit
            assert_eq!(l.hit(r.x + r.w / 2, r.y + r.h / 2), None);
        }
        assert_eq!(l.hit_segment(row.x + 12, row.y + row.h / 2), None);
        assert_eq!(l.hit(row.x + 12, row.y + row.h / 2), None);
        // other rows still hit
        assert_eq!(l.hit(100, l.rows[0].0.y + 5), Some(1));
        assert_eq!(l.hit_segment(100, l.rows[0].0.y + 5), None);
    }

    #[test]
    fn no_header_starts_at_the_top_and_scales() {
        let m = Menu {
            items: vec![Row::action(1, glyph::REFRESH, "Refresh")],
            ..Default::default()
        };
        let l = layout(&m, 1.5);
        assert_eq!((l.w, l.rows[0].0.y, l.h), (420, 9, 9 + 48 + 9));
        assert!(l.seps.is_empty());
    }
}
