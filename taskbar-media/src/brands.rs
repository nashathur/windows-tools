//! The brand file, `media_brands.txt` next to the exe: each app's and site's colour (for the
//! tint) and where its logo comes from (for media without a cover). Edits apply live.
//!
//! ```text
//! [apps]
//! Spotify | SpotifyAB.SpotifyMusic | #1ED760 | app         | https://developer.spotify.com/…
//! [sites]
//! YouTube | - YouTube              | #FF0000 | youtube.com | https://www.youtube.com/…  label=YT
//! ```
//! name | match (several separated by ";") | colour (#RRGGBB or "neutral") | logo | source.
//! - [apps]: `match` is part of the media session's app id; the first matching line wins.
//! - [sites]: `match` is text in the browser tab's title. Site names sit at the end of titles,
//!   so the match that appears last in the title wins ("Netflix trailer - YouTube" is
//!   YouTube); on a tie, the earlier line (so "YouTube Music" goes before "YouTube").
//! - logo: `app` = the installed app's own icon; a domain (`youtube.com`) = the site's icon,
//!   downloaded once and cached; a full https URL to a PNG or ICO; empty = none.
//! - An optional `label=XX` in the source column sets the badge text (default: from the name).

/// Grey used for "neutral" (black or white brands, whose own colour would be invisible).
pub const NEUTRAL: (u32, u32, u32) = (0x80, 0x80, 0x80);

#[derive(Clone, PartialEq, Debug)]
pub struct Brand {
    pub name: String,
    pub matches: Vec<String>,
    pub color: (u32, u32, u32),
    pub logo: String,
    pub label: String,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Brands {
    pub apps: Vec<Brand>,
    pub sites: Vec<Brand>,
}

/// The badge text for a name: its capitals when it has 2 or 3 ("YouTube" → "YT",
/// "SoundCloud" → "SC"), else its first two letters ("Spotify" → "Sp").
pub fn auto_label(name: &str) -> String {
    let word = name.split_whitespace().next().unwrap_or(name);
    let caps: String = word.chars().filter(|c| c.is_uppercase()).collect();
    if (2..=3).contains(&caps.chars().count()) && caps.chars().count() < word.chars().count() {
        return caps;
    }
    let mut it = word.chars().filter(|c| c.is_alphanumeric());
    match (it.next(), it.next()) {
        (Some(a), Some(b)) => format!("{}{}", a.to_uppercase(), b.to_lowercase()),
        (Some(a), None) => a.to_uppercase().to_string(),
        _ => "♪".into(),
    }
}

fn parse_color(s: &str) -> Option<(u32, u32, u32)> {
    if s.trim().eq_ignore_ascii_case("neutral") {
        return Some(NEUTRAL);
    }
    crate::config::parse_hex(s)
}

pub fn parse(text: &str) -> Brands {
    let mut out = Brands::default();
    let mut section = 0; // 1 = apps, 2 = sites
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.eq_ignore_ascii_case("[apps]") {
            section = 1;
            continue;
        }
        if line.eq_ignore_ascii_case("[sites]") {
            section = 2;
            continue;
        }
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        if cols.len() < 3 || section == 0 {
            continue;
        }
        let Some(color) = parse_color(cols[2]) else { continue };
        // the old four-column file (no logo column) still reads
        let (logo, source) = if cols.len() >= 5 { (cols[3], cols[4]) } else { ("", cols.get(3).copied().unwrap_or("")) };
        let label = source
            .split_whitespace()
            .find_map(|w| w.strip_prefix("label="))
            .map(String::from)
            .unwrap_or_else(|| auto_label(cols[0]));
        let brand = Brand {
            name: cols[0].to_string(),
            matches: cols[1].split(';').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect(),
            color,
            logo: logo.to_string(),
            label,
        };
        if section == 1 { out.apps.push(brand) } else { out.sites.push(brand) }
    }
    out
}

impl Brands {
    /// The brand of an app, by its media session id: the first line that matches.
    pub fn app(&self, id: &str) -> Option<&Brand> {
        let id = id.to_lowercase();
        self.apps.iter().find(|b| b.matches.iter().any(|m| id.contains(&m.to_lowercase())))
    }

    /// The brand of the site in a browser tab's title: the match that appears last in it; on a
    /// tie, the earlier line.
    pub fn site(&self, title: &str) -> Option<&Brand> {
        let title = title.to_lowercase();
        let mut best: Option<(usize, &Brand)> = None;
        for b in &self.sites {
            for m in &b.matches {
                if let Some(pos) = title.rfind(&m.to_lowercase()) {
                    if best.is_none_or(|(p, _)| pos > p) {
                        best = Some((pos, b));
                    }
                }
            }
        }
        best.map(|(_, b)| b)
    }

    pub fn path() -> Option<std::path::PathBuf> {
        std::env::current_exe().ok().map(|e| e.with_file_name("media_brands.txt"))
    }

    pub fn load() -> Brands {
        Brands::path().and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(Brands::default, |t| parse(&t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
# comment
[apps]
Spotify              | SpotifyAB.SpotifyMusic        | #1ED760 | app | https://developer.spotify.com/
Firefox              | 308046B0AF4A39CB; firefox     | #FF7139 | app | https://mozilla.design/
[sites]
YouTube Music   | - YouTube Music | #FF0000 | music.youtube.com | https://x/
YouTube         | - YouTube       | #FF0000 | youtube.com | https://x/  label=YT
Netflix         | Netflix         | #E50914 | netflix.com | https://x/
TIDAL           | TIDAL           | neutral | tidal.com | https://x/
Old format      | Old             | #123456 | https://source-only/
";

    #[test]
    fn reads_the_file() {
        let b = parse(FILE);
        assert_eq!(b.apps.len(), 2);
        assert_eq!(b.sites.len(), 5);
        assert_eq!(b.apps[0].color, (0x1E, 0xD7, 0x60));
        assert_eq!(b.apps[1].matches, vec!["308046B0AF4A39CB", "firefox"]);
        assert_eq!(b.sites[1].label, "YT");
        assert_eq!(b.sites[3].color, NEUTRAL);
        assert_eq!(b.sites[4].logo, ""); // the old four-column format
    }

    #[test]
    fn apps_first_match_wins() {
        let b = parse(FILE);
        assert_eq!(b.app("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify").unwrap().name, "Spotify");
        assert_eq!(b.app("308046B0AF4A39CB").unwrap().name, "Firefox");
        assert!(b.app("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic").is_none());
    }

    #[test]
    fn sites_last_match_in_the_title_wins() {
        let b = parse(FILE);
        assert_eq!(b.site("Netflix trailer - YouTube").unwrap().name, "YouTube");
        assert_eq!(b.site("Song - YouTube Music").unwrap().name, "YouTube Music"); // tie: earlier line
        assert_eq!(b.site("#HiddenBeat: Dipha Barus (PonYourTone) - YouTube").unwrap().name, "YouTube");
        assert!(b.site("Some page - Example").is_none());
    }

    #[test]
    fn the_real_file_reads() {
        let b = parse(include_str!("../media_brands.txt"));
        assert_eq!((b.apps.len(), b.sites.len()), (9, 28));
        assert_eq!(b.app("308046B0AF4A39CB").unwrap().logo, "app");
        let yt = b.site("#HiddenBeat: Dipha Barus feat. Hindia (PonYourTone: SIDE POP) - YouTube").unwrap();
        assert_eq!((yt.name.as_str(), yt.logo.as_str(), yt.color), ("YouTube", "youtube.com", (0xFF, 0, 0)));
        assert_eq!(b.site("subroza - Twitch").unwrap().name, "Twitch");
        assert_eq!(b.site("哔哩哔哩 (゜-゜)つロ").unwrap().name, "Bilibili");
    }

    #[test]
    fn badge_labels() {
        assert_eq!(auto_label("YouTube"), "YT");
        assert_eq!(auto_label("SoundCloud"), "SC");
        assert_eq!(auto_label("Spotify"), "Sp");
        assert_eq!(auto_label("Spotify Web"), "Sp");
        assert_eq!(auto_label("TIDAL"), "Ti");
        assert_eq!(auto_label("Disney+"), "Di");
    }
}
