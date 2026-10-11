//! Easing for the widgets' animations (slide, zoom, fade).

/// Progress `t` (0..1) eased out cubically: fast start, gentle stop.
pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Progress `t` (0..1) eased out quadratically (used for fades).
pub fn ease_out_quad(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(2)
}

/// A CSS-style cubic-bezier easing curve from (0, 0) to (1, 1) with control points (x1, y1) and
/// (x2, y2); x1 and x2 within 0..1.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CubicBezier {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl CubicBezier {
    /// The eased progress at progress `t` (0..1): solves x(s) = t for the curve parameter s
    /// (bisection: x(s) always rises for x1, x2 in 0..1), then gives y(s).
    pub fn at(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if t == 0.0 || t == 1.0 {
            return t;
        }
        let b = |p1: f32, p2: f32, s: f32| 3.0 * (1.0 - s) * (1.0 - s) * s * p1 + 3.0 * (1.0 - s) * s * s * p2 + s * s * s;
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..24 {
            let s = (lo + hi) / 2.0;
            if b(self.x1, self.x2, s) < t { lo = s } else { hi = s }
        }
        b(self.y1, self.y2, (lo + hi) / 2.0)
    }
}

/// The Windows 11 taskbar's own button slide (an app closes, the buttons to its right glide
/// over): a slow start, a fast middle and a long gentle stop. Fitted 2026-10-11 to 3 measured
/// 44 px slides (`_tmp\taskbar_slide.ps1`, 144 Hz frames): over [`DOCK_MS`] within 0.7 px of
/// every frame (best fit 304–312 ms: 0.3 px). `ease_out_cubic` was 4.4 px off at best.
pub const DOCK: CubicBezier = CubicBezier { x1: 0.5, y1: 0.0, x2: 0.0, y2: 1.0 };
/// How long the taskbar's button slide takes (measured with [`DOCK`]): the default `glide_ms`.
pub const DOCK_MS: f32 = 300.0;

/// Progress `t` (0..1) eased like the taskbar's own buttons ([`DOCK`]): the strips' glide and
/// the compact switch.
pub fn ease_dock(t: f32) -> f32 {
    DOCK.at(t)
}

/// Progress of an animation that started `elapsed_ms` ago and lasts `duration_ms`
/// (1.0 straight away when the duration is 0, e.g. animations turned off).
pub fn progress(elapsed_ms: f32, duration_ms: f32) -> f32 {
    if duration_ms <= 0.0 { 1.0 } else { (elapsed_ms / duration_ms).clamp(0.0, 1.0) }
}

/// How much a hovered cover / logo grows (rule "Cover feel" in CLAUDE.md), and how much it
/// shrinks while pressed (a press from rest dips below 1; pressed while hovered goes from the
/// grown size down to this).
pub const HOVER_GROW: f32 = 1.06;
pub const PRESS_SHRINK: f32 = 0.96;
/// Frame interval (ms) of the timer that drives a [`Feel`] while it moves.
pub const FEEL_FRAME_MS: u32 = 16;

/// The size factor of a strip's cover or logo as the mouse treats it: it grows when hovered,
/// dips when pressed, and eases between them (exponentially, so a reversal mid-way is smooth).
/// Pure state, like [`crate::panel::PanelBehaviour`]: the widget calls [`Feel::set`] when the
/// mouse state changes, runs a [`FEEL_FRAME_MS`] timer while [`Feel::animating`], and calls
/// [`Feel::tick`] on each frame; [`Feel::value`] is the factor to draw the cover and ring at
/// (see `draw::grow_box`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feel {
    value: f32,
    target: f32,
    /// Time constant (ms) of the current move: quick to press, a little slower to settle.
    tau: f32,
    last: Option<std::time::Instant>,
}

impl Default for Feel {
    fn default() -> Self {
        Feel::new()
    }
}

impl Feel {
    pub const fn new() -> Feel {
        Feel { value: 1.0, target: 1.0, tau: 80.0, last: None }
    }

    /// The mouse is over the cover (`hover`), and its button is down on it (`pressed`). Pass
    /// false for both while the strip isn't at rest (`strip::Motion::calm`), so a drag from the
    /// cover doesn't fight the strip's own zoom. True if the target changed (redraw); cheap to
    /// call on every mouse move.
    pub fn set(&mut self, hover: bool, pressed: bool) -> bool {
        let target = match (hover, pressed) {
            (_, true) => PRESS_SHRINK,
            (true, false) => HOVER_GROW,
            _ => 1.0,
        };
        if target == self.target {
            return false;
        }
        self.target = target;
        self.tau = if pressed { 45.0 } else { 80.0 };
        self.last = None;
        true
    }

    /// Jumps to the target (animations turned off).
    pub fn snap(&mut self) {
        self.value = self.target;
        self.last = None;
    }

    /// The factor to draw at now (1.0 = normal size).
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Is it still moving (a timer must keep running)?
    pub fn animating(&self) -> bool {
        (self.value - self.target).abs() > 0.002
    }

    /// Moves toward the target for the time since the last tick (one frame the first time);
    /// true while still moving.
    pub fn tick(&mut self, now: std::time::Instant) -> bool {
        let dt = self.last.map_or(FEEL_FRAME_MS as f32, |l| now.saturating_duration_since(l).as_secs_f32() * 1000.0).min(100.0);
        self.value += (self.target - self.value) * (1.0 - (-dt / self.tau).exp());
        if (self.value - self.target).abs() <= 0.002 {
            self.value = self.target;
            self.last = None;
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// One cycle of [`pulse`], in seconds.
pub const PULSE_S: f32 = 2.4;

/// A slow, smooth pulse (0..1) at `t` seconds: 0 → 1 → 0 each [`PULSE_S`], along a cosine, so it
/// never jumps. For "alive, but nothing else to show", e.g. a progress ring with no progress.
pub fn pulse(t: f32) -> f32 {
    (1.0 - (std::f32::consts::TAU * t / PULSE_S).cos()) / 2.0
}

/// After a [`pulse`] stops at `from` (0..1): its level `t` seconds later, gliding down to 0
/// along the pulse's own falling curve and pace (at most half a [`PULSE_S`]), so it doesn't
/// jump; 0 once there.
pub fn pulse_settle(from: f32, t: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    // the angle on the pulse's falling half (π..2π) where it has this level, then onward
    let start = TAU - (1.0 - 2.0 * from.clamp(0.0, 1.0)).acos();
    let angle = (start + TAU * t.max(0.0) / PULSE_S).min(TAU);
    if angle >= TAU { 0.0 } else { (1.0 - angle.max(PI).cos()) / 2.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feel_grows_on_hover_dips_on_press_and_settles() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut f = Feel::new();
        assert_eq!((f.value(), f.animating()), (1.0, false));
        f.set(true, false);
        assert!(f.animating());
        let mut prev = f.value();
        for i in 1..=200 {
            let more = f.tick(t0 + Duration::from_millis(16 * i));
            assert!(f.value() >= prev && f.value() <= HOVER_GROW); // eases up, never overshoots
            prev = f.value();
            if !more {
                break;
            }
        }
        assert_eq!((f.value(), f.animating()), (HOVER_GROW, false));
        // pressed while hovered: down to the press size, then released: back up to grown
        let mut now = t0 + Duration::from_secs(10);
        let mut run = |f: &mut Feel| {
            for _ in 0..500 {
                now += Duration::from_millis(16);
                if !f.tick(now) {
                    return;
                }
            }
            panic!("never settled");
        };
        f.set(true, true);
        run(&mut f);
        assert_eq!(f.value(), PRESS_SHRINK);
        f.set(true, false);
        run(&mut f);
        assert_eq!(f.value(), HOVER_GROW);
        // the mouse leaves: back to normal; animations off: straight there
        f.set(false, false);
        f.snap();
        assert_eq!((f.value(), f.animating()), (1.0, false));
    }

    #[test]
    fn dock_easing_matches_the_measured_taskbar_slide() {
        assert_eq!((ease_dock(0.0), ease_dock(1.0)), (0.0, 1.0));
        assert_eq!((ease_dock(-1.0), ease_dock(2.0)), (0.0, 1.0));
        // never goes back, never overshoots
        let mut prev = 0.0;
        for i in 1..=1000 {
            let v = ease_dock(i as f32 / 1000.0);
            assert!(v >= prev - 1e-6 && v <= 1.0 + 1e-6);
            prev = v;
        }
        // px still to go of a 44 px slide, measured on 144 Hz frames (slide 2; the first frame that
        // moved is 22 ms in, the best start for 300 ms). Whole px measured (±0.5) plus the fit's
        // 0.7: within 1.2 px
        let measured = [(22.0, 43.0), (35.6, 42.0), (49.4, 41.0), (63.2, 38.0), (77.0, 33.0), (90.8, 25.0),
            (97.8, 20.0), (111.6, 15.0), (132.7, 10.0), (153.5, 7.0), (174.3, 4.0), (195.1, 3.0),
            (222.9, 2.0), (257.6, 1.0)];
        for (ms, left) in measured {
            let model = 44.0 * (1.0 - ease_dock(ms / DOCK_MS));
            assert!((model - left).abs() <= 1.2, "{ms} ms: model {model:.1} px, measured {left}");
        }
    }

    #[test]
    fn pulse_settle_glides_down_at_the_pulse_pace() {
        assert!((pulse_settle(1.0, 0.0) - 1.0).abs() < 1e-4);
        assert!((pulse_settle(0.5, 0.0) - 0.5).abs() < 1e-4);
        // from full it takes half a cycle, like the pulse falling
        assert!((pulse_settle(1.0, PULSE_S / 4.0) - 0.5).abs() < 1e-3);
        assert_eq!(pulse_settle(1.0, PULSE_S / 2.0), 0.0);
        // from lower down it's there sooner
        assert_eq!(pulse_settle(0.5, PULSE_S / 4.0), 0.0);
        // always going down, without jumps
        let mut prev = pulse_settle(0.8, 0.0);
        for i in 1..=1500 {
            let v = pulse_settle(0.8, i as f32 * 0.001);
            assert!(v <= prev && prev - v < 0.01);
            prev = v;
        }
    }

    #[test]
    fn pulse_is_slow_and_smooth() {
        assert!(pulse(0.0) < 1e-4);
        assert!((pulse(PULSE_S / 2.0) - 1.0).abs() < 1e-4);
        assert!(pulse(PULSE_S) < 1e-4);
        // no jumps: small steps in time give small changes
        let mut prev = pulse(0.0);
        for i in 1..=2400 {
            let v = pulse(i as f32 * 0.001);
            assert!((v - prev).abs() < 0.01);
            prev = v;
        }
    }
}
