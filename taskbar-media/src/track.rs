//! What the strip shows for the current session: identity (source, picture, badge, tint),
//! live detection, buttons and their commands.

use super::*;

pub(crate) fn current(a: &App) -> Option<&Session> {
    let id = a.shown.as_ref()?;
    a.sessions.iter().find(|s| &s.id == id)
}

/// Does this session's app report its position live (learned.rs)? Otherwise it gets the full
/// ring, even when it does report a position (Firefox once paused, or right after play).
pub(crate) fn realtime(a: &App, s: &Session) -> bool {
    a.learned.realtime(&s.id)
}

/// Previous / next track when the app has them, else −/+ skip_seconds.
pub(crate) fn track_buttons(s: &Session) -> bool {
    s.can_next || s.can_prev
}

/// The full ring's (and the panel's full bar's) brightness, 0..1, when there's no position to
/// show: pulsing slowly while playing (from dim, so starting doesn't jump); paused, gliding
/// down to dim from wherever it was. Windows' animations off: bright playing, dim paused.
pub(crate) fn ring_level(a: &App, s: &Session) -> f32 {
    match (s.playing, a.look.animations) {
        (true, true) => anim::pulse(a.pulse_since.map_or(0.0, |t| t.elapsed().as_secs_f32())),
        (true, false) => 1.0,
        (false, true) => a.settle.map_or(0.0, |(from, at)| anim::pulse_settle(from, at.elapsed().as_secs_f32())),
        (false, false) => 0.0,
    }
}

pub(crate) fn brands_mtime() -> Option<SystemTime> {
    brands::Brands::path().and_then(|p| std::fs::metadata(p).ok()?.modified().ok())
}

/// High contrast mode is on (Windows controls all colours then: no tint).
pub(crate) fn high_contrast() -> bool {
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    unsafe {
        SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, Some(&mut hc as *mut _ as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).is_ok()
            && (hc.dwFlags & HCF_HIGHCONTRASTON).0 != 0
    }
}

/// How the shown session presents itself: its brand, picture and automatic tint colour.
pub(crate) struct Identity {
    /// "YouTube · Firefox", "Spotify".
    pub source: String,
    /// The cover; else the site's or app's logo.
    pub picture: Option<media::CoverRef>,
    /// No picture: the brand's colour and label for a badge (no brand: None, a note).
    pub badge: Option<((u32, u32, u32), String)>,
    /// The automatic tint colour, or None in high contrast.
    pub color: Option<(u32, u32, u32)>,
}

pub(crate) fn identity(a: &App, s: &Session) -> Identity {
    let site = s.tab.as_deref().and_then(|t| a.brands.site(t));
    let app = a.brands.app(&s.id);
    let app_name = app.map_or_else(|| apps::display_name(&s.id), |b| b.name.clone());
    let source = match site {
        Some(b) => format!("{} · {}", b.name, app_name),
        None => app_name,
    };
    let brand = site.or(app);
    // the app's own icon, for its logo and (for an app not in the brand file) its colour
    let app_icon = || logos::get(app.map_or("app", |b| b.logo.as_str()), &s.id);
    let tab_icon = if site.is_none() {
        s.tab.as_deref().and_then(sites::icon_host)
            .and_then(|host| logos::get(&host, &s.id))
    } else { None };
    let logo = site.and_then(|b| logos::get(&b.logo, &s.id)).or(tab_icon.clone()).or_else(app_icon);
    let picture = s.cover.clone().or_else(|| logo.map(media::CoverRef));
    let badge = brand.filter(|_| picture.is_none()).map(|b| (b.color, b.label.clone()));
    let brand_color = || site.map(|b| b.color)
        .or_else(|| tab_icon.as_ref().and_then(|i| cover::dominant(i)))
        .or_else(|| app.map(|b| b.color))
        .or_else(|| app_icon().and_then(|i| cover::dominant(&i)))
        .unwrap_or(brands::NEUTRAL);
    let color = (!high_contrast()).then(brand_color);
    Identity { source, picture, badge, color }
}

/// The strip's brand tint now: its colour, how far it is from paused (quiet) to playing (active)
/// as playback fades it. Block supplies the Highlight box Strength. None: no automatic tint
/// (high contrast).
pub(crate) fn strip_tint(a: &App) -> Option<taskbar_dock::block::Tint> {
    let s = current(a)?;
    let (r, g, b) = identity(a, s).color?;
    let active = a.tint.value(Instant::now());
    Some(taskbar_dock::block::Tint { rgb: r << 16 | g << 8 | b, active, strength: 1.0 })
}

/// The ring for the shown session: its position, or full (pulsing at `ring_level`; red live).
pub(crate) fn ring(a: &App, s: &Session) -> Ring {
    let level = ring_level(a, s);
    match s.progress() {
        _ if is_live(a, s) => Ring::Full { level, live: true },
        Progress::Live => Ring::Full { level, live: true },
        p @ Progress::At { .. } if realtime(a, s) => Ring::At(p.fraction().unwrap_or(0.0)),
        // no live position: full ring
        _ => Ring::Full { level, live: false },
    }
}

pub(crate) fn strip_content(a: &App) -> Option<StripContent> {
    let s = current(a)?;
    let id = identity(a, s);
    Some(StripContent {
        title: if s.title.is_empty() { apps::display_name(&s.id) } else { s.title.clone() },
        artist: s.artist.clone(),
        cover: id.picture,
        badge: id.badge,
        buttons: buttons(a, s),
        ring: ring(a, s),
    })
}

/// A track's identity, for remembering things about it while it plays.
pub(crate) fn track_key(s: &Session) -> String {
    format!("{}\u{1}{}\u{1}{}", s.id, s.title, s.artist)
}

/// Sites that are live streams (a tab on them plays live).
pub(crate) const LIVE_SITES: [&str; 2] = ["Twitch", "Kick"];

/// How long a track must keep looking live (paused, no length) before it counts as live:
/// Firefox briefly reports that while an ordinary video loads.
pub(crate) const LIVE_CONFIRM: Duration = Duration::from_secs(2);

/// Is the track a live stream? Remembered per track (see `update_live`).
pub(crate) fn is_live(a: &App, s: &Session) -> bool {
    a.live.contains(&track_key(s)) || on_live_site(a, s)
}

/// A tab on a live-streaming site (LIVE_SITES): live from the first moment.
pub(crate) fn on_live_site(a: &App, s: &Session) -> bool {
    let site = s.tab.as_deref().and_then(|t| a.brands.site(t));
    site.is_some_and(|b| LIVE_SITES.iter().any(|l| b.name.eq_ignore_ascii_case(l)))
}

/// Does the track look live in this report?
/// - An app that reports its position live (Spotify; see learned.rs): a position but no length.
/// - Firefox, which drops the length of every video while it plays, so that means nothing
///   there: no length while paused (an ordinary video reports its real length then, a live
///   stream 0). Only after the track has played: a video that is still loading also reports
///   paused with no length.
fn looks_live(a: &App, s: &Session) -> bool {
    if a.learned.realtime(&s.id) {
        return s.progress() == Progress::Live;
    }
    let r = &s.timeline;
    let key = track_key(s);
    !s.playing && r.end == r.start && r.end >= 0.0 && s.can_play_pause && !s.title.is_empty()
        && a.played.contains(&key) && !a.has_length.contains(&key)
}

/// Updates which tracks are live (on every media report and every second): a track becomes
/// live after looking live for LIVE_CONFIRM, and never again once it has reported a real length
/// (apps without live progress only). Tracks that are gone are forgotten.
pub(crate) fn update_live(a: &mut App, now: Instant) {
    let mut keys = std::collections::HashSet::new();
    for s in &a.sessions {
        let key = track_key(s);
        if s.playing {
            a.played.insert(key.clone());
        }
        if s.timeline.end > s.timeline.start && !a.learned.realtime(&s.id) {
            a.has_length.insert(key.clone());
            a.live.remove(&key);
        }
        if on_live_site(a, s) {
            a.live.insert(key.clone());
        } else if looks_live(a, s) {
            let since = *a.live_since.entry(key.clone()).or_insert(now);
            if now.duration_since(since) >= LIVE_CONFIRM {
                a.live.insert(key.clone());
            }
        } else {
            a.live_since.remove(&key);
        }
        keys.insert(key);
    }
    a.live.retain(|k| keys.contains(k));
    a.live_since.retain(|k, _| keys.contains(k));
    a.played.retain(|k| keys.contains(k));
    a.has_length.retain(|k| keys.contains(k));
}

/// The three buttons for the shown session (strip and click panel): glyph, and whether it
/// can be used. Previous/next when the app has them, else −/+ skip_seconds, which a live
/// stream can't do.
pub(crate) fn buttons(a: &App, s: &Session) -> [(char, bool); 3] {
    let play = (if s.playing { glyph::PAUSE } else { glyph::PLAY }, s.can_play_pause);
    if track_buttons(s) {
        [(glyph::PREVIOUS, s.can_prev), play, (glyph::NEXT, s.can_next)]
    } else {
        let seek = s.can_seek && !is_live(a, s);
        [(glyph::REWIND, seek), play, (glyph::FORWARD, seek)]
    }
}

/// What button `i` does now, or None if it can't be used (a dimmed button does nothing).
pub(crate) fn button_cmd(a: &App, i: usize) -> Option<Cmd> {
    let s = current(a)?;
    if !buttons(a, s).get(i)?.1 {
        return None;
    }
    let id = s.id.clone();
    let by = a.cfg.skip_seconds.max(1) as f64;
    let live = realtime(a, s);
    Some(match (i, track_buttons(s)) {
        (1, _) => Cmd::PlayPause(id),
        (0, true) => Cmd::Prev(id),
        (2, true) => Cmd::Next(id),
        (0, false) => Cmd::Skip(id, -by, live),
        _ => Cmd::Skip(id, by, live),
    })
}
