//! Which media session the widget shows: always the one that's playing.
//!
//! - A playing session wins; if several are playing, the one that started playing most recently.
//! - Nothing playing: the last one shown stays (paused) for `hide_after`, then the strip hides.
//!   If that session goes away (app or tab closed), the strip hides at once.
//! - Before anything has played, nothing is shown.

use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Default, Debug)]
pub struct Selector {
    /// When each playing session started playing.
    playing_since: HashMap<String, Instant>,
    /// The session shown (or last shown, while hidden after a pause).
    shown: Option<String>,
    /// Since when nothing has been playing.
    idle_since: Option<Instant>,
}

impl Selector {
    /// `sessions`: every session now, as (app id, playing). Returns the one to show, or None
    /// to hide the strip.
    pub fn update(&mut self, now: Instant, sessions: &[(&str, bool)], hide_after: Duration) -> Option<String> {
        self.playing_since.retain(|id, _| sessions.iter().any(|(s, playing)| s == id && *playing));
        for (id, playing) in sessions {
            if *playing {
                self.playing_since.entry(id.to_string()).or_insert(now);
            }
        }
        // the most recent start wins; on a tie the earlier one in the list
        let mut best: Option<(&str, Instant)> = None;
        for (id, playing) in sessions {
            if !*playing {
                continue;
            }
            let since = self.playing_since[*id];
            if best.is_none_or(|(_, b)| since > b) {
                best = Some((id, since));
            }
        }
        if let Some((id, _)) = best {
            self.shown = Some(id.to_string());
            self.idle_since = None;
            return self.shown.clone();
        }
        let shown = self.shown.clone()?;
        if !sessions.iter().any(|(s, _)| *s == shown) {
            self.shown = None;
            self.idle_since = None;
            return None;
        }
        let idle = *self.idle_since.get_or_insert(now);
        (now.duration_since(idle) < hide_after).then_some(shown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIDE: Duration = Duration::from_secs(120);

    fn at(t0: Instant, s: u64) -> Instant {
        t0 + Duration::from_secs(s)
    }

    #[test]
    fn nothing_shown_before_anything_plays() {
        let mut s = Selector::default();
        assert_eq!(s.update(Instant::now(), &[("spotify", false)], HIDE), None);
    }

    #[test]
    fn the_playing_session_is_shown() {
        let (mut s, t0) = (Selector::default(), Instant::now());
        assert_eq!(s.update(t0, &[("firefox", false), ("spotify", true)], HIDE).as_deref(), Some("spotify"));
    }

    #[test]
    fn the_most_recently_started_wins() {
        let (mut s, t0) = (Selector::default(), Instant::now());
        assert_eq!(s.update(t0, &[("spotify", true), ("firefox", false)], HIDE).as_deref(), Some("spotify"));
        // Firefox starts later while Spotify keeps playing: Firefox
        assert_eq!(s.update(at(t0, 5), &[("spotify", true), ("firefox", true)], HIDE).as_deref(), Some("firefox"));
        // Firefox pauses: back to Spotify, which is still playing
        assert_eq!(s.update(at(t0, 9), &[("spotify", true), ("firefox", false)], HIDE).as_deref(), Some("spotify"));
    }

    #[test]
    fn a_paused_session_stays_then_hides() {
        let (mut s, t0) = (Selector::default(), Instant::now());
        s.update(t0, &[("spotify", true)], HIDE);
        assert_eq!(s.update(at(t0, 10), &[("spotify", false)], HIDE).as_deref(), Some("spotify"));
        assert_eq!(s.update(at(t0, 129), &[("spotify", false)], HIDE).as_deref(), Some("spotify"));
        assert_eq!(s.update(at(t0, 130), &[("spotify", false)], HIDE), None);
        // playing again: shown again
        assert_eq!(s.update(at(t0, 200), &[("spotify", true)], HIDE).as_deref(), Some("spotify"));
    }

    #[test]
    fn a_closed_session_hides_at_once() {
        let (mut s, t0) = (Selector::default(), Instant::now());
        s.update(t0, &[("firefox", true)], HIDE);
        assert_eq!(s.update(at(t0, 1), &[], HIDE), None);
        // and doesn't come back when another one is merely paused
        assert_eq!(s.update(at(t0, 2), &[("spotify", false)], HIDE), None);
    }

    #[test]
    fn pausing_restarts_the_hide_timer_each_time() {
        let (mut s, t0) = (Selector::default(), Instant::now());
        s.update(t0, &[("spotify", true)], HIDE);
        s.update(at(t0, 100), &[("spotify", false)], HIDE);
        s.update(at(t0, 150), &[("spotify", true)], HIDE);
        // paused again at 160: shown until 280
        assert_eq!(s.update(at(t0, 160), &[("spotify", false)], HIDE).as_deref(), Some("spotify"));
        assert_eq!(s.update(at(t0, 279), &[("spotify", false)], HIDE).as_deref(), Some("spotify"));
        assert_eq!(s.update(at(t0, 280), &[("spotify", false)], HIDE), None);
    }
}
