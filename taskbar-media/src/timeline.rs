//! Playback position: what an app reports, counted forward between its updates, and how to
//! skip ±N seconds.
//!
//! Measured on this PC: Spotify reports its position about every 4.5 s; Firefox reports 0 for
//! position and length while playing, and real values while paused or right after a seek.

/// What an app reported, in seconds, and when (seconds since the Unix epoch).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Reported {
    pub start: f64,
    pub end: f64,
    pub position: f64,
    pub updated: f64,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Progress {
    /// No usable timeline (e.g. Firefox while playing).
    Unknown,
    /// A position but no length: a live stream.
    Live,
    /// `pos` of `len` seconds.
    At { pos: f64, len: f64 },
}

impl Progress {
    /// 0..1 along, for the ring and the bar.
    pub fn fraction(&self) -> Option<f32> {
        match *self {
            Progress::At { pos, len } => Some((pos / len).clamp(0.0, 1.0) as f32),
            _ => None,
        }
    }
}

/// The position now (`now`, seconds since the epoch): the reported one, plus the time since the
/// report while playing; never past the end.
pub fn progress(r: &Reported, playing: bool, now: f64) -> Progress {
    let len = r.end - r.start;
    if len > 0.5 {
        let since = if playing { (now - r.updated).clamp(0.0, len) } else { 0.0 };
        return Progress::At { pos: (r.position - r.start + since).clamp(0.0, len), len };
    }
    // a position but no length: live. Any position counts: a Twitch stream that has just
    // started reports 0.035 s. (A normal video always has a length, even a stale one; a
    // negative length, which Firefox reports for e.g. WhatsApp, isn't "no length".)
    if r.position > 0.0 && r.end - r.start >= 0.0 {
        return Progress::Live;
    }
    Progress::Unknown
}

/// How to skip by `by` seconds.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Skip {
    /// Seek straight to this position (seconds from the start).
    To(f64),
    /// The position isn't known: pause (the app then reports it), seek, play.
    ViaPause,
}

pub fn skip(p: &Progress, by: f64) -> Option<Skip> {
    match *p {
        Progress::At { pos, len } => Some(Skip::To(target(pos, len, by))),
        Progress::Unknown => Some(Skip::ViaPause),
        Progress::Live => None,
    }
}

/// `pos + by`, kept within the track (not right at its end, which would end it).
pub fn target(pos: f64, len: f64, by: f64) -> f64 {
    (pos + by).clamp(0.0, (len - 1.0).max(0.0))
}

/// "1:05", "12:34", "1:02:03".
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(position: f64, end: f64, updated: f64) -> Reported {
        Reported { start: 0.0, end, position, updated }
    }

    #[test]
    fn counts_forward_while_playing_only() {
        let r = rep(62.0, 237.0, 1000.0);
        assert_eq!(progress(&r, true, 1003.5), Progress::At { pos: 65.5, len: 237.0 });
        assert_eq!(progress(&r, false, 1003.5), Progress::At { pos: 62.0, len: 237.0 });
        // never past the end, and a report "from the future" doesn't go backwards
        assert_eq!(progress(&r, true, 2000.0), Progress::At { pos: 237.0, len: 237.0 });
        assert_eq!(progress(&r, true, 990.0), Progress::At { pos: 62.0, len: 237.0 });
    }

    #[test]
    fn firefox_while_playing_is_unknown_and_live_has_no_length() {
        assert_eq!(progress(&rep(0.0, 0.0, 1000.0), true, 1001.0), Progress::Unknown);
        assert_eq!(progress(&rep(5400.0, 0.0, 1000.0), true, 1001.0), Progress::Live);
        assert_eq!(progress(&rep(0.035, 0.0, 1000.0), true, 1001.0), Progress::Live); // just started
        assert_eq!(progress(&rep(0.0, -922_337_203_685.0, 1000.0), true, 1001.0), Progress::Unknown); // WhatsApp
        assert_eq!(Progress::Live.fraction(), None);
        assert_eq!(Progress::At { pos: 50.0, len: 200.0 }.fraction(), Some(0.25));
    }

    #[test]
    fn skip_seeks_directly_when_the_position_is_known() {
        let p = Progress::At { pos: 100.0, len: 237.0 };
        assert_eq!(skip(&p, 15.0), Some(Skip::To(115.0)));
        assert_eq!(skip(&p, -15.0), Some(Skip::To(85.0)));
        // clamped to the start and to just before the end
        assert_eq!(skip(&Progress::At { pos: 5.0, len: 237.0 }, -15.0), Some(Skip::To(0.0)));
        assert_eq!(skip(&Progress::At { pos: 230.0, len: 237.0 }, 15.0), Some(Skip::To(236.0)));
        assert_eq!(skip(&Progress::Unknown, 15.0), Some(Skip::ViaPause));
        assert_eq!(skip(&Progress::Live, 15.0), None);
    }

    #[test]
    fn clock_format() {
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(754.0), "12:34");
        assert_eq!(clock(3723.0), "1:02:03");
    }
}
