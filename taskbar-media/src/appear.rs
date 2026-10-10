//! The strip fading in and out (`fade_ms`, 300 ms by default; instant with Windows' animations
//! off). A fade can turn round half-way (playback resumed while fading out): it goes back from
//! where it is, at the same speed.

use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct Appear {
    from: f32,
    to: f32,
    start: Instant,
    /// Time for a whole 0 → 1 fade.
    ms: f32,
}

impl Appear {
    /// Fully hidden (opacity 0).
    pub fn hidden(now: Instant) -> Appear {
        Appear { from: 0.0, to: 0.0, start: now, ms: 0.0 }
    }

    /// Opacity now, 0..1.
    pub fn value(&self, now: Instant) -> f32 {
        let span = (self.to - self.from).abs();
        if self.ms <= 0.0 || span <= 0.0 {
            return self.to;
        }
        let t = (now.duration_since(self.start).as_secs_f32() * 1000.0 / (self.ms * span)).min(1.0);
        self.from + (self.to - self.from) * t
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    /// Still changing?
    pub fn running(&self, now: Instant) -> bool {
        self.value(now) != self.to
    }

    /// Fades to `to` (0 or 1) from wherever it is now, over `ms` for a whole fade.
    pub fn go(&mut self, to: f32, now: Instant, ms: f32) {
        if to == self.to {
            return;
        }
        *self = Appear { from: self.value(now), to, start: now, ms };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn fades_in_and_out() {
        let t0 = Instant::now();
        let mut a = Appear::hidden(t0);
        assert_eq!(a.value(t0), 0.0);
        a.go(1.0, t0, 300.0);
        assert!((a.value(ms(t0, 150)) - 0.5).abs() < 0.01);
        assert_eq!(a.value(ms(t0, 300)), 1.0);
        assert!(!a.running(ms(t0, 300)));
        a.go(0.0, ms(t0, 400), 300.0);
        assert!((a.value(ms(t0, 475)) - 0.75).abs() < 0.01);
        assert_eq!(a.value(ms(t0, 700)), 0.0);
    }

    #[test]
    fn turning_round_half_way_goes_back_from_there() {
        let t0 = Instant::now();
        let mut a = Appear::hidden(t0);
        a.go(1.0, t0, 300.0);
        a.go(0.0, ms(t0, 300), 300.0);
        a.go(1.0, ms(t0, 390), 300.0); // faded out to 0.7, then back in
        assert!((a.value(ms(t0, 390)) - 0.7).abs() < 0.01);
        assert_eq!(a.value(ms(t0, 480)), 1.0); // 0.3 to go takes 90 ms
    }

    #[test]
    fn instant_without_animations() {
        let t0 = Instant::now();
        let mut a = Appear::hidden(t0);
        a.go(1.0, t0, 0.0);
        assert_eq!(a.value(t0), 1.0);
        assert!(!a.running(t0));
    }
}
