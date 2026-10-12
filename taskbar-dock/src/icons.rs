//! The taskbar's own buttons (Start, search, task view, pinned and running apps), read through
//! UI Automation, so the dock stack never covers them.
//!
//! The buttons are XAML inside Explorer's taskbar window, not windows of their own, so only UI
//! Automation (what screen readers use; read-only) sees them. The query is the one
//! TaskbarWidgets (github.com/pfcdev/TaskbarWidgets) uses: every Button-type element under the
//! taskbar, kept if it sits in the taskbar's band. It runs on a worker thread, every
//! [`SCAN_EVERY`], and only while someone asked for it in the last [`IDLE_AFTER`] (the dock's
//! leader: see `dock::layout`). A scan that finds nothing (Explorer busy, the action center
//! open) never replaces the last good one.
//!
//! Each scan keeps the tray boundary of its time: the tray's own buttons are Buttons too, and a
//! scan is up to [`SCAN_EVERY`] old. When the tray shrinks (e.g. the microphone-in-use icon
//! goes away as a call ends), the old scan's tray buttons sit left of the new boundary and
//! would read as app buttons right up to it: no room, every widget hid for a moment. So the
//! edge is judged against the smaller of the two boundaries ([`buttons`]).
//!
//! Measured 2026-10-09 (left-aligned taskbar, 7 apps): 44 px per app button, Start 0..55, the
//! last app ending at 363; the tray's own buttons (from the tray boundary on) are Buttons too,
//! and [`right_edge`] leaves them out.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::RECT;

pub const SCAN_EVERY: Duration = Duration::from_millis(500);
pub const IDLE_AFTER: Duration = Duration::from_secs(3);

static TASKBAR: AtomicIsize = AtomicIsize::new(0);
static WANTED: Mutex<Option<Instant>> = Mutex::new(None);
/// The last good scan: (taskbar, button rectangles in screen coordinates, the tray boundary
/// while it ran).
static BUTTONS: Mutex<Option<(isize, Vec<RECT>, i32)>> = Mutex::new(None);
static START: Once = Once::new();

/// Keep scanning taskbar `tb` (call at least every [`IDLE_AFTER`]); starts the worker once.
pub fn want(tb: isize) {
    TASKBAR.store(tb, Ordering::Relaxed);
    *WANTED.lock().unwrap() = Some(Instant::now());
    START.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("taskbar-icons".into())
            .spawn(worker);
    });
}

/// The last good scan of taskbar `tb`, if there is one, and the tray boundary while it ran
/// (pass the smaller of that and today's to [`right_edge`]).
pub fn buttons(tb: isize) -> Option<(Vec<RECT>, i32)> {
    BUTTONS
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(h, _, _)| *h == tb)
        .map(|(_, b, at)| (b.clone(), *at))
}

/// The right edge of the taskbar's own buttons left of the tray boundary `boundary`: where the
/// stack must end. Buttons reaching past the boundary (the tray's, Windows' Widgets button when
/// it sits there) don't count. (Our own strips aren't Buttons to UI Automation; if one covers
/// an app button, that button still counts: that's the collision to avoid.)
pub fn right_edge(buttons: &[RECT], boundary: i32) -> Option<i32> {
    buttons
        .iter()
        .filter(|b| b.right > b.left && b.right <= boundary)
        .map(|b| b.right)
        .max()
}

/// Keeps the button rectangles that sit in the taskbar's band (at least half of each one's
/// height inside it), as TaskbarWidgets does.
pub fn in_band(rects: Vec<RECT>, taskbar: &RECT) -> Vec<RECT> {
    rects
        .into_iter()
        .filter(|r| {
            let h = r.bottom - r.top;
            let overlap = r.bottom.min(taskbar.bottom) - r.top.max(taskbar.top);
            r.right > r.left && h > 0 && overlap > 0 && overlap * 2 >= h
        })
        .collect()
}

fn worker() {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut uia: Option<IUIAutomation> = None;
    loop {
        std::thread::sleep(SCAN_EVERY);
        let wanted = WANTED
            .lock()
            .unwrap()
            .is_some_and(|t| t.elapsed() < IDLE_AFTER);
        let tb = TASKBAR.load(Ordering::Relaxed);
        if !wanted || tb == 0 {
            continue;
        }
        if uia.is_none() {
            uia = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() };
        }
        let Some(u) = &uia else { continue };
        // the boundary before and after: if the tray moved during the scan, the smaller one
        let boundary = || unsafe { crate::dock::tray_boundary(tb, crate::system::scale_for(tb)) };
        let before = boundary();
        match unsafe { scan(u, tb) } {
            Some(found) if !found.is_empty() => {
                *BUTTONS.lock().unwrap() = Some((tb, found, before.min(boundary())))
            }
            _ => {} // keep the last good scan
        }
    }
}

/// Every Button-type element under the taskbar window, in its band.
unsafe fn scan(
    uia: &windows::Win32::UI::Accessibility::IUIAutomation,
    tb: isize,
) -> Option<Vec<RECT>> {
    use windows::core::VARIANT;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Accessibility::{
        TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_ControlTypePropertyId,
    };
    let root = uia.ElementFromHandle(HWND(tb as *mut _)).ok()?;
    let cond = uia
        .CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_ButtonControlTypeId.0),
        )
        .ok()?;
    let found = root.FindAll(TreeScope_Descendants, &cond).ok()?;
    let mut rects = Vec::new();
    for i in 0..found.Length().unwrap_or(0) {
        let Ok(r) = found
            .GetElement(i)
            .and_then(|e| e.CurrentBoundingRectangle())
        else {
            continue;
        };
        rects.push(RECT {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        });
    }
    Some(in_band(rects, &crate::dock::rect(tb)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(left: i32, right: i32) -> RECT {
        RECT {
            left,
            top: 1032,
            right,
            bottom: 1080,
        }
    }

    #[test]
    fn the_edge_is_the_last_app_button_left_of_the_tray() {
        // measured: Start 0..55, apps up to 363, tray buttons from the boundary (1745) on
        let b = [
            r(0, 55),
            r(55, 99),
            r(319, 363),
            r(1745, 1777),
            r(1781, 1805),
            r(1908, 1920),
        ];
        assert_eq!(right_edge(&b, 1745), Some(363));
    }

    #[test]
    fn windows_widgets_button_by_the_tray_is_not_an_app() {
        // left-aligned with Widgets: the boundary is 170 px left of the tray, the button inside
        let b = [r(0, 55), r(55, 99), r(1590, 1740)];
        assert_eq!(right_edge(&b, 1575), Some(99));
    }

    #[test]
    fn a_shrunk_tray_doesnt_turn_its_old_buttons_into_apps() {
        // scanned with the tray from 1745; then its first icon (the mic) went away: boundary 1777
        let b = [r(0, 55), r(319, 363), r(1745, 1777), r(1781, 1805)];
        assert_eq!(
            right_edge(&b, 1777),
            Some(1777),
            "judged by today's boundary alone"
        );
        assert_eq!(right_edge(&b, 1777.min(1745)), Some(363));
    }

    #[test]
    fn no_buttons_no_edge() {
        assert_eq!(right_edge(&[], 1745), None);
    }

    #[test]
    fn only_buttons_in_the_taskbar_band() {
        let tb = r(0, 1920);
        let popup = RECT {
            left: 100,
            top: 600,
            right: 200,
            bottom: 640,
        }; // a flyout's button
        let half = RECT {
            left: 300,
            top: 1010,
            right: 340,
            bottom: 1050,
        }; // 18 of 40 px inside
        assert_eq!(in_band(vec![r(0, 55), popup, half], &tb).len(), 1);
    }
}
