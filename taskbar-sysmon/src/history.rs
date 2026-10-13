use std::time::{Duration, Instant};
#[derive(Clone, Debug)]
pub struct Point {
    pub at: Instant,
    pub interval: Duration,
    pub values: [Option<f64>; 5],
}
#[derive(Default, Debug)]
pub struct History {
    pub points: Vec<Point>,
}
impl History {
    pub fn push(&mut self, p: Point) {
        let cut = p.at.checked_sub(Duration::from_secs(3600));
        self.points.push(p);
        // oldest first: drop only from the front (no scan of the whole hour per sample)
        if let Some(cut) = cut {
            let old = self.points.iter().take_while(|x| x.at < cut).count();
            self.points.drain(..old);
        }
    }
    pub fn clear(&mut self) {
        self.points.clear();
    }
    /// The last `seconds` of `channel` as `width` pixel columns: the max of the samples in each
    /// column, columns between two samples joined linearly, None before the first sample and
    /// across a gap (samples further apart than 2.5 × their interval: idle or paused).
    pub fn view(&self, seconds: u64, width: usize, channel: usize) -> Vec<Option<f64>> {
        if width == 0 {
            return Vec::new();
        }
        let Some(end) = self.points.last().map(|p| p.at) else {
            return vec![None; width];
        };
        let start = end.checked_sub(Duration::from_secs(seconds)).unwrap_or(end);
        let column = |at: Instant| {
            ((at.saturating_duration_since(start).as_secs_f64() / seconds.max(1) as f64)
                * width.saturating_sub(1) as f64)
                .round() as usize
        };
        let mut out: Vec<Option<f64>> = vec![None; width];
        let mut broken = vec![false; width]; // columns inside a gap
        let mut prev: Option<&Point> = None;
        for p in self.points.iter().filter(|p| p.at >= start) {
            let x = column(p.at).min(width - 1);
            if let Some(q) = prev {
                if p.at.duration_since(q.at) > q.interval.mul_f64(2.5) {
                    for b in &mut broken[(column(q.at) + 1).min(x)..x] {
                        *b = true;
                    }
                }
            }
            if let Some(v) = p.values[channel] {
                out[x] = Some(out[x].map_or(v, |o| o.max(v)));
            }
            prev = Some(p);
        }
        // join neighbouring samples across the empty columns between them
        let mut last: Option<usize> = None;
        for i in 0..width {
            let Some(v) = out[i] else { continue };
            if let Some(a) = last.filter(|&a| i > a + 1 && !broken[a + 1..i].iter().any(|&b| b)) {
                let from = out[a].unwrap_or(v);
                for j in a + 1..i {
                    out[j] = Some(from + (v - from) * (j - a) as f64 / (i - a) as f64);
                }
            }
            last = Some(i);
        }
        out
    }
    pub fn pixel_signature(&self, seconds: u64, channel: usize, out: &mut [i16]) {
        out.fill(i16::MIN);
        if out.is_empty() {
            return;
        }
        let width = out.len();
        let Some(end) = self.points.last().map(|p| p.at) else {
            return;
        };
        let start = end - Duration::from_secs(seconds);
        for (i, p) in self
            .points
            .iter()
            .enumerate()
            .filter(|(_, p)| p.at >= start)
        {
            let x = (((p.at - start).as_secs_f64() / seconds as f64)
                * (width.saturating_sub(1)) as f64)
                .round() as usize;
            if i > 0 {
                let prev = &self.points[i - 1];
                if p.at.duration_since(prev.at) > prev.interval.mul_f64(2.5) {
                    let a = (((prev.at - start).as_secs_f64() / seconds as f64)
                        * (width.saturating_sub(1)) as f64)
                        .round() as usize;
                    for v in &mut out[a.saturating_add(1).min(width - 1)..=x.min(width - 1)] {
                        *v = i16::MIN;
                    }
                }
            }
            if let Some(v) = p.values[channel] {
                let px = (v.clamp(0., 100.) * 12. / 100.).round() as i16;
                let slot = &mut out[x.min(width - 1)];
                *slot = if *slot == i16::MIN {
                    px
                } else {
                    (*slot).max(px)
                };
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gaps_break_and_downsample_keeps_max() {
        let now = Instant::now();
        let mut h = History::default();
        for (i, v) in [(0, Some(2.)), (1, Some(8.)), (10, Some(4.))] {
            h.push(Point {
                at: now + Duration::from_secs(i),
                interval: Duration::from_secs(1),
                values: [v, None, None, None, None],
            });
        }
        let x = h.view(10, 3, 0);
        assert_eq!(x[0], Some(8.));
        assert!(x[1].is_none());
        assert_eq!(x[2], Some(4.));
    }

    #[test]
    fn samples_fewer_than_pixels_draw_a_joined_line() {
        let now = Instant::now();
        let mut h = History::default();
        for i in 0..=10u64 {
            h.push(Point {
                at: now + Duration::from_secs(i),
                interval: Duration::from_secs(1),
                values: [Some(i as f64 * 10.), None, None, None, None],
            });
        }
        let x = h.view(10, 101, 0);
        assert!(x.iter().all(Option::is_some), "no stripes: every column has a value");
        assert_eq!(x[0], Some(0.));
        assert_eq!(x[100], Some(100.));
        assert!((x[55].unwrap() - 55.).abs() < 1e-9, "joined linearly");
    }
}
