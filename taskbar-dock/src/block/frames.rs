//! Animation frames on the display's own refresh (glide, zoom, compact switch, panel fade, cover
//! feel). A `SetTimer` of 10 ms fires every ~15.6 ms (Windows' timer tick): ~64 fps on any
//! monitor, while the taskbar's own slides run at the display's rate (144 Hz here, 2026-10-11).
//! Instead a worker thread waits for each composition (`DwmFlush`, once per refresh) and posts
//! [`message`] to Block's main window, only while an animation runs ([`run`]). One frame message
//! at most is in the queue ([`done`]), so a slow frame never piles them up. Animations go by the
//! time elapsed, not by frames, so their duration doesn't depend on the rate.

use crate::wide;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Condvar, Mutex, Once};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, RegisterWindowMessageW};

/// Without composition (`DwmFlush` failing): frames this far apart, as the old timer.
const FALLBACK: Duration = Duration::from_millis(10);
/// Never closer than this (DwmFlush returning at once, e.g. while the display is off): 250 fps.
const MIN_GAP: Duration = Duration::from_millis(4);

static WANTED: Mutex<bool> = Mutex::new(false);
static WAKE: Condvar = Condvar::new();
static TARGET: AtomicIsize = AtomicIsize::new(0);
static PENDING: AtomicBool = AtomicBool::new(false);
static START: Once = Once::new();

/// The message each frame posts to the main window.
pub(super) fn message() -> u32 {
    static MSG: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(wide("TaskbarDock.Frame").as_ptr()) })
}

/// Frames to `main` while `on` (some animation runs), none otherwise.
pub(super) fn run(main: HWND, on: bool) {
    TARGET.store(main, Ordering::Relaxed);
    *WANTED.lock().unwrap() = on;
    if on {
        START.call_once(|| {
            let _ = std::thread::Builder::new().name("block-frames".into()).spawn(worker);
        });
        WAKE.notify_one();
    }
}

/// The frame message was taken: the next refresh may post another.
pub(super) fn done() {
    PENDING.store(false, Ordering::Release);
}

fn worker() {
    loop {
        {
            let mut wanted = WANTED.lock().unwrap();
            while !*wanted {
                wanted = WAKE.wait(wanted).unwrap();
            }
        }
        let t0 = Instant::now();
        let synced = unsafe { DwmFlush() } >= 0;
        std::thread::sleep(rest(synced, t0.elapsed()));
        if *WANTED.lock().unwrap() && !PENDING.swap(true, Ordering::AcqRel) {
            let main = TARGET.load(Ordering::Relaxed);
            if main == 0 || unsafe { PostMessageW(main, message(), 0, 0) } == 0 {
                PENDING.store(false, Ordering::Release); // the window is gone: nothing to take it
            }
        }
    }
}

/// How long to wait after a `DwmFlush` that took `waited` (`synced`: it worked), so frames are
/// never closer than [`MIN_GAP`], and [`FALLBACK`] apart without composition.
fn rest(synced: bool, waited: Duration) -> Duration {
    let gap = if synced { MIN_GAP } else { FALLBACK };
    gap.saturating_sub(waited)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_follow_the_refresh_but_never_race() {
        let ms = Duration::from_millis;
        // a refresh waited for: straight on (144 Hz: 6.9 ms; 60 Hz: 16.7 ms)
        assert_eq!(rest(true, Duration::from_micros(6900)), Duration::ZERO);
        assert_eq!(rest(true, ms(16)), Duration::ZERO);
        // returned at once (display off): no faster than 250 fps
        assert_eq!(rest(true, ms(1)), ms(3));
        // no composition: the old 10 ms timer's pace
        assert_eq!(rest(false, Duration::ZERO), ms(10));
    }
}
