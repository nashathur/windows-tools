//! Where the strip goes when it isn't in the dock stack: on the left of the taskbar, or
//! floating. The arithmetic is pure (tested below); the Win32 readers feed it.

use crate::dock;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
};

/// Width of Windows' own Widgets button at the taskbar's left end, px at 100%.
const WIDGETS_BUTTON: f32 = 170.0;

/// Left side: the first x (screen) from `start` where a strip `width` wide doesn't overlap any
/// of the `occupied` spans (left, right), keeping `gap` after each.
pub(crate) fn left_x(start: i32, gap: i32, mut occupied: Vec<(i32, i32)>, width: i32) -> i32 {
    occupied.sort();
    let mut x = start;
    for (l, r) in occupied {
        if l < x + width && r > x {
            x = r + gap;
        }
    }
    x
}

/// Floating beside the dock: left of the tray `boundary` and of every docked member (`lefts`:
/// their left edges), `gap` apart.
pub(crate) fn beside_dock(boundary: i32, lefts: &[i32], width: i32, gap: i32) -> i32 {
    lefts.iter().copied().fold(boundary, i32::min) - width - gap
}

/// Floating without a taskbar: the bottom right corner of a `screen` (w, h), `margin` in.
pub(crate) fn corner(screen: (i32, i32), width: i32, margin: i32) -> i32 {
    screen.0 - width - margin
}

/// Left side, live: x for a strip `me` drawn `width` wide on taskbar `tb`.
pub(crate) unsafe fn left_side(tb: HWND, me: HWND, width: i32, gap: i32, scale: f32) -> i32 {
    let t = dock::rect(tb);
    let widgets = if !dock::left_aligned() && dock::widgets_button() {
        (WIDGETS_BUTTON * scale) as i32
    } else {
        0
    };
    let occupied = dock::children(tb)
        .iter()
        .filter(|c| c.hwnd != me && !c.squeezed && !dock::is_taskbar_part(c, t.right - t.left))
        .map(|c| (c.rc.left, c.rc.right))
        .collect();
    left_x(t.left + gap + widgets, gap, occupied, width)
}

/// Floating, live: x for a strip `width` wide beside the dock on `tb` (0: no taskbar).
pub(crate) unsafe fn floating_x(tb: HWND, width: i32, gap: i32, scale: f32) -> i32 {
    if tb == 0 {
        return corner(
            (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)),
            width,
            (8.0 * scale).round() as i32,
        );
    }
    let lefts: Vec<i32> = dock::children(tb)
        .iter()
        .filter(|c| c.member && !c.squeezed)
        .map(|c| c.rc.left)
        .collect();
    beside_dock(dock::tray_boundary(tb, scale), &lefts, width, gap)
}

/// A saved floating x ([`super::FloatAt::Saved`]).
pub(crate) fn read_saved(path: &std::path::Path) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub(crate) fn write_saved(path: &std::path::Path, x: i32) {
    let _ = std::fs::write(path, x.to_string());
}

/// The screen width, for a floating strip's drag limits.
pub(crate) fn screen_width() -> i32 {
    unsafe { GetSystemMetrics(SM_CXSCREEN) }
}

/// Is a full-screen app in front? A floating strip hides then (the taskbar does that for an
/// attached one). The desktop and the taskbars don't count.
pub(crate) unsafe fn fullscreen_app_active() -> bool {
    let h = GetForegroundWindow();
    if h == 0 {
        return false;
    }
    let mut cls = [0u16; 64];
    let n = GetClassNameW(h, cls.as_mut_ptr(), cls.len() as i32).max(0) as usize;
    let cls = String::from_utf16_lossy(&cls[..n]);
    if [
        "Progman",
        "WorkerW",
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
    ]
    .contains(&cls.as_str())
    {
        return false;
    }
    covers_screen(
        dock::rect(h),
        (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)),
    )
}

pub(crate) fn covers_screen(r: RECT, screen: (i32, i32)) -> bool {
    r.left <= 0 && r.top <= 0 && r.right >= screen.0 && r.bottom >= screen.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_side_goes_around_occupied_spans() {
        assert_eq!(left_x(13, 3, vec![], 75), 13);
        // a docked window from 50 to 120: after it
        assert_eq!(left_x(13, 3, vec![(50, 120)], 75), 123);
        // and one right after that too (unsorted)
        assert_eq!(left_x(13, 3, vec![(150, 200), (50, 120)], 75), 203);
        // a span further right with room before it: stays
        assert_eq!(left_x(13, 3, vec![(400, 500)], 75), 13);
    }

    #[test]
    fn floating_beside_the_dock_or_in_the_corner() {
        assert_eq!(beside_dock(1745, &[], 100, 3), 1642);
        assert_eq!(beside_dock(1745, &[1563, 1485], 100, 3), 1382);
        assert_eq!(corner((1920, 1080), 100, 8), 1812);
    }

    #[test]
    fn full_screen_means_covering_the_screen() {
        let r = |l, t, rr, b| RECT {
            left: l,
            top: t,
            right: rr,
            bottom: b,
        };
        assert!(covers_screen(r(0, 0, 1920, 1080), (1920, 1080)));
        assert!(
            covers_screen(r(-8, -8, 1928, 1088), (1920, 1080)),
            "maximised borders"
        );
        assert!(
            !covers_screen(r(0, 0, 1920, 1032), (1920, 1080)),
            "above the taskbar"
        );
    }
}
