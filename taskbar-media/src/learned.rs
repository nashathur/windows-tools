//! Which apps report their playback position live, learned by watching them and remembered in
//! `progress.txt` next to the exe (one "app id<TAB>yes|no" per line).
//!
//! Measured on this PC: Spotify reports its position about every 4.5 s while playing. Firefox
//! reports once when playback starts (where it is at that moment), then 0 for position and
//! length until it's paused again, so a single report proves nothing.
//!
//! - "yes": two reports while playing, at least a second apart (the position is kept current).
//! - "no": seen playing with no position at all. "No" always wins: an app that ever stops
//!   reporting while playing can't be trusted to show progress.
//! - Not known yet: treated as "no" (a full ring, skips through a pause) until it proves itself.

use crate::timeline::Reported;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Default, Debug)]
pub struct Learned {
    known: HashMap<String, bool>,
    /// The last report seen from each playing app (its time), to spot a second one.
    last: HashMap<String, f64>,
}

impl Learned {
    /// Does this app report its position live?
    pub fn realtime(&self, id: &str) -> bool {
        self.known.get(id) == Some(&true)
    }

    /// Takes in what a session reports now. Returns true if what's known about it changed.
    pub fn observe(&mut self, id: &str, playing: bool, r: &Reported) -> bool {
        if !playing {
            self.last.remove(id); // reports while paused prove nothing
            return false;
        }
        let before = self.known.get(id).copied();
        if r.end - r.start <= 0.5 {
            if r.position <= 0.5 {
                self.known.insert(id.to_string(), false);
            }
        } else {
            match self.last.get(id) {
                Some(&prev) if r.updated > prev + 1.0 && before != Some(false) => {
                    self.known.insert(id.to_string(), true);
                }
                _ => {}
            }
            self.last.insert(id.to_string(), r.updated);
        }
        self.known.get(id).copied() != before
    }

    pub fn path() -> Option<PathBuf> {
        std::env::current_exe().ok().map(|e| e.with_file_name("progress.txt"))
    }

    pub fn parse(text: &str) -> Learned {
        let known = text
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(id, v)| (id.trim().to_string(), v.trim().eq_ignore_ascii_case("yes")))
            .filter(|(id, _)| !id.is_empty())
            .collect();
        Learned { known, last: HashMap::new() }
    }

    pub fn to_text(&self) -> String {
        let mut ids: Vec<&String> = self.known.keys().collect();
        ids.sort();
        ids.iter().map(|id| format!("{id}\t{}\n", if self.known[*id] { "yes" } else { "no" })).collect()
    }

    pub fn load() -> Learned {
        Learned::path().and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(Learned::default, |t| Learned::parse(&t))
    }

    pub fn save(&self) {
        if let Some(p) = Learned::path() {
            let _ = std::fs::write(p, self.to_text());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(position: f64, end: f64, updated: f64) -> Reported {
        Reported { start: 0.0, end, position, updated }
    }

    #[test]
    fn spotify_proves_itself_with_a_second_report() {
        let mut l = Learned::default();
        assert!(!l.observe("spotify", true, &rep(62.0, 237.0, 1000.0)));
        assert!(!l.realtime("spotify")); // one report: not yet
        assert!(!l.observe("spotify", true, &rep(62.0, 237.0, 1000.0))); // the same report again
        assert!(l.observe("spotify", true, &rep(66.5, 237.0, 1004.5)));
        assert!(l.realtime("spotify"));
    }

    #[test]
    fn firefox_one_report_then_zeros_is_no() {
        let mut l = Learned::default();
        l.observe("firefox", true, &rep(3050.0, 4377.0, 1000.0)); // the report at play
        assert!(!l.realtime("firefox"));
        assert!(l.observe("firefox", true, &rep(0.0, 0.0, 1003.0)));
        assert!(!l.realtime("firefox"));
        // a later burst of reports (e.g. after a seek) doesn't turn it into a yes
        l.observe("firefox", true, &rep(3100.0, 4377.0, 1010.0));
        l.observe("firefox", true, &rep(3102.0, 4377.0, 1012.0));
        assert!(!l.realtime("firefox"));
    }

    #[test]
    fn reports_while_paused_prove_nothing() {
        let mut l = Learned::default();
        l.observe("x", true, &rep(10.0, 200.0, 1000.0));
        l.observe("x", false, &rep(12.0, 200.0, 1002.0));
        l.observe("x", true, &rep(12.0, 200.0, 1002.0)); // first report of a new play
        assert!(!l.realtime("x"));
    }

    #[test]
    fn a_live_stream_is_not_a_no() {
        let mut l = Learned::default();
        assert!(!l.observe("radio", true, &rep(5400.0, 0.0, 1000.0)));
    }

    #[test]
    fn saved_and_loaded() {
        let mut l = Learned::default();
        l.observe("firefox", true, &rep(0.0, 0.0, 1.0));
        l.observe("spotify", true, &rep(1.0, 200.0, 1.0));
        l.observe("spotify", true, &rep(5.0, 200.0, 5.0));
        let back = Learned::parse(&l.to_text());
        assert!(back.realtime("spotify") && !back.realtime("firefox"));
        assert_eq!(l.to_text(), "firefox\tno\nspotify\tyes\n");
    }
}
