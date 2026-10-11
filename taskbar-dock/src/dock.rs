//! Docking widgets into the Windows 11 taskbar as one stack against the tray.
//!
//! Each widget is a layered child window of the taskbar (`Shell_TrayWnd`). Members mark their
//! window with the property [`PROP_MEMBER`] (and [`PROP_DRAGGING`] while being dragged); other
//! processes can read window properties, so the widgets coordinate without talking to each
//! other. The stack sits against the tray boundary; its order (right to left, by window class)
//! is in `%APPDATA%\TaskbarDock\order.txt` and changes only when a dragged widget is dropped.
//! While one is dragged, the others leave its slot open where it would land (live preview).
//!
//! The stack never covers the taskbar's own buttons (Start, apps; read through UI Automation
//! by one member, see [`crate::icons`]). Without room, [`fit`] narrows members (media's text
//! column first, then compact mode) and then hides them, farthest from the tray first; they
//! come back when there's room. Each member publishes its widths ([`publish_widths`]) and
//! applies its own [`Slot::fit`].
//!
//! All coordinates are screen coordinates. Functions are `unsafe` because they call Win32.

use crate::{settings, wide};
use std::collections::HashMap;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::UI::Accessibility::{NotifyWinEvent, SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK, WINEVENTPROC};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const PROP_MEMBER: &str = "TaskbarDock.Member";
pub const PROP_DRAGGING: &str = "TaskbarDock.Dragging";
/// A member hidden because there's no room next to the taskbar's buttons (see [`fit`]).
pub const PROP_SQUEEZED: &str = "TaskbarDock.Squeezed";
/// A member's widths for the fit: in its own mode, compact, and whether it can take any width
/// in between ([`publish_widths`]).
pub const PROP_FULL: &str = "TaskbarDock.FullWidth";
pub const PROP_MIN: &str = "TaskbarDock.MinWidth";
pub const PROP_STRETCH: &str = "TaskbarDock.Stretch";
/// While a member's width animates (compact switch): the width it's drawn at right now
/// ([`publish_drawn`]). The others place themselves by it, frame by frame ([`Slot::follow`]).
pub const PROP_DRAWN: &str = "TaskbarDock.DrawnWidth";
/// The right edge of the taskbar's own buttons, published by the member that reads them.
pub const PROP_ICONS: &str = "TaskbarDock.IconsRight";
/// While dragged: the slot it would drop into (widgets staying to its right), decided by the
/// dragged member alone ([`track_drag`]) so the live preview and the drop always agree.
pub const PROP_SLOT: &str = "TaskbarDock.Slot";
/// Px (at 100%) past the halfway point before a dragged widget changes slot: no flicker when
/// it's held right at the switch point.
pub const STICKY_PX: f32 = 10.0;

/// The taskbar window, if Explorer is running.
pub unsafe fn taskbar() -> HWND {
    FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null())
}

pub unsafe fn rect(h: HWND) -> RECT {
    let mut rc: RECT = std::mem::zeroed();
    GetWindowRect(h, &mut rc);
    rc
}

pub unsafe fn cursor() -> POINT {
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    pt
}

/// Joins the stack (call once the window is a taskbar child), or marks a drag start/end.
pub unsafe fn mark_member(h: HWND) {
    SetPropW(h, wide(PROP_MEMBER).as_ptr(), 1);
}

/// Leaves the stack (e.g. a widget moved to the other end of the taskbar).
pub unsafe fn leave(h: HWND) {
    for p in [PROP_MEMBER, PROP_SQUEEZED, PROP_FULL, PROP_MIN, PROP_STRETCH, PROP_DRAWN, PROP_ICONS, PROP_SLOT] {
        RemovePropW(h, wide(p).as_ptr());
    }
}

/// Marks a drag start or end. On a drop, call [`drop_reorder`] first and this after it: the
/// others keep the preview until the new order is saved, so nothing flicks back for a moment.
pub unsafe fn set_dragging(h: HWND, dragging: bool) {
    if dragging {
        SetPropW(h, wide(PROP_DRAGGING).as_ptr(), 1);
    } else {
        RemovePropW(h, wide(PROP_DRAGGING).as_ptr());
        RemovePropW(h, wide(PROP_SLOT).as_ptr());
    }
}

unsafe fn is_dragging(h: HWND) -> bool {
    GetPropW(h, wide(PROP_DRAGGING).as_ptr()) != 0
}

/// Must member `h` be raised? Only when something that isn't a dock member (the taskbar's own
/// content) is above it, or, while `h` is dragged, anything at all (it passes over its
/// neighbours). Members never overlap at rest, so their order among themselves doesn't matter.
pub unsafe fn needs_raise(h: HWND) -> bool {
    let (member, dragging) = (wide(PROP_MEMBER), wide(PROP_DRAGGING));
    let dragged = GetPropW(h, dragging.as_ptr()) != 0;
    let mut above = GetWindow(h, GW_HWNDPREV);
    while above != 0 {
        if IsWindowVisible(above) != 0 && (dragged || GetPropW(above, member.as_ptr()) == 0) {
            return true;
        }
        above = GetWindow(above, GW_HWNDPREV);
    }
    false
}

/// The z-order flag for a member's `SetWindowPos(h, HWND_TOP, ..)`: 0 (raise) when
/// [`needs_raise`], else `SWP_NOZORDER`. Raising on every redraw made the members raise each
/// other in turn (each raise is a WinEvent the others react to): ~7 restacks a second, and every
/// strip on the taskbar flickered now and then. OR it into every member's `SetWindowPos` flags.
pub unsafe fn raise_flag(h: HWND) -> u32 {
    if needs_raise(h) { 0 } else { SWP_NOZORDER }
}

/// A number in a window property, readable by the other widgets' processes. Bit 32 marks it
/// as set, so 0 and negative screen x survive.
unsafe fn set_num(h: HWND, name: &str, v: Option<i32>) {
    let n = wide(name);
    match v {
        Some(v) => {
            SetPropW(h, n.as_ptr(), (v as u32 as isize) | (1isize << 32));
        }
        None => {
            RemovePropW(h, n.as_ptr());
        }
    }
}

unsafe fn get_num(h: HWND, name: &str) -> Option<i32> {
    let v = GetPropW(h, wide(name).as_ptr());
    (v & (1isize << 32) != 0).then_some(v as u32 as i32)
}

/// Tells the other members how wide this one is (`full`, in its own mode), how narrow it can
/// get (`min`: its compact width) and whether it can take any width in between (`stretch`).
/// Unzoomed px. Call after drawing; [`fit`] decides from these, never from the window's
/// current (maybe narrowed) size.
pub unsafe fn publish_widths(h: HWND, s: Size) {
    let stretch = Some(s.stretch as i32);
    if get_num(h, PROP_FULL) != Some(s.full) || get_num(h, PROP_MIN) != Some(s.min) || get_num(h, PROP_STRETCH) != stretch {
        set_num(h, PROP_FULL, Some(s.full));
        set_num(h, PROP_MIN, Some(s.min));
        set_num(h, PROP_STRETCH, Some(s.stretch as i32));
    }
}

/// While this member's width animates (compact switch): the width it's drawn at on this frame;
/// None on the last frame. Call before the window takes the new width, so the others, placing
/// themselves on the move it causes, already see it. The fit still goes by [`publish_widths`].
/// True if it changed (cleared: [`notify`], as the window may not change on that frame).
pub unsafe fn publish_drawn(h: HWND, w: Option<i32>) -> bool {
    let w = w.filter(|w| *w > 0);
    if get_num(h, PROP_DRAWN) == w {
        return false;
    }
    set_num(h, PROP_DRAWN, w);
    true
}

/// Hidden for lack of room ([`Fit::Hidden`]): it stays in the stack's order while hidden, and
/// the others keep its widths in the fit, so it comes back when there's room again.
pub unsafe fn set_squeezed(h: HWND, on: bool) {
    set_num(h, PROP_SQUEEZED, on.then_some(1));
}

/// A member's published widths; its window width when it hasn't published any.
unsafe fn size_of(h: HWND, rc: RECT) -> Size {
    let full = get_num(h, PROP_FULL).unwrap_or(rc.right - rc.left);
    Size { full, min: get_num(h, PROP_MIN).unwrap_or(full).min(full), stretch: get_num(h, PROP_STRETCH) == Some(1) }
}

/// The message a widget's strip receives when another widget opens its panel: close yours.
/// (Clicking a widget doesn't activate anything, so a panel opened by a click on another
/// widget wouldn't notice the click by itself.) Handle it in the strip's window procedure.
pub fn close_panel_message() -> u32 {
    static MSG: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(wide("TaskbarDock.ClosePanel").as_ptr()) })
}

/// Asks every other dock member to close its panel; call when opening yours.
pub unsafe fn close_other_panels(tb: HWND, me: HWND) {
    let msg = close_panel_message();
    for c in children(tb) {
        if c.member && c.hwnd != me {
            PostMessageW(c.hwnd, msg, 0, 0);
        }
    }
}

/// Tells the other widgets (through their WinEvent hooks) to look again, e.g. after a drop.
pub unsafe fn notify(h: HWND) {
    const OBJID_WINDOW: i32 = 0;
    NotifyWinEvent(EVENT_OBJECT_LOCATIONCHANGE, h, OBJID_WINDOW, 0);
}

/// A direct child window of the taskbar: visible, or a member squeezed out (hidden for lack of
/// room; its `rc` is where it last was, so don't treat it as occupied space).
pub struct Child {
    pub hwnd: HWND,
    pub class: String,
    pub rc: RECT,
    pub member: bool,
    pub dragging: bool,
    pub squeezed: bool,
    /// A member's widths for the fit ([`publish_widths`]).
    pub size: Size,
    /// A member whose width is animating: its width right now ([`publish_drawn`]).
    pub drawn: Option<i32>,
}

/// The taskbar's visible child windows, plus squeezed-out members. While the action center is
/// open this can briefly come back empty, so callers treat "I'm not in the list" as "the list
/// can't be trusted".
pub unsafe fn children(tb: HWND) -> Vec<Child> {
    unsafe extern "system" fn collect(h: HWND, lp: LPARAM) -> BOOL {
        (*(lp as *mut Vec<HWND>)).push(h);
        1
    }
    let mut found: Vec<HWND> = Vec::new();
    EnumChildWindows(tb, Some(collect), &mut found as *mut _ as LPARAM);
    let (member, dragging, squeezed) = (wide(PROP_MEMBER), wide(PROP_DRAGGING), wide(PROP_SQUEEZED));
    found
        .into_iter()
        .filter(|h| GetParent(*h) == tb)
        .filter_map(|h| {
            let is_member = GetPropW(h, member.as_ptr()) != 0;
            let is_squeezed = is_member && GetPropW(h, squeezed.as_ptr()) != 0;
            if IsWindowVisible(h) == 0 && !is_squeezed {
                return None;
            }
            let mut name = [0u16; 128];
            let n = GetClassNameW(h, name.as_mut_ptr(), name.len() as i32);
            let rc = rect(h);
            (rc.right > rc.left).then(|| Child {
                hwnd: h,
                class: String::from_utf16_lossy(&name[..n.max(0) as usize]),
                rc,
                member: is_member,
                dragging: GetPropW(h, dragging.as_ptr()) != 0,
                squeezed: is_squeezed,
                size: if is_member { size_of(h, rc) } else { Size::fixed(rc.right - rc.left) },
                drawn: if is_member { get_num(h, PROP_DRAWN).filter(|w| *w > 0) } else { None },
            })
        })
        .collect()
}

// ---- order ------------------------------------------------------------------

pub fn order_path() -> std::path::PathBuf {
    settings::dir().join("order.txt")
}

/// Saved stack order, right (tray side) to left, as window class names.
pub fn read_order() -> Vec<String> {
    std::fs::read_to_string(order_path())
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Atomic (temp file + rename): the others read it on every reflow and must never see half.
pub fn write_order(order: &[String]) {
    let _ = std::fs::create_dir_all(settings::dir());
    let _ = settings::write_atomic(&order_path(), &(order.join("\n") + "\n"));
}

/// Position in the saved order (unknown classes after the known ones).
pub fn rank(order: &[String], class: &str) -> usize {
    order.iter().position(|o| o == class).unwrap_or(order.len())
}

// ---- the tray boundary ----------------------------------------------------------

/// Last good tray edge per taskbar: the shell sometimes hides or resizes the tray window
/// briefly (action center); then the last good value is used, never the screen edge.
static TRAY_LEFT: Mutex<Option<(isize, i32)>> = Mutex::new(None);

unsafe fn explorer_setting(name: &str, default: u32) -> u32 {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let (mut v, mut sz) = (0u32, 4u32);
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced");
    let n = wide(name);
    let r = RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), n.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut v as *mut u32 as *mut _, &mut sz);
    if r == 0 { v } else { default }
}

/// Taskbar icons are left-aligned (Settings > Personalization > Taskbar > Taskbar alignment).
pub unsafe fn left_aligned() -> bool {
    explorer_setting("TaskbarAl", 1) == 0
}

/// Windows' own Widgets button is on the taskbar (Settings > Personalization > Taskbar).
pub unsafe fn widgets_button() -> bool {
    explorer_setting("TaskbarDa", 1) != 0
}

/// Is this one of the taskbar's own parts (rather than a docked widget or other app's window)?
/// Its XAML surface spans the whole bar; only narrower windows count as occupied space.
pub fn is_taskbar_part(c: &Child, taskbar_width: i32) -> bool {
    c.class == "Start" || c.class.starts_with("Windows.UI.") || c.class.starts_with("MSTask")
        || c.class == "ReBarWindow32" || c.class == "TrayNotifyWnd" || c.rc.right - c.rc.left >= taskbar_width / 2
}

/// Screen x of the tray boundary: the tray area's left edge, minus Windows' own Widgets button
/// when it sits there (left-aligned taskbar). Follows the tray live as icons come and go.
pub unsafe fn tray_boundary(tb: HWND, scale: f32) -> i32 {
    let t = rect(tb);
    let notify = FindWindowExW(tb, 0, wide("TrayNotifyWnd").as_ptr(), std::ptr::null());
    let mut rc: RECT = std::mem::zeroed();
    let good = notify != 0
        && GetWindowRect(notify, &mut rc) != 0
        && IsWindowVisible(notify) != 0
        && rc.right > rc.left
        && rc.left > t.left
        && rc.left < t.right;
    let tray_left = {
        let mut last = TRAY_LEFT.lock().unwrap();
        if good {
            *last = Some((tb, rc.left));
            rc.left
        } else {
            last.filter(|(h, _)| *h == tb).map_or(t.right, |(_, x)| x)
        }
    };
    tray_left - if left_aligned() && widgets_button() { (170.0 * scale) as i32 } else { 0 }
}

// ---- layout -----------------------------------------------------------------------

/// Lays out widths right to left from the boundary `b`, `gap` apart, stepping around obstacles
/// (other apps' docked windows, sorted rightmost first). Returns each one's x.
pub fn place(b: i32, gap: i32, obstacles: &[RECT], widths: &[i32]) -> Vec<i32> {
    let mut right = b;
    widths
        .iter()
        .map(|&w| {
            let mut anchor = right;
            for o in obstacles {
                if o.left < anchor && o.right > anchor - w - gap {
                    anchor = o.left;
                }
            }
            right = anchor - w - gap;
            right
        })
        .collect()
}

/// Where a widget being dragged would go if dropped now, as the number of widgets that stay to
/// its right: the slot nearest to where it's held. `widths` are the others in dock order (right
/// to left); `d_left`, `d_width` the dragged one's left edge and width. Used both for the live
/// preview and the drop, so they agree.
///
/// With neighbours of any size this swaps when the dragged one is halfway over its neighbour,
/// i.e. when its leading edge passes the neighbour's centre. It also holds for a wide widget
/// held against the tray (its left edge can't get far, but the far-right slot is the nearest).
pub fn insert_index(b: i32, gap: i32, obstacles: &[RECT], widths: &[i32], d_left: i32, d_width: i32) -> usize {
    sticky_index(b, gap, obstacles, widths, d_left, d_width, None, 0)
}

/// [`insert_index`] that keeps the slot `prev` until another one is nearer by more than `hold`
/// px (hysteresis, as TaskbarWidgets' sticky gap): held right at the halfway point, the
/// neighbours don't swap back and forth.
pub fn sticky_index(b: i32, gap: i32, obstacles: &[RECT], widths: &[i32], d_left: i32, d_width: i32, prev: Option<usize>, hold: i32) -> usize {
    let dist = |k: usize| {
        let mut w = widths.to_vec();
        w.insert(k, d_width);
        (place(b, gap, obstacles, &w)[k] - d_left).abs()
    };
    let best = (0..=widths.len()).min_by_key(|&k| dist(k)).unwrap_or(0);
    match prev.filter(|&p| p <= widths.len()) {
        Some(p) if dist(p) <= dist(best) + hold => p,
        _ => best,
    }
}

// ---- fitting between the taskbar's buttons and the tray ------------------------------

/// A member's widths for [`fit`] (unzoomed px).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Size {
    /// In its own mode (full, or compact when the user chose compact).
    pub full: i32,
    /// Its compact width: the narrowest it can be.
    pub min: i32,
    /// It can also take any width between `min` and `full` (media: the text column narrows).
    pub stretch: bool,
}

impl Size {
    /// A width that can't change.
    pub fn fixed(w: i32) -> Size {
        Size { full: w, min: w, stretch: false }
    }
}

/// What [`fit`] gives a member.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fit {
    /// Its own width.
    Full,
    /// Narrower: draw at this width (the compact width, or for a stretchable member anything
    /// between compact and full).
    Shrunk(i32),
    /// No room: hide it ([`set_squeezed`]) until there is again.
    Hidden,
}

impl Fit {
    pub fn width(self, full: i32) -> i32 {
        match self {
            Fit::Full => full,
            Fit::Shrunk(w) => w,
            Fit::Hidden => 0,
        }
    }
}

/// How the stack fits between the taskbar's own buttons and the tray boundary `b`. `edge` is
/// the leftmost x the stack may use (the buttons' right edge plus the gap; None while they
/// haven't been read: all full). `sizes` in dock order, right to left.
///
/// All full if they fit. If not, in this order, until they do: stretchable members narrow
/// (farthest from the tray first, down to compact), then the others go compact (farthest
/// first). If even that isn't enough, the farthest one hides and the rest start again from
/// full. Every member computes this from the same published numbers, so they agree.
pub fn fit(b: i32, gap: i32, obstacles: &[RECT], edge: Option<i32>, sizes: &[Size]) -> Vec<Fit> {
    let n = sizes.len();
    let mut fits = vec![Fit::Full; n];
    let Some(edge) = edge else { return fits };
    let left_end = |fits: &[Fit]| {
        let ws: Vec<i32> = (0..n).filter(|&i| fits[i] != Fit::Hidden).map(|i| fits[i].width(sizes[i].full)).collect();
        place(b, gap, obstacles, &ws).last().copied().unwrap_or(b)
    };
    loop {
        for f in fits.iter_mut().filter(|f| **f != Fit::Hidden) {
            *f = Fit::Full;
        }
        // stretchable ones narrow; the others go compact; then, if some did, the stretchable
        // ones start again from full and take only what's left (none of it wasted)
        for (stretch, again) in [(true, false), (false, false), (true, true)] {
            for i in (0..n).rev() {
                let s = sizes[i];
                if fits[i] == Fit::Hidden || s.stretch != stretch || s.min >= s.full {
                    continue;
                }
                if again {
                    fits[i] = Fit::Full;
                }
                // a few steps: stepping around an obstacle can make one step not enough
                for _ in 0..4 {
                    let (short, w) = (edge - left_end(&fits), fits[i].width(s.full));
                    if short <= 0 || w <= s.min {
                        break;
                    }
                    fits[i] = Fit::Shrunk(if stretch { (w - short).max(s.min) } else { s.min });
                }
            }
        }
        if left_end(&fits) >= edge {
            return fits;
        }
        match (0..n).rev().find(|&i| fits[i] != Fit::Hidden) {
            Some(i) => fits[i] = Fit::Hidden,
            None => return fits,
        }
    }
}

/// A member's side of the fit: what changed since last time, so it knows what to do.
#[derive(Default, Debug)]
pub struct FitState(Option<Fit>);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FitChange {
    None,
    /// Narrower or wider: redraw at the new width (bus/weather: compact or not).
    Redraw,
    /// No room any more: close the panel, hide, `set_squeezed(true)`, log it.
    Hide,
    /// Room again: `set_squeezed(false)`, redraw, show, log it.
    Show,
}

impl FitState {
    pub fn update(&mut self, f: Fit) -> FitChange {
        let old = self.0.replace(f);
        match (old, f) {
            (Some(o), _) if o == f => FitChange::None,
            (_, Fit::Hidden) => FitChange::Hide,
            (Some(Fit::Hidden), _) => FitChange::Show,
            (None, Fit::Full) => FitChange::None,
            _ => FitChange::Redraw,
        }
    }

    pub fn get(&self) -> Fit {
        self.0.unwrap_or(Fit::Full)
    }

    pub fn hidden(&self) -> bool {
        self.get() == Fit::Hidden
    }

}

/// The taskbar's buttons' right edge as last seen in this process, per taskbar: for the drag
/// limit (`strip::Limits::min`).
static ICONS: Mutex<Option<(isize, Option<i32>)>> = Mutex::new(None);

/// The right edge of the taskbar's own buttons (Start, apps; see [`crate::icons`]). Only the
/// first member in dock order (the leader) reads them through UI Automation; it publishes the
/// edge on its window ([`PROP_ICONS`]) and the others read it there, so all of them fit the
/// stack to the same number. When the leader quits, the next member takes over.
unsafe fn read_icons(tb: HWND, me: HWND, members: &[&Child], boundary: i32) -> Option<i32> {
    let leader = members.first()?;
    let edge = if leader.hwnd == me {
        crate::icons::want(tb);
        // test aid: TASKBAR_DOCK_TEST_ICONS_RIGHT=<screen x> in the leader's environment stands
        // in for the buttons' right edge (a collision without changing the taskbar)
        let test = std::env::var("TASKBAR_DOCK_TEST_ICONS_RIGHT").ok().and_then(|v| v.trim().parse().ok());
        let e = test.or_else(|| crate::icons::buttons(tb).and_then(|(b, at)| crate::icons::right_edge(&b, boundary.min(at))));
        if get_num(me, PROP_ICONS) != e {
            set_num(me, PROP_ICONS, e);
            notify(me); // the others fit again now, not at their next tick
        }
        e
    } else {
        get_num(leader.hwnd, PROP_ICONS)
    };
    *ICONS.lock().unwrap() = Some((tb, edge));
    edge
}

/// The right edge of the taskbar's own buttons, as last read (None: not known). A dragged strip
/// stays right of it (plus the gap).
pub fn icons_right(tb: HWND) -> Option<i32> {
    ICONS.lock().unwrap().filter(|(h, _)| *h == tb).and_then(|(_, e)| e)
}

/// A dock member as seen in a [`Layout`], with its fit.
pub struct Member {
    pub hwnd: HWND,
    pub class: String,
    pub fit: Fit,
}

/// The stack as seen by one member (`me`): the members not being dragged and not squeezed out,
/// in dock order, at their widths for placing ([`placed_width`]); every member with its fit; the
/// obstacles; and a member being dragged, if any.
pub struct Layout {
    pub hwnds: Vec<HWND>,
    pub classes: Vec<String>,
    pub widths: Vec<i32>,
    /// Another member in the stack is changing width ([`publish_drawn`]): follow it.
    pub resizing: bool,
    pub obstacles: Vec<RECT>,
    /// Another member being dragged: (window, width, left, right, rank in the saved order).
    pub dragged: Option<(HWND, i32, i32, i32, usize)>,
    /// Every member in dock order (also squeezed-out and dragged ones), with its fit.
    pub all: Vec<Member>,
}

/// The width a member takes in the stack's placement: while its width animates, the width it's
/// drawn at ([`publish_drawn`]), so its neighbours stay flush on every frame; otherwise its fitted
/// width. (The fit itself always goes by the published widths: who narrows or hides doesn't
/// change in the middle of an animation.)
pub fn placed_width(fit: Fit, size: Size, drawn: Option<i32>) -> i32 {
    drawn.filter(|w| *w > 0).unwrap_or(fit.width(size.full))
}

/// `mine`: my sizes (None: as I published them).
pub unsafe fn layout(tb: HWND, me: HWND, mine: Option<Size>, boundary: i32, gap: i32) -> Layout {
    let t = rect(tb);
    let kids = children(tb);
    // other apps' windows docked into the taskbar: the stack goes around them
    let mut obstacles: Vec<RECT> = kids
        .iter()
        .filter(|c| !c.member && c.hwnd != me && !is_taskbar_part(c, t.right - t.left))
        .map(|c| c.rc)
        .collect();
    obstacles.sort_by_key(|rc| -rc.left);
    let order = read_order();
    // the fit goes by the saved order, the dragged member at its old place: who narrows or
    // hides doesn't change in the middle of a drag
    let mut members: Vec<&Child> = kids.iter().filter(|c| c.member || c.hwnd == me).collect();
    members.sort_by_key(|c| (rank(&order, &c.class), -c.rc.right));
    let sizes: Vec<Size> = members.iter().map(|c| if c.hwnd == me { mine.unwrap_or(c.size) } else { c.size }).collect();
    let icons = read_icons(tb, me, &members, boundary);
    let fits = fit(boundary, gap, &obstacles, icons.map(|r| r + gap), &sizes);
    let stack: Vec<usize> = (0..members.len())
        .filter(|&i| fits[i] != Fit::Hidden && (members[i].hwnd == me || !members[i].dragging))
        .collect();
    Layout {
        hwnds: stack.iter().map(|&i| members[i].hwnd).collect(),
        classes: stack.iter().map(|&i| members[i].class.clone()).collect(),
        widths: stack.iter().map(|&i| placed_width(fits[i], sizes[i], members[i].drawn)).collect(),
        resizing: stack.iter().any(|&i| members[i].hwnd != me && members[i].drawn.is_some()),
        obstacles,
        dragged: kids
            .iter()
            .find(|c| c.member && c.dragging && c.hwnd != me)
            .map(|c| (c.hwnd, c.rc.right - c.rc.left, c.rc.left, c.rc.right, rank(&order, &c.class))),
        all: (0..members.len()).map(|i| Member { hwnd: members[i].hwnd, class: members[i].class.clone(), fit: fits[i] }).collect(),
    }
}

/// Where a member belongs: its screen x for its fitted `width`, whether to slide there (another
/// member is being dragged: the live preview flows), whether to follow (another member's width
/// is animating: go there at once, frame by frame, see `strip::Motion::follow`), and its fit
/// (narrower, or hidden).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Slot {
    pub x: i32,
    pub width: i32,
    pub slide: bool,
    pub follow: bool,
    pub fit: Fit,
}

impl Slot {
    /// The x for a strip drawn `drawn` px wide right now (collapsing, zoomed after a drop):
    /// the same right edge, as the stack hangs from the tray.
    pub fn x_for(&self, drawn: i32) -> i32 {
        self.x + self.width - drawn
    }
}

/// `me`'s slot, for sizes `size` (unzoomed; see [`publish_widths`]). None if `me` isn't among
/// the taskbar's children right now: the list is unreliable then (action center open), so stay
/// put. A hidden (`Fit::Hidden`) member's x is where it is.
pub unsafe fn slot(tb: HWND, me: HWND, size: Size, boundary: i32, gap: i32) -> Option<Slot> {
    let mut l = layout(tb, me, Some(size), boundary, gap);
    let fit = l.all.iter().find(|m| m.hwnd == me)?.fit;
    let Some(mine) = l.hwnds.iter().position(|h| *h == me) else {
        let r = rect(me);
        return Some(Slot { x: r.left, width: r.right - r.left, slide: false, follow: false, fit });
    };
    let Some((dh, dw, d_left, _, _)) = l.dragged else {
        let x = place(boundary, gap, &l.obstacles, &l.widths)[mine];
        return Some(Slot { x, width: l.widths[mine], slide: false, follow: l.resizing, fit });
    };
    let width = l.widths[mine];
    // the slot the dragged one published; computed here only if it hasn't (yet)
    let ins = published_slot(dh, l.widths.len())
        .unwrap_or_else(|| insert_index(boundary, gap, &l.obstacles, &l.widths, d_left, dw));
    l.widths.insert(ins, dw);
    let xs = place(boundary, gap, &l.obstacles, &l.widths);
    Some(Slot { x: xs[if mine >= ins { mine + 1 } else { mine }], width, slide: true, follow: false, fit })
}

/// The slot dragged member `h` published ([`track_drag`]), if it's a valid index for `n` others.
unsafe fn published_slot(h: HWND, n: usize) -> Option<usize> {
    get_num(h, PROP_SLOT).and_then(|s| usize::try_from(s).ok()).filter(|&s| s <= n)
}

/// While `me` is dragged: decides the slot it would drop into, sticky ([`sticky_index`]), and
/// publishes it ([`PROP_SLOT`]); the others' live preview ([`slot`]) and [`drop_reorder`] use
/// it. Call after every move of the dragged strip (does nothing when it isn't dragged). Only the
/// dragged one decides, so the hysteresis has a single memory and every process agrees.
pub unsafe fn track_drag(tb: HWND, me: HWND, boundary: i32, gap: i32) {
    if !is_dragging(me) {
        return;
    }
    let l = layout(tb, me, None, boundary, gap);
    let widths: Vec<i32> = (0..l.hwnds.len()).filter(|&i| l.hwnds[i] != me).map(|i| l.widths[i]).collect();
    let prev = published_slot(me, widths.len());
    let scale = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(tb).max(96) as f32 / 96.0;
    // the gap between two slots' distances grows twice as fast as the strip moves
    let hold = (2.0 * STICKY_PX * scale).round() as i32;
    let r = rect(me);
    let idx = sticky_index(boundary, gap, &l.obstacles, &widths, r.left, r.right - r.left, prev, hold);
    if prev != Some(idx) {
        set_num(me, PROP_SLOT, Some(idx as i32));
        notify(me); // the others may have reflowed for this move already: look again
    }
}

/// After `me` (class `my_class`) is dropped: its place is the slot the live preview had opened.
/// Saved to order.txt so all widgets, and the next start, agree. Squeezed-out members keep
/// their place in the order.
pub unsafe fn drop_reorder(tb: HWND, me: HWND, my_class: &str, boundary: i32, gap: i32) {
    let old = read_order();
    let l = layout(tb, me, None, boundary, gap);
    let others: Vec<usize> = (0..l.hwnds.len()).filter(|&i| l.hwnds[i] != me).collect();
    let widths: Vec<i32> = others.iter().map(|&i| l.widths[i]).collect();
    let r = rect(me);
    let idx = published_slot(me, widths.len())
        .unwrap_or_else(|| insert_index(boundary, gap, &l.obstacles, &widths, r.left, r.right - r.left));
    let mut order: Vec<String> = l.all.iter().filter(|m| m.hwnd != me).map(|m| m.class.clone()).collect();
    // the slot among the shown ones, as a place in the whole order
    let pos = |i: usize| order.iter().position(|c| *c == l.classes[i]);
    let at = match others.get(idx) {
        Some(&i) => pos(i).unwrap_or(order.len()),
        None => others.last().and_then(|&i| pos(i)).map_or(0, |p| p + 1),
    };
    order.insert(at.min(order.len()), my_class.to_string());
    for c in old {
        if !order.contains(&c) {
            order.push(c); // members not running right now keep their saved place at the end
        }
    }
    write_order(&order);
}

// ---- reacting to neighbours ---------------------------------------------------------

/// WinEvent hooks on every process that owns taskbar windows (Explorer for the tray, the other
/// widgets), so a widget can reflow within milliseconds when a neighbour moves or resizes.
/// Call [`Hooks::update`] periodically (e.g. every second) to follow processes restarting.
#[derive(Default)]
pub struct Hooks(HashMap<u32, HWINEVENTHOOK>);

impl Hooks {
    pub unsafe fn update(&mut self, tb: HWND, callback: WINEVENTPROC) {
        let me = std::process::id();
        let mut pids: Vec<u32> = Vec::new();
        if tb != 0 {
            let mut pid = 0u32;
            GetWindowThreadProcessId(tb, &mut pid);
            pids.push(pid);
            for c in children(tb) {
                let mut pid = 0u32;
                GetWindowThreadProcessId(c.hwnd, &mut pid);
                if pid != 0 && pid != me && !pids.contains(&pid) {
                    pids.push(pid);
                }
            }
        }
        // a widget whose strip is gone for now (switched off, discord not in a call) keeps its
        // hook while its process runs: its strip coming back must reach us at once, not on the
        // next update (they overlapped for ~0.5 s, 2026-10-10)
        self.0.retain(|pid, hook| {
            let keep = pids.contains(pid) || alive(*pid);
            if !keep {
                UnhookWinEvent(*hook);
            }
            keep
        });
        for pid in pids {
            if pid != 0 && !self.0.contains_key(&pid) {
                // DESTROY..LOCATIONCHANGE covers create/show/hide/reorder/move/resize
                let hook = SetWinEventHook(EVENT_OBJECT_DESTROY, EVENT_OBJECT_LOCATIONCHANGE, 0, callback, pid, 0, WINEVENT_OUTOFCONTEXT);
                if hook != 0 {
                    self.0.insert(pid, hook);
                }
            }
        }
    }
}

/// The process is still running.
unsafe fn alive(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    const STILL_ACTIVE: u32 = 259;
    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if h == 0 {
        return false;
    }
    let mut code = 0u32;
    let ok = GetExitCodeProcess(h, &mut code) != 0;
    windows_sys::Win32::Foundation::CloseHandle(h);
    ok && code == STILL_ACTIVE
}

/// For a WinEvent callback: is this event about one of the taskbar's own child windows
/// (other than `me`)? Then the stack may have changed.
pub unsafe fn is_neighbour_event(hwnd: HWND, id_object: i32, tb: HWND, me: HWND) -> bool {
    const OBJID_WINDOW: i32 = 0;
    id_object == OBJID_WINDOW && hwnd != 0 && hwnd != me && GetParent(hwnd) == tb
}

#[cfg(test)]
mod tests {
    use super::*;

    // tray boundary 1745, gap 3 (measured on screen: bus 179 px, media 258, weather 75)
    const B: i32 = 1745;
    const GAP: i32 = 3;

    fn ins(others: &[i32], d_left: i32, d_width: i32) -> usize {
        insert_index(B, GAP, &[], others, d_left, d_width)
    }

    #[test]
    fn sticky_slot_needs_ten_px_past_halfway() {
        // media 258 px: slots 1302 (index 1) and 1484 (index 0), halfway 1393; hold 20 = 10 px
        let s = |d_left, prev| sticky_index(B, GAP, &[], &[179, 75], d_left, 258, Some(prev), 20);
        assert_eq!(s(1400, 1), 1); // 7 px past halfway: stays
        assert_eq!(s(1404, 1), 0); // 11 px past: takes it
        assert_eq!(s(1386, 0), 0); // and back the same way
        assert_eq!(s(1382, 0), 1);
        // no memory yet, or a memory from a different stack size: the nearest slot
        assert_eq!(sticky_index(B, GAP, &[], &[179, 75], 1400, 258, None, 20), 0);
        assert_eq!(sticky_index(B, GAP, &[], &[179, 75], 1400, 258, Some(5), 20), 0);
    }

    #[test]
    fn place_stacks_right_to_left() {
        assert_eq!(place(B, GAP, &[], &[179, 75]), vec![1563, 1485]);
    }

    #[test]
    fn at_rest_or_nudged_it_keeps_its_place() {
        // media between bus and weather: its slot is 1302
        assert_eq!(ins(&[179, 75], 1302, 258), 1);
        // picked up and zoomed to 294 px, centred: left edge 18 px further left
        assert_eq!(ins(&[179, 75], 1284, 294), 1);
        assert_eq!(ins(&[179, 75], 1284 + 60, 294), 1); // +80 would be past halfway (1357)
        assert_eq!(ins(&[179, 75], 1284 - 30, 294), 1);
    }

    #[test]
    fn halfway_over_the_right_neighbour_takes_its_place() {
        // slots for a 258 px media: 1302 (index 1), 1484 (index 0); halfway is 1393
        assert_eq!(ins(&[179, 75], 1392, 258), 1);
        assert_eq!(ins(&[179, 75], 1394, 258), 0);
    }

    #[test]
    fn a_wide_strip_held_against_the_tray_takes_the_far_right() {
        // the reported bug: media zoomed to 294 px and held at the tray end (1448..1742)
        assert_eq!(ins(&[179, 75], 1448, 294), 0);
    }

    #[test]
    fn dragged_left_past_everyone_goes_last() {
        assert_eq!(ins(&[258, 75], 1500, 179), 0, "bus still in its slot (1563)");
        assert_eq!(ins(&[258, 75], 1300, 179), 1);
        assert_eq!(ins(&[258, 75], 1100, 179), 2);
    }

    // measured: bus 179 (compact 52), media 258 (compact 112, any width between), weather 88
    // (compact 61); stack order bus, media, weather from the tray
    const BUS: Size = Size { full: 179, min: 52, stretch: false };
    const MEDIA: Size = Size { full: 258, min: 112, stretch: true };
    const WEATHER: Size = Size { full: 88, min: 61, stretch: false };
    const ALL: [Size; 3] = [BUS, MEDIA, WEATHER];

    fn fits(edge: i32) -> Vec<Fit> {
        fit(B, GAP, &[], Some(edge), &ALL)
    }

    #[test]
    fn with_room_or_unknown_buttons_all_are_full() {
        // all full: 179 + 258 + 88 + 3 gaps = 534, so the stack starts at 1211
        assert_eq!(fits(366), vec![Fit::Full; 3]);
        assert_eq!(fits(1211), vec![Fit::Full; 3]);
        assert_eq!(fit(B, GAP, &[], None, &ALL), vec![Fit::Full; 3]);
    }

    #[test]
    fn media_narrows_first_then_goes_compact() {
        assert_eq!(fits(1261), vec![Fit::Full, Fit::Shrunk(208), Fit::Full]);
        // 146 px short: media can give only 146 (258 → 112)
        assert_eq!(fits(1357), vec![Fit::Full, Fit::Shrunk(112), Fit::Full]);
    }

    #[test]
    fn then_the_others_go_compact_farthest_first_and_media_takes_the_rest() {
        // media compact starts at 1357; weather compact gives 27 more (1384), bus 127 (1511)
        assert_eq!(fits(1384), vec![Fit::Full, Fit::Shrunk(112), Fit::Shrunk(61)]);
        // weather compact leaves 14 px to spare: media gets them back
        assert_eq!(fits(1370), vec![Fit::Full, Fit::Shrunk(126), Fit::Shrunk(61)]);
        assert_eq!(fits(1400), vec![Fit::Shrunk(52), Fit::Shrunk(223), Fit::Shrunk(61)]);
        assert_eq!(fits(1511), vec![Fit::Shrunk(52), Fit::Shrunk(112), Fit::Shrunk(61)]);
    }

    #[test]
    fn then_the_farthest_hides_and_the_rest_start_from_full() {
        // all compact start at 1511: past that weather hides, and bus and media fit again
        assert_eq!(fits(1520), vec![Fit::Shrunk(52), Fit::Shrunk(167), Fit::Hidden]);
        // only bus, compact (1690); then nothing
        assert_eq!(fits(1690), vec![Fit::Shrunk(52), Fit::Hidden, Fit::Hidden]);
        assert_eq!(fits(1700), vec![Fit::Hidden; 3]);
    }

    #[test]
    fn a_member_in_compact_mode_already_cant_shrink() {
        let sizes = [Size::fixed(52), MEDIA, WEATHER];
        assert_eq!(fit(B, GAP, &[], Some(1520), &sizes), vec![Fit::Full, Fit::Shrunk(167), Fit::Hidden]);
    }

    #[test]
    fn a_widgets_state_says_what_changed() {
        let mut s = FitState::default();
        assert_eq!(s.update(Fit::Full), FitChange::None);
        assert_eq!(s.update(Fit::Shrunk(52)), FitChange::Redraw);
        assert_eq!(s.update(Fit::Shrunk(52)), FitChange::None);
        assert_eq!(s.update(Fit::Hidden), FitChange::Hide);
        assert!(s.hidden());
        assert_eq!(s.update(Fit::Full), FitChange::Show);
    }

    #[test]
    fn neighbours_stay_flush_with_a_strip_changing_width() {
        // bus against the tray switches to compact (179 → 52) and back, weather on its left; at
        // each frame bus is drawn `w` wide and keeps its right edge
        let flush = |w: i32| {
            let ws = [placed_width(Fit::Full, BUS, Some(w)), placed_width(Fit::Full, WEATHER, None)];
            let xs = place(B, GAP, &[], &ws);
            assert_eq!(xs[0] + w, B - GAP, "bus keeps its right edge");
            assert_eq!(xs[1] + 88 + GAP, xs[0], "weather flush with bus at {w} px");
        };
        for t in 0..=10 {
            let e = crate::anim::ease_dock(t as f32 / 10.0);
            flush(179 - (127.0 * e).round() as i32); // shrinking
            flush(52 + (127.0 * e).round() as i32); // growing
        }
        // by the published (final) widths, mid-switch weather would sit over the still-wide bus
        let bus_left = B - GAP - 120;
        let weather_right = place(B, GAP, &[], &[52, 88])[1] + 88;
        assert!(weather_right > bus_left);
        // no animation running: the fitted width
        assert_eq!(placed_width(Fit::Shrunk(52), BUS, None), 52);
        assert_eq!(placed_width(Fit::Full, BUS, Some(0)), 179);
    }

    #[test]
    fn slots_step_around_obstacles() {
        // another app's window at 1500..1600: the stack goes around it
        let obstacles = [RECT { left: 1500, top: 0, right: 1600, bottom: 48 }];
        assert_eq!(place(B, GAP, &obstacles, &[100, 75]), vec![1642, 1422]);
        assert_eq!(insert_index(B, GAP, &obstacles, &[100, 75], 1400, 75), 1);
    }
}
