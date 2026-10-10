//! Windows' media sessions (the API behind the volume flyout's media controls), on a worker
//! thread: it keeps a [`Snapshot`] of every session and posts it to the UI whenever something
//! changes (events from the sessions, plus a check every second), and carries out [`Cmd`]s.
//! WinRT calls block (`.get()`), so none of this runs on the UI thread.

use crate::cover::{self, Cover};
use crate::timeline::{self, Reported, Skip};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use windows::core::Interface;
use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
use windows::Graphics::Imaging::*;
use windows::Media::Control::*;
use windows::Storage::Streams::IRandomAccessStream;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

/// A cover picture shared between snapshots; equal when it's the same picture.
#[derive(Clone, Debug)]
pub struct CoverRef(pub Arc<Cover>);
impl PartialEq for CoverRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Session {
    /// The app's id (AppUserModelID), e.g. "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify".
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub can_play_pause: bool,
    pub can_next: bool,
    pub can_prev: bool,
    pub can_seek: bool,
    pub timeline: Reported,
    pub cover: Option<CoverRef>,
    /// A browser's: the title of the tab playing it (sites.rs), for the site's brand.
    pub tab: Option<String>,
}

impl Session {
    pub fn progress(&self) -> timeline::Progress {
        timeline::progress(&self.timeline, self.playing, unix_now())
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Snapshot {
    pub sessions: Vec<Session>,
}

#[derive(Clone, Debug)]
pub enum Cmd {
    PlayPause(String),
    Next(String),
    Prev(String),
    /// Skip by ± seconds; whether the position the session reports can be trusted (see
    /// timeline::realtime: not right after Firefox starts playing).
    Skip(String, f64, bool),
    /// Jump to this many seconds from the start (the panel's progress bar).
    Seek(String, f64),
}

pub enum Msg {
    Changed,
    /// This session's title, artist or picture changed: look at its picture again.
    Props(String),
    Cmd(Cmd),
}

pub fn unix_now() -> f64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// Starts the worker. `post` gets every new snapshot (on the worker thread). Send it
/// `Msg::Cmd`s through the returned sender.
pub fn start(post: impl Fn(Snapshot) + Send + 'static) -> Sender<Msg> {
    let (tx, rx) = mpsc::channel();
    let events = tx.clone();
    std::thread::spawn(move || run(rx, events, post));
    tx
}

type Mgr = GlobalSystemMediaTransportControlsSessionManager;
type Sess = GlobalSystemMediaTransportControlsSession;

/// A session we listen to, with its event registrations (removed when it goes away).
struct Hooked {
    session: Sess,
    tokens: (EventRegistrationToken, EventRegistrationToken, EventRegistrationToken),
}

fn run(rx: Receiver<Msg>, events: Sender<Msg>, post: impl Fn(Snapshot)) {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let mgr = loop {
        match Mgr::RequestAsync().and_then(|op| op.get()) {
            Ok(m) => break m,
            Err(e) => {
                crate::log(&format!("media sessions unavailable: {e}"));
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    };
    let t = events.clone();
    let _ = mgr.SessionsChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(Msg::Changed);
        Ok(())
    }));
    let mut hooked: Vec<Hooked> = Vec::new();
    let mut covers: HashMap<String, CoverCache> = HashMap::new();
    let mut last: Option<Snapshot> = None;
    // sessions whose properties changed since the last look
    let mut props: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Msg::Cmd(c)) => {
                if let Err(e) = command(&mgr, &c) {
                    crate::log(&format!("{c:?} failed: {e}"));
                }
            }
            Ok(Msg::Props(id)) => {
                props.insert(id);
            }
            Ok(Msg::Changed) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // several events usually come together: take them all, then look once
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Cmd(c) => {
                    if let Err(e) = command(&mgr, &c) {
                        crate::log(&format!("{c:?} failed: {e}"));
                    }
                }
                Msg::Props(id) => {
                    props.insert(id);
                }
                Msg::Changed => {}
            }
        }
        let snap = snapshot(&mgr, &mut hooked, &mut covers, &events, &std::mem::take(&mut props));
        if last.as_ref() != Some(&snap) {
            post(snap.clone());
            last = Some(snap);
        }
    }
}

fn sessions(mgr: &Mgr) -> Vec<Sess> {
    let Ok(list) = mgr.GetSessions() else { return Vec::new() };
    let n = list.Size().unwrap_or(0);
    (0..n).filter_map(|i| list.GetAt(i).ok()).collect()
}

/// The session shown for this app (the same choice as the snapshot's: see one_per_app).
fn find(mgr: &Mgr, id: &str) -> Option<Sess> {
    one_per_app(sessions(mgr)).into_iter().find(|(i, _)| i == id).map(|(_, s)| s)
}

/// Every session, one per app: Firefox can have several (one per tab with media) under the same
/// app id, and replaces them as tabs come and go. Of several, the playing one, else the newest.
fn one_per_app(all: Vec<Sess>) -> Vec<(String, Sess)> {
    let mut out: Vec<(String, Sess, bool)> = Vec::new();
    for s in all {
        let Ok(id) = s.SourceAppUserModelId().map(|a| a.to_string()) else { continue };
        let playing = is_playing(&s).unwrap_or(false);
        match out.iter_mut().find(|(i, _, _)| *i == id) {
            Some(slot) => {
                if playing || !slot.2 {
                    *slot = (id, s, playing);
                }
            }
            None => out.push((id, s, playing)),
        }
    }
    out.into_iter().map(|(id, s, _)| (id, s)).collect()
}

fn snapshot(mgr: &Mgr, hooked: &mut Vec<Hooked>, covers: &mut HashMap<String, CoverCache>,
            events: &Sender<Msg>, props: &std::collections::HashSet<String>) -> Snapshot {
    let all = sessions(mgr);
    // listen to every session object (Windows hands back the same object for a session, so a
    // new object is a new session, e.g. Firefox replacing its tab's session); stop listening
    // to those that went away
    hooked.retain(|h| {
        let keep = all.contains(&h.session);
        if !keep {
            let _ = h.session.RemoveMediaPropertiesChanged(h.tokens.0);
            let _ = h.session.RemovePlaybackInfoChanged(h.tokens.1);
            let _ = h.session.RemoveTimelinePropertiesChanged(h.tokens.2);
        }
        keep
    });
    let mut new_sessions: Vec<Sess> = Vec::new();
    for s in &all {
        if !hooked.iter().any(|h| h.session == *s) {
            let id = s.SourceAppUserModelId().map(|a| a.to_string()).unwrap_or_default();
            if let Some(h) = hook(s, &id, events) {
                hooked.push(h);
            }
            new_sessions.push(s.clone());
        }
    }
    let mut out = Vec::new();
    for (id, s) in one_per_app(all) {
        // a new session object for an app: look at its picture again too
        let fresh = props.contains(&id) || new_sessions.contains(&s);
        match read(&s, &id, covers, fresh) {
            Ok(info) => out.push(info),
            Err(e) => crate::log(&format!("reading {id}: {e}")),
        }
    }
    covers.retain(|id, _| out.iter().any(|s| &s.id == id));
    Snapshot { sessions: out }
}

fn hook(s: &Sess, id: &str, events: &Sender<Msg>) -> Option<Hooked> {
    let (t1, t2, t3) = (events.clone(), events.clone(), events.clone());
    let props_id = id.to_string();
    let a = s.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t1.send(Msg::Props(props_id.clone()));
        Ok(())
    })).ok()?;
    let b = s.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t2.send(Msg::Changed);
        Ok(())
    })).ok()?;
    let c = s.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t3.send(Msg::Changed);
        Ok(())
    })).ok()?;
    Some(Hooked { session: s.clone(), tokens: (a, b, c) })
}

fn secs(ticks: i64) -> f64 {
    ticks as f64 / 1e7
}

fn reported(s: &Sess) -> windows::core::Result<Reported> {
    let t = s.GetTimelineProperties()?;
    Ok(Reported {
        start: secs(t.StartTime()?.Duration),
        end: secs(t.EndTime()?.Duration),
        position: secs(t.Position()?.Duration),
        // DateTime: 100 ns ticks since 1601
        updated: secs(t.LastUpdatedTime()?.UniversalTime) - 11_644_473_600.0,
    })
}

/// The cover fetched for a session's current track.
struct CoverCache {
    /// The track it's for.
    key: String,
    cover: Option<CoverRef>,
    /// A browser's: the title of the tab playing it, looked up at the same moments.
    tab: Option<String>,
    /// When the track changed, and how many times the picture has been looked at again since.
    at: Instant,
    rechecks: u8,
}

/// After a track change, look at the picture again 1.5 s and 4 s later.
const RECHECKS: [f32; 2] = [1.5, 4.0];

fn recheck_due(done: u8, since: Duration) -> bool {
    RECHECKS.get(done as usize).is_some_and(|&s| since.as_secs_f32() >= s)
}

/// `props_changed`: the session said its title, artist or picture changed since the last look.
fn read(s: &Sess, id: &str, covers: &mut HashMap<String, CoverCache>, props_changed: bool) -> windows::core::Result<Session> {
    let props = s.TryGetMediaPropertiesAsync()?.get()?;
    let info = s.GetPlaybackInfo()?;
    let c = info.Controls()?;
    let playing = info.PlaybackStatus()? == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
    // a field an app leaves out (or a read mid-change) is just empty
    let text = |r: windows::core::Result<windows::core::HSTRING>| r.map(|h| h.to_string()).unwrap_or_default();
    let (title, artist, album) = (text(props.Title()), text(props.Artist()), text(props.AlbumTitle()));
    let thumb = props.Thumbnail().ok();
    // the cover is fetched when the track (or whether it has a picture) changes, and looked at
    // again a little later (see recheck_due): Firefox reports a new tab's title before its
    // picture, so the first look can still get the previous video's
    let key = format!("{title}\u{1}{artist}\u{1}{album}\u{1}{}", thumb.is_some());
    let fetch = |thumb: Option<&windows::Storage::Streams::IRandomAccessStreamReference>| {
        thumb.and_then(|t| match load_cover(t) {
            Ok(c) => Some(c),
            Err(e) => {
                crate::log(&format!("cover for {id}: {e}"));
                None
            }
        })
    };
    // a browser: which tab (so which site) is playing it
    let find_tab = || if crate::sites::is_browser(id) { crate::sites::tab_with(&title) } else { None };
    let (cover, tab) = match covers.get_mut(id) {
        Some(c) if c.key == key => {
            let due = recheck_due(c.rechecks, c.at.elapsed());
            if due || props_changed {
                if due {
                    c.rechecks += 1;
                }
                let fresh = fetch(thumb.as_ref());
                // a different picture replaces it; the same one keeps the old (no redraw)
                if fresh.as_ref() != c.cover.as_ref().map(|r| r.0.as_ref()) {
                    c.cover = fresh.map(|p| CoverRef(Arc::new(p)));
                }
                if let Some(t) = find_tab() {
                    c.tab = Some(t);
                }
            }
            (c.cover.clone(), c.tab.clone())
        }
        _ => {
            let cover = fetch(thumb.as_ref()).map(|p| CoverRef(Arc::new(p)));
            let tab = find_tab();
            covers.insert(id.to_string(), CoverCache { key, cover: cover.clone(), tab: tab.clone(), at: Instant::now(), rechecks: 0 });
            (cover, tab)
        }
    };
    Ok(Session {
        id: id.to_string(),
        title,
        artist,
        album,
        playing,
        can_play_pause: c.IsPlayPauseToggleEnabled()? || c.IsPlayEnabled()? || c.IsPauseEnabled()?,
        can_next: c.IsNextEnabled()?,
        can_prev: c.IsPreviousEnabled()?,
        can_seek: c.IsPlaybackPositionEnabled()?,
        timeline: reported(s).unwrap_or_default(),
        cover,
        tab,
    })
}

/// Decodes the session's thumbnail into a square cover.
fn load_cover(r: &windows::Storage::Streams::IRandomAccessStreamReference) -> windows::core::Result<Cover> {
    let stream: IRandomAccessStream = r.OpenReadAsync()?.get()?.cast()?;
    decode(&stream)
}

/// Decodes a picture (PNG, JPEG, ICO, …) into a square cover (black bars trimmed).
pub fn decode(stream: &IRandomAccessStream) -> windows::core::Result<Cover> {
    decode_as(stream, true)
}

/// Decodes a logo: the centred square, nothing trimmed.
pub fn decode_logo(stream: &IRandomAccessStream) -> windows::core::Result<Cover> {
    decode_as(stream, false)
}

fn decode_as(stream: &IRandomAccessStream, trim: bool) -> windows::core::Result<Cover> {
    let dec = BitmapDecoder::CreateAsync(stream)?.get()?;
    let (w, h) = (dec.OrientedPixelWidth()?.max(1), dec.OrientedPixelHeight()?.max(1));
    // decode at most 320 px on the long side: enough for the crop and the panels
    let f = (320.0 / w.max(h) as f32).min(1.0);
    let (sw, sh) = (((w as f32 * f).round() as u32).max(1), ((h as f32 * f).round() as u32).max(1));
    let tr = BitmapTransform::new()?;
    tr.SetScaledWidth(sw)?;
    tr.SetScaledHeight(sh)?;
    tr.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
    let data = dec
        .GetPixelDataTransformedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied, &tr,
            ExifOrientationMode::RespectExifOrientation, ColorManagementMode::DoNotColorManage)?
        .get()?
        .DetachPixelData()?;
    let px: Vec<u32> = data.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    if px.len() < (sw * sh) as usize {
        return Err(windows::core::Error::new(windows::core::HRESULT(0x8000_4005u32 as i32), "short pixel data"));
    }
    Ok(if trim { cover::square(&px, sw as i32, sh as i32, cover::KEEP) } else { cover::fit(&px, sw as i32, sh as i32, cover::KEEP) })
}

fn command(mgr: &Mgr, c: &Cmd) -> windows::core::Result<()> {
    let id = match c {
        Cmd::PlayPause(id) | Cmd::Next(id) | Cmd::Prev(id) | Cmd::Skip(id, _, _) | Cmd::Seek(id, _) => id,
    };
    let Some(s) = find(mgr, id) else {
        crate::log(&format!("{c:?}: no such session"));
        return Ok(());
    };
    let t0 = Instant::now();
    let ok = match c {
        Cmd::PlayPause(_) => play_pause(&s)?,
        Cmd::Next(_) => s.TrySkipNextAsync()?.get()?,
        Cmd::Prev(_) => s.TrySkipPreviousAsync()?.get()?,
        Cmd::Skip(_, by, realtime) => skip(&s, *by, *realtime)?,
        Cmd::Seek(_, to) => {
            let r = reported(&s)?;
            let t = timeline::target(*to, r.end - r.start, 0.0);
            s.TryChangePlaybackPositionAsync(ticks(r.start + t))?.get()?
        }
    };
    if !ok {
        crate::log(&format!("{c:?}: refused by the app ({} ms)", t0.elapsed().as_millis()));
    }
    Ok(())
}

fn is_playing(s: &Sess) -> windows::core::Result<bool> {
    Ok(s.GetPlaybackInfo()?.PlaybackStatus()? == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
}

/// Waits up to `limit` for the session to be playing (or not); true if it got there.
fn wait_for(s: &Sess, playing: bool, limit: Duration) -> windows::core::Result<bool> {
    let t0 = Instant::now();
    loop {
        if is_playing(s)? == playing {
            return Ok(true);
        }
        if t0.elapsed() >= limit {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Play if paused, pause if playing: an explicit play or pause (not the app's own "toggle"),
/// then a check that it happened within 2 s.
///
/// A page that's hidden may refuse to play: Twitch in a minimised Firefox window says yes and
/// doesn't. Then the app's window is brought up (restored if minimised) and play is asked
/// again. Only for play, and only when it didn't happen; pausing never moves any window.
fn play_pause(s: &Sess) -> windows::core::Result<bool> {
    let was = is_playing(s)?;
    if was {
        s.TryPauseAsync()?.get()?;
        return wait_for(s, false, Duration::from_secs(2));
    }
    s.TryPlayAsync()?.get()?;
    if wait_for(s, true, Duration::from_secs(2))? {
        return Ok(true);
    }
    let id = s.SourceAppUserModelId()?.to_string();
    crate::apps::activate(&id);
    std::thread::sleep(Duration::from_millis(300)); // let the page notice it's visible
    s.TryPlayAsync()?.get()?;
    let worked = wait_for(s, true, Duration::from_secs(2))?;
    crate::log(&format!("play didn't take, so {} was brought up and asked again: {}", crate::apps::display_name(&id),
        if worked { "playing" } else { "still not playing" }));
    Ok(worked)
}

fn ticks(seconds: f64) -> i64 {
    (seconds * 1e7).round() as i64
}

/// ± seconds: straight to the new position when the app reports where it is; otherwise
/// (Firefox while playing) pause, which makes it report the position, seek, and play again.
fn skip(s: &Sess, by: f64, realtime: bool) -> windows::core::Result<bool> {
    let playing = s.GetPlaybackInfo()?.PlaybackStatus()? == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
    let r = reported(s)?;
    // a position that isn't live is no use for seeking straight: go through a pause
    let p = if realtime || !playing { timeline::progress(&r, playing, unix_now()) } else { timeline::Progress::Unknown };
    let ok;
    match timeline::skip(&p, by) {
        Some(Skip::To(t)) => {
            ok = s.TryChangePlaybackPositionAsync(ticks(r.start + t))?.get()?;
        }
        Some(Skip::ViaPause) => {
            if playing {
                s.TryPauseAsync()?.get()?;
            }
            // wait (briefly) until the paused session reports its position
            let t0 = Instant::now();
            let r = loop {
                let r = reported(s)?;
                if r.end - r.start > 0.5 || t0.elapsed() > Duration::from_millis(1500) {
                    break r;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            let len = r.end - r.start;
            if len > 0.5 {
                let t = timeline::target(r.position - r.start, len, by);
                ok = s.TryChangePlaybackPositionAsync(ticks(r.start + t))?.get()?;
            } else {
                crate::log(&format!("skip: no position {} ms after pausing", t0.elapsed().as_millis()));
                ok = false;
            }
            if playing {
                s.TryPlayAsync()?.get()?;
            }
        }
        None => ok = false,
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::recheck_due;
    use std::time::Duration;

    #[test]
    fn the_picture_is_looked_at_again_twice_after_a_track_change() {
        let at = |s: f32| Duration::from_secs_f32(s);
        assert!(!recheck_due(0, at(1.0)));
        assert!(recheck_due(0, at(1.6)));
        assert!(!recheck_due(1, at(2.0)));
        assert!(recheck_due(1, at(4.2)));
        assert!(!recheck_due(2, at(60.0))); // then not again until the next track
    }
}
