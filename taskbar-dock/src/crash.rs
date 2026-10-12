//! When a widget crashes: write why to its log, and start it again (from TaskbarWidgets' provider
//! supervisor: restart with a growing wait, give up when it keeps crashing).
//!
//! Every widget builds with `panic = "abort"`, so a panic in any thread ends the whole process
//! at once. [`log_panics`] writes the panic message to the widget's log first. The launcher
//! (`taskbar-dock-start`, src/bin) stays running after sign-in and starts a widget again when it
//! ended with a crash code ([`is_crash`]); a Quit (exit code 0) or being stopped (Task Manager,
//! dev.ps1: exit code 1 or -1) is left alone. Waits and the give-up rule: [`Restarts`].

use std::time::Duration;

/// Sends panic messages (with file and line) to `log` before the process aborts. Call first
/// thing in `main`; `log` is the widget's own log function.
pub fn log_panics(log: fn(&str)) {
    std::panic::set_hook(Box::new(move |info| log(&format!("crashed: {info}"))));
}

/// An exit code that means a crash: an NTSTATUS error (0xC0000xxx), such as an abort after a
/// panic (0xC0000409), an access violation (0xC0000005) or a stack overflow (0xC00000FD).
/// Not 0 (Quit), not 1 or -1 (stopped by Task Manager or `Stop-Process`).
pub fn is_crash(code: i32) -> bool {
    (code as u32) & 0xF000_0000 == 0xC000_0000 && code != -1
}

/// Crashes within this window count towards giving up.
pub const WINDOW_S: u64 = 600;
/// More crashes than this within [`WINDOW_S`]: stop restarting (something is broken for good).
pub const MAX_CRASHES: usize = 5;

/// One widget's recent crashes (seconds on some clock).
#[derive(Default)]
pub struct Restarts(Vec<u64>);

impl Restarts {
    /// A crash at `now`: how long to wait before starting it again (2 s, 4 s, 8 s, … for crashes
    /// close together, at most 5 min), or None to give up.
    pub fn crashed(&mut self, now: u64) -> Option<Duration> {
        self.0.retain(|&t| now.saturating_sub(t) < WINDOW_S);
        self.0.push(now);
        let n = self.0.len();
        (n <= MAX_CRASHES).then(|| Duration::from_secs((1u64 << n).min(300)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_codes() {
        assert!(is_crash(0xC000_0409u32 as i32)); // abort after a panic
        assert!(is_crash(0xC000_0005u32 as i32));
        assert!(!is_crash(0)); // Quit
        assert!(!is_crash(1)); // Task Manager
        assert!(!is_crash(-1)); // Stop-Process
    }

    #[test]
    fn waits_grow_then_give_up() {
        let mut r = Restarts::default();
        let waits: Vec<_> = (0..6)
            .map(|i| r.crashed(100 + i * 10).map(|d| d.as_secs()))
            .collect();
        assert_eq!(
            waits,
            vec![Some(2), Some(4), Some(8), Some(16), Some(32), None]
        );
    }

    #[test]
    fn old_crashes_are_forgotten() {
        let mut r = Restarts::default();
        r.crashed(0);
        r.crashed(10);
        assert_eq!(r.crashed(10 + WINDOW_S).map(|d| d.as_secs()), Some(2));
    }
}
