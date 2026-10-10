#![windows_subsystem = "windows"]

//! Now playing on the Windows 11 taskbar, docked next to the other taskbar widgets (bus,
//! weather) through the shared taskbar-dock library. See PLAN.md.

mod appear;
mod audio;
mod brands;
mod apps;
mod config;
mod cover;
mod iconurl;
mod layout;
mod learned;
mod logos;
mod media;
mod panel_view;
mod popup;
mod render;
mod select;
mod sites;
mod timeline;
mod track;
mod placement;

use placement::*;
use track::*;

use config::Config;
use layout::{Part, Strip};
use media::{Cmd, Msg, Session, Snapshot};
use render::{glyph, wide, Ring, StripContent};
use appear::Appear;
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime};
use taskbar_dock::panel::{Fade, PanelBehaviour};
use taskbar_dock::strip::Release;
use taskbar_dock::style::Look;
use taskbar_dock::{anim, dock, settings, system};
use timeline::Progress;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;

use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_MEDIA: u32 = WM_APP + 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_WATCH: usize = 1;
const TIMER_GLIDE: usize = 2;
const TIMER_ZOOM: usize = 3;
/// The ring pulsing (only while a full ring is shown for something playing).
const TIMER_BEAT: usize = 4;
/// The strip fading in or out.
const TIMER_FADE: usize = 5;
/// The strip's width animating after compact mode was switched (popup timers are 6..8).
const TIMER_RESIZE: usize = 9;
/// The cover growing on hover / dipping on press (anim::Feel), only while it moves.
const TIMER_FEEL: usize = 10;

const MAIN_CLASS: PCWSTR = w!("TaskbarMediaMain");
const STRIP_CLASS: PCWSTR = w!("TaskbarMediaStrip");
/// The strip's window class: its name in the dock order.
const MY_CLASS: &str = "TaskbarMediaStrip";
/// No taskbar for this long: float instead (an Explorer restart brings it back sooner).
const FLOAT_AFTER: Duration = Duration::from_secs(10);
/// Floating because attaching failed: try attaching again this often.
const ATTACH_RETRY: Duration = Duration::from_secs(60);

struct App {
    main: HWND,
    strip: Option<HWND>,
    /// The strip is a floating topmost window above the taskbar instead of a child of it
    /// (the fallback; see `create_strip`).
    floating: bool,
    /// Started with `--float`: always floating (for testing the fallback).
    force_float: bool,
    /// Attaching to the taskbar failed at this time (retried after `ATTACH_RETRY`).
    attach_failed: Option<Instant>,
    /// Since when no taskbar has been found (floating after `FLOAT_AFTER`).
    no_taskbar_since: Option<Instant>,
    /// Floating and hidden because a full-screen app is in front.
    fs_hidden: bool,
    cfg: Config,
    cfg_mtime: Option<SystemTime>,
    /// Shared style + Windows settings, kept current.
    look: Look,
    media: Sender<Msg>,
    sessions: Vec<Session>,
    selector: select::Selector,
    /// The session shown, None = strip hidden.
    shown: Option<String>,
    /// The mouse is over the strip: which part.
    hover: Option<Part>,
    /// Last layout drawn (size, hit testing).
    layout: Option<Strip>,
    /// Last layout drawn at normal size (not zoomed by a drag): the widths are worked out from it.
    rest_layout: Option<Strip>,
    /// What was drawn last, to skip identical redraws.
    drawn: Option<String>,
    /// The width the strip was last headed for (its mode's, or narrowed for room): a change
    /// animates the width.
    drawn_target: Option<i32>,
    /// Room next to the taskbar's buttons (dock::fit): full, narrower, or hidden.
    fit: dock::FitState,
    /// The strip's widths for the fit, as last published.
    size: Option<dock::Size>,
    taskbar_created: u32,
    hooks: dock::Hooks,
    /// The strip gliding, dragged and zoomed (the shared behaviour, `taskbar_dock::strip`).
    motion: taskbar_dock::strip::Motion,
    /// The part of the strip the mouse was pressed on (a click acts on it).
    pressed: Option<Part>,
    /// The cover's hover / press size factor.
    feel: anim::Feel,
    feeling: bool,
    /// The ring pulse: since when it's been pulsing (playing), or where it was when playback
    /// paused and when, as it glides down (anim::pulse_settle).
    pulse_since: Option<Instant>,
    settle: Option<(f32, Instant)>,
    /// The shown session was playing at the last redraw.
    was_playing: bool,
    /// The pulse timer is running.
    beating: bool,
    /// The strip's opacity: fading in when it appears, out before it hides.
    appear: Appear,
    /// What was drawn last: still shown while fading out after the session went away.
    last_content: Option<StripContent>,
    /// Which apps report their position live (learned, kept in progress.txt).
    learned: learned::Learned,
    /// Tracks seen to be live streams (track_key), while they play.
    live: std::collections::HashSet<String>,
    /// When each track started looking live (confirmed after LIVE_CONFIRM).
    live_since: std::collections::HashMap<String, Instant>,
    /// Tracks seen playing (a paused track that never played is still loading, not live).
    played: std::collections::HashSet<String>,
    /// Tracks seen reporting a real length: never live (rapid pause/play makes Firefox send a
    /// stale paused report with no length that can last past LIVE_CONFIRM).
    has_length: std::collections::HashSet<String>,
    /// The brand file (media_brands.txt), reloaded when it changes.
    brands: brands::Brands,
    brands_mtime: Option<SystemTime>,
    /// The strip's tint: in while playing, out when paused.
    tint: Appear,
    /// When the panel opens and closes (the shared behaviour for every widget), its fade-in,
    /// and the panel itself.
    panel: PanelBehaviour,
    pfade: Fade,
    popup: popup::Panel,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Short, non-reentrant access to the app state. Never call into Win32 functions that send
/// messages to our own windows (CreateWindow, DestroyWindow, SetWindowPos, ...) inside `f`.
fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

/// Appends a line to taskbar-media.log next to the exe (kept small).
pub fn log(line: &str) {
    let Ok(exe) = std::env::current_exe() else { return };
    let path = exe.with_file_name("taskbar-media.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let _ = writeln!(f, "{now} {line}");
    }
}

fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).unwrap_or_default().into() }
}

fn raw(h: HWND) -> isize {
    h.0 as isize
}

fn taskbar() -> Option<HWND> {
    unsafe { FindWindowW(w!("Shell_TrayWnd"), None).ok().filter(|h| !h.is_invalid()) }
}

fn look() -> Look {
    with(|a| a.look.clone()).unwrap_or_else(Look::current)
}

/// Re-reads the shared style and the Windows settings; true if anything changed.
fn refresh_look() -> bool {
    let new = Look::current();
    with(|a| {
        let changed = a.look != new;
        a.look = new;
        changed
    }) == Some(true)
}

fn scale_of(tb: HWND) -> f32 {
    system::scale_for(raw(tb))
}

fn gap_px(scale: f32) -> i32 {
    (look().gap * scale).round() as i32
}

// ---------------------------------------------------------------- start with Windows

fn startup_enabled() -> bool {
    taskbar_dock::startup::is_on("media")
}

/// Puts the widget on the shared startup list (or takes it off) to match the setting.
fn apply_startup(on: bool) {
    if on != startup_enabled() {
        let _ = taskbar_dock::startup::set("media", on);
    }
}

// ---------------------------------------------------------------- media

fn on_media(snap: Box<Snapshot>) {
    let changed = with(|a| {
        a.sessions = snap.sessions;
        // learn which apps report their position live (see learned.rs)
        let mut changed = false;
        for s in &a.sessions {
            changed |= a.learned.observe(&s.id, s.playing, &s.timeline);
        }
        if changed {
            a.learned.save();
            a.drawn = None;
        }
        // remember live streams per track (see update_live)
        update_live(a, Instant::now());
        changed
    });
    if changed == Some(true) {
        log("learned which apps report progress live: see progress.txt");
    }
    reselect();
}

/// Fades the strip to opacity `to` (1 = in, 0 = out); 0 hides it at the end (fade_step).
fn fade_to(to: f32) {
    let ms = with(|a| if a.look.animations { a.cfg.fade_ms.max(0.0) } else { 0.0 }).unwrap_or(0.0);
    let Some((main, running)) = with(|a| {
        let now = Instant::now();
        a.appear.go(to, now, ms);
        (a.main, a.appear.running(now))
    }) else { return };
    if running {
        unsafe { SetTimer(main, TIMER_FADE, 16, None); }
    }
    fade_step();
}

/// One frame of a fade; at the end of a fade-out the strip hides and the others close up.
fn fade_step() {
    let Some((main, running, target, strip)) = with(|a| {
        let now = Instant::now();
        (a.main, a.appear.running(now), a.appear.target(), a.strip)
    }) else { return };
    if !running {
        unsafe { let _ = KillTimer(main, TIMER_FADE); }
    }
    match (running, target == 0.0, strip) {
        (false, true, Some(h)) => unsafe {
            if IsWindowVisible(h).as_bool() {
                let _ = ShowWindow(h, SW_HIDE);
                with(|a| {
                    a.drawn = None;
                    a.last_content = None;
                });
                dock::notify(raw(h)); // the others close up
            }
        },
        _ => redraw_strip(),
    }
}

/// Picks the session to show (the one playing), and shows or hides the strip.
fn reselect() {
    let Some((shown, changed)) = with(|a| {
        let list: Vec<(&str, bool)> = a.sessions.iter().map(|s| (s.id.as_str(), s.playing)).collect();
        let hide = Duration::from_secs(a.cfg.hide_after_pause_s as u64);
        let shown = a.selector.update(Instant::now(), &list, hide);
        let changed = shown.is_some() != a.shown.is_some();
        a.shown = shown.clone();
        (shown, changed)
    }) else { return };
    let strip = with(|a| a.strip).flatten();
    match (shown, strip) {
        (None, Some(_)) if changed => fade_to(0.0),
        (Some(_), Some(h)) => {
            // playing again while fading out: fade back in from where it is
            let fading_out = with(|a| a.appear.target() == 0.0).unwrap_or(false);
            if fading_out && unsafe { IsWindowVisible(h).as_bool() } {
                fade_to(1.0);
            } else {
                redraw_strip();
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- the strip

/// Creates the strip: a child of the taskbar if possible, else (attaching failed, or no taskbar
/// for `FLOAT_AFTER`, or `--float`) a floating topmost window above the taskbar.
fn create_strip() {
    let Some((has, force, failed)) = with(|a| (a.strip.is_some(), a.force_float, a.attach_failed)) else { return };
    if has {
        return;
    }
    let tb = taskbar();
    if let Some(tb) = tb.filter(|_| !force && failed.is_none_or(|t| t.elapsed() >= ATTACH_RETRY)) {
        if let Some(hwnd) = attach(tb) {
            adopt(hwnd, false);
            return;
        }
    }
    // right after an Explorer restart the taskbar is missing for a moment: wait for it
    if tb.is_none() && !force && failed.is_none() {
        let since = with(|a| *a.no_taskbar_since.get_or_insert_with(Instant::now));
        if since.is_none_or(|t| t.elapsed() < FLOAT_AFTER) {
            return;
        }
    }
    let hwnd = unsafe {
        CreateWindowExW(WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW, STRIP_CLASS,
            w!("Now playing"), WS_POPUP, 0, 0, 1, 1, None, None, hinstance(), None)
    };
    match hwnd {
        Ok(hwnd) => adopt(hwnd, true),
        Err(e) => log(&format!("creating the floating strip failed: {e}")),
    }
}

/// The strip as a child of the taskbar, or None (logged; floating is tried next).
fn attach(tb: HWND) -> Option<HWND> {
    let hwnd = unsafe {
        CreateWindowExW(WS_EX_LAYERED | WS_EX_NOACTIVATE, STRIP_CLASS, w!("Now playing"),
            WS_CHILD | WS_CLIPSIBLINGS, 0, 0, 1, 1, tb, None, hinstance(), None)
    };
    match hwnd {
        Ok(h) => Some(h),
        Err(e) => {
            log(&format!("attaching the strip to the taskbar failed: {e}"));
            with(|a| a.attach_failed = Some(Instant::now()));
            None
        }
    }
}

fn adopt(hwnd: HWND, floating: bool) {
    let was = with(|a| {
        let was = a.floating;
        a.strip = Some(hwnd);
        a.floating = floating;
        a.fs_hidden = false;
        a.drawn = None;
        // a new strip starts full and shown
        a.fit = dock::FitState::default();
        a.drawn_target = None;
        if !floating {
            a.attach_failed = None;
        }
        was
    });
    if floating {
        log("strip floating above the taskbar");
    } else {
        if was == Some(true) {
            log("strip attached to the taskbar again");
        }
        unsafe { dock::mark_member(raw(hwnd)); }
    }
    redraw_strip();
}

/// Floating: where the strip goes on screen, left of the tray and of the widgets docked there
/// when there's a taskbar, else in the bottom right corner of the primary screen.
fn float_pos(width: i32, height: i32, scale: f32) -> (i32, i32) {
    unsafe {
        match taskbar() {
            Some(tb) => {
                let t = dock::rect(raw(tb));
                let left = dock::children(raw(tb)).iter().filter(|c| c.member && !c.squeezed).map(|c| c.rc.left)
                    .fold(dock::tray_boundary(raw(tb), scale), i32::min);
                let x = left - width - gap_px(scale);
                (x, t.top + (t.bottom - t.top - height) / 2)
            }
            None => {
                let m = (8.0 * scale).round() as i32;
                (GetSystemMetrics(SM_CXSCREEN) - width - m, GetSystemMetrics(SM_CYSCREEN) - height - m)
            }
        }
    }
}

/// Floating: is a full-screen app in front? Then the strip hides (the taskbar does that for
/// the attached strip).
fn fullscreen_app_active() -> bool {
    unsafe {
        let h = GetForegroundWindow();
        if h.is_invalid() {
            return false;
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(h, &mut cls).max(0) as usize;
        let cls = String::from_utf16_lossy(&cls[..n]);
        if ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"].contains(&cls.as_str()) {
            return false;
        }
        let r = dock::rect(raw(h));
        r.left <= 0 && r.top <= 0 && r.right >= GetSystemMetrics(SM_CXSCREEN) && r.bottom >= GetSystemMetrics(SM_CYSCREEN)
    }
}

fn destroy_strip() {
    if let Some(Some(h)) = with(|a| a.strip.take()) {
        unsafe { let _ = DestroyWindow(h); }
    }
}

/// Runs the pulse timer exactly while the shown ring pulses (25 frames a second).
fn set_beating(on: bool) {
    let Some((main, was)) = with(|a| (a.main, std::mem::replace(&mut a.beating, on))) else { return };
    if on != was {
        unsafe {
            if on {
                SetTimer(main, TIMER_BEAT, 40, None);
            } else {
                let _ = KillTimer(main, TIMER_BEAT);
            }
        }
    }
}

/// The mouse state changed: the cover grows while hovered, dips while pressed (anim::Feel), and
/// the frame timer runs while it moves. Only while the strip is at rest: dragged from the cover,
/// it eases back to normal and leaves the size to the drag zoom. Cheap when nothing changed.
pub(crate) fn feel_update() {
    let Some((main, changed, run, was)) = with(|a| {
        let on = a.hover == Some(Part::Cover) && a.motion.calm();
        let changed = a.feel.set(on, on && a.pressed == Some(Part::Cover));
        if !a.look.animations {
            a.feel.snap();
        }
        (a.main, changed, a.feel.animating(), std::mem::replace(&mut a.feeling, a.feel.animating()))
    }) else { return };
    if run && !was {
        unsafe { SetTimer(main, TIMER_FEEL, taskbar_dock::anim::FEEL_FRAME_MS, None); }
    }
    if changed {
        redraw_strip();
    }
}

fn feel_step() {
    let Some((main, more)) = with(|a| (a.main, a.feel.tick(Instant::now()))) else { return };
    if !more {
        with(|a| a.feeling = false);
        unsafe { let _ = KillTimer(main, TIMER_FEEL); }
    }
    redraw_strip();
}

fn redraw_strip() {
    // floating behind a full-screen app: stays hidden (watch shows it again)
    if with(|a| a.fs_hidden) == Some(true) {
        set_beating(false);
        return;
    }
    let Some(Some((hwnd, content, hover, zoom, cfg, visible, playing, animations, opacity, grow))) = with(|a| {
        let playing = current(a).is_some_and(|s| s.playing);
        // the pulse starts (from dim) when playback starts; on pause, remember where it was
        // so it can glide down from there
        if playing != a.was_playing {
            let now = Instant::now();
            if playing {
                a.pulse_since = Some(now);
                a.settle = None;
            } else {
                let level = a.pulse_since.map_or(0.0, |t| anim::pulse(t.elapsed().as_secs_f32()));
                a.settle = Some((level, now));
                a.pulse_since = None;
            }
            // the tint fades in with playback and out with a pause
            let ms = if a.look.animations { a.cfg.fade_ms.max(0.0) } else { 0.0 };
            a.tint.go(if playing { 1.0 } else { 0.0 }, now, ms);
            a.was_playing = playing;
        }
        let tint_fading = a.tint.running(Instant::now());
        let settling = a.settle.is_some_and(|(from, at)| anim::pulse_settle(from, at.elapsed().as_secs_f32()) > 0.0);
        // fading out after the session went away: keep showing what was there
        let content = strip_content(a).or_else(|| a.last_content.clone().filter(|_| a.appear.target() == 0.0));
        let opacity = a.appear.value(Instant::now());
        a.strip.map(|h| (h, content, a.hover, a.motion.zoom(),a.cfg.clone(), a.shown.is_some() && !a.fit.hidden(), playing || settling || tint_fading, a.look.animations, opacity, a.feel.value()))
    }) else { return };
    // the pulse timer runs while the full ring pulses or glides down after a pause
    let beats = visible && playing && animations && matches!(content.as_ref().map(|c| c.ring), Some(Ring::Full { .. }));
    set_beating(beats);
    let fading_out = with(|a| a.appear.target() == 0.0 && a.appear.running(Instant::now())).unwrap_or(false);
    let Some(content) = content.filter(|_| visible || fading_out) else { return };
    with(|a| a.last_content = Some(content.clone()));
    let lk = look();
    unsafe {
        // attached: the taskbar is the parent; floating: as tall as the taskbar, if there is one
        let parent = if with(|a| a.floating) == Some(true) {
            None
        } else {
            let Ok(tb) = GetParent(hwnd) else { return };
            Some(tb)
        };
        let (height, scale) = match parent {
            Some(tb) => {
                let mut rc = RECT::default();
                let _ = GetClientRect(tb, &mut rc);
                (rc.bottom - rc.top, scale_of(tb))
            }
            None => {
                let scale = system::scale_for(raw(hwnd));
                let h = taskbar().map_or((48.0 * scale).round() as i32, |t| {
                    let r = dock::rect(raw(t));
                    r.bottom - r.top
                });
                (h, scale)
            }
        };
        if height <= 0 {
            return;
        }
        // the ring moves in steps of half a pixel along its ~4 × 32 px length: skip redraws
        // that wouldn't change a pixel
        let ring_step = match content.ring {
            Ring::At(f) => (f * 4.0 * 32.0 * scale * 2.0).round() as i32,
            // pulsing: redraw when the brightness visibly changes (64 steps)
            Ring::Full { level, live } => -1 - (level * 64.0).round() as i32 - if live { 1000 } else { 0 },
        };
        // compact mode switched, or narrowed for room: the width animates to the new one
        let resize_w = target_width(hwnd, &cfg, scale, &lk, parent.is_some() && IsWindowVisible(hwnd).as_bool());
        let key = format!("{:?}|{:?}|{}|{}|{:?}|{}|{:?}|{}", content.title, content.artist,
            content.cover.as_ref().map_or(0, |c| std::sync::Arc::as_ptr(&c.0) as usize),
            ring_step, content.buttons, height, hover, zoom);
        let key = format!("{key}|{:?}|{:?}", content.badge, content.tint.map(|(c, a)| (c, (a * 255.0).round() as i32)));
        let key = format!("{key}|{scale}|{:?}|{:?}|{}|{}", lk, cfg, IsWindowVisible(hwnd).as_bool(), (opacity * 255.0).round());
        let key = format!("{key}|{resize_w:?}|{}", (grow * 400.0).round());
        if with(|a| a.drawn.as_deref() == Some(key.as_str())) == Some(true) {
            return;
        }
        let was_visible = IsWindowVisible(hwnd).as_bool();
        // appearing: start fully transparent, then fade in (below)
        let opacity = if was_visible { opacity } else { 0.0 };
        let before = dock::rect(raw(hwnd));
        // a resize running while picked up grows with the zoom like everything else
        let draw_w = resize_w.map(|w| (w as f32 * zoom).round() as i32);
        let l = render::update_strip(hwnd, &content, height, scale * zoom, &lk, hover, cfg.compact && resize_w.is_none(), cfg.max_width, draw_w, cfg.live_rgb(), opacity, parent.is_none(), grow);
        with(|a| {
            a.layout = Some(l);
            if zoom == 1.0 {
                a.rest_layout = Some(l);
            }
            a.drawn = Some(key);
        });
        let Some(tb) = parent else {
            let (x, y) = float_pos(l.width, height, scale);
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, l.width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            if !was_visible {
                with(|a| {
                    a.appear = Appear::hidden(Instant::now());
                    a.drawn = None;
                });
                fade_to(1.0);
            }
            return;
        };
        if zoom == 1.0 {
            // the other members fit the stack with these (dock::fit): the mode's width, the
            // compact one, and any width between (the text column narrows)
            let text_w = (cfg.max_width * scale).round() as i32;
            let size = dock::Size {
                full: layout::strip(scale, l.cover, l.ring_x, cfg.compact, text_w).width,
                min: layout::strip(scale, l.cover, l.ring_x, true, 0).width,
                stretch: true,
            };
            with(|a| a.size = Some(size));
            dock::publish_widths(raw(hwnd), size);
        }
        let _ = SetWindowPos(hwnd, HWND_TOP, 0, 0, l.width, height, SWP_NOMOVE | SWP_NOACTIVATE | SWP_SHOWWINDOW | SET_WINDOW_POS_FLAGS(dock::raise_flag(raw(hwnd))));
        if was_visible && with(|a| a.motion.settled()) == Some(true) {
            // the stack hangs from the tray: a new width keeps the right edge, not the left
            taskbar_dock::strip::keep_right_edge(raw(hwnd), raw(tb), before);
        }
        let (x, other_dragged) = slot(tb, hwnd, l.width, scale);
        if !was_visible {
            // appearing: jump into place (still transparent), the others make room, fade in
            jump_to(hwnd, tb, x);
            dock::notify(raw(hwnd));
            with(|a| {
                a.appear = Appear::hidden(Instant::now());
                a.drawn = None;
            });
            fade_to(1.0);
        } else {
            glide_to(hwnd, tb, x, other_dragged); // stays put while dragged
        }
    }
}

/// The width the strip heads for changed since the last redraw: compact mode switched (the
/// panel's switch or the settings file), or the room next to the taskbar's buttons narrowed or
/// widened it (dock::fit). Starts animating from the current width. Returns the width to draw
/// the full strip at: animating, or narrowed for room (None: the mode's own layout).
/// `animate`: only a shown, docked strip animates (appearing or floating, it just shows).
fn target_width(hwnd: HWND, cfg: &Config, scale: f32, lk: &Look, animate: bool) -> Option<i32> {
    // (from the layout at normal size: a zoomed one's bigger cover would look like a new width)
    let (last, running, narrowed) = with(|a| (a.rest_layout, a.motion.resize_width(), a.fit.narrowed()))?;
    // the mode's own width: the cover and its margin are the same in every mode
    let text_w = (cfg.max_width * scale).round() as i32;
    let target = narrowed.or_else(|| last.map(|l| layout::strip(scale, l.cover, l.ring_x, cfg.compact, text_w).width));
    let changed = with(|a| {
        let changed = a.drawn_target.is_some() && target.is_some() && a.drawn_target != target;
        if target.is_some() {
            a.drawn_target = target;
        }
        changed
    }) == Some(true);
    if let (true, true, Some(to)) = (changed, animate, target) {
        let from = running.unwrap_or_else(|| {
            let r = unsafe { dock::rect(raw(hwnd)) };
            r.right - r.left
        });
        if with(|a| a.motion.resize(from, to, lk, Instant::now())) == Some(true) {
            if let Some(main) = with(|a| a.main) {
                unsafe { SetTimer(main, TIMER_RESIZE, 10, None); }
            }
        }
    }
    with(|a| a.motion.resize_width()).flatten().or(narrowed)
}

/// One frame of the collapse/expand animation (the last one draws the new mode).
fn resize_step() {
    let lk = look();
    let step = with(|a| a.motion.resize_step(&lk, Instant::now())).flatten();
    if step.map_or(true, |s| s.done) {
        if let Some(main) = with(|a| a.main) {
            unsafe { let _ = KillTimer(main, TIMER_RESIZE); }
        }
    }
    redraw_strip();
}

/// Every second: follow the shared style and Windows settings, keep the strip alive (Explorer
/// restarts destroy it), run the hide timer, and move the ring along.
fn watch() {
    // a paused stream sends no new reports: confirm live tracks here too
    with(|a| {
        let before = a.live.len();
        update_live(a, Instant::now());
        if a.live.len() != before {
            a.drawn = None;
        }
    });
    if refresh_look() {
        with(|a| a.drawn = None);
    }
    watch_config();
    // the brand file was edited: read it again, and the logos it names
    let mtime = brands_mtime();
    if with(|a| a.brands_mtime != mtime) == Some(true) {
        let b = brands::Brands::load();
        logos::forget();
        with(|a| {
            a.brands = b;
            a.brands_mtime = mtime;
            a.drawn = None;
        });
    }
    let tb = taskbar();
    let Some((strip, floating, force, failed, fs_hidden)) = with(|a| {
        if tb.is_some() {
            a.no_taskbar_since = None;
        }
        (a.strip, a.floating, a.force_float, a.attach_failed, a.fs_hidden)
    }) else { return };
    unsafe {
        match strip {
            // floating, and the taskbar is there (back, or worth another try): attach to it
            Some(h) if floating && !force && tb.is_some() && failed.is_none_or(|t| t.elapsed() >= ATTACH_RETRY) => {
                if let Some(child) = tb.and_then(attach) {
                    with(|a| a.strip = None);
                    let _ = DestroyWindow(h);
                    adopt(child, false);
                }
            }
            // floating: hide while a full-screen app is in front
            Some(h) if floating => {
                let fs = fullscreen_app_active();
                if fs != fs_hidden {
                    with(|a| {
                        a.fs_hidden = fs;
                        a.drawn = None;
                    });
                    if fs {
                        let _ = ShowWindow(h, SW_HIDE);
                    } else {
                        redraw_strip();
                    }
                } else if !fs && IsWindowVisible(h).as_bool() {
                    // the docked widgets moved: stay left of them
                    let r = dock::rect(raw(h));
                    let (w, ht) = (r.right - r.left, r.bottom - r.top);
                    let (x, y) = float_pos(w, ht, system::scale_for(raw(h)));
                    if (x, y) != (r.left, r.top) {
                        let _ = SetWindowPos(h, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
                    }
                }
            }
            // attached: Explorer restarted, the taskbar (and the strip with it) is gone
            Some(h) => {
                let parent_ok = GetParent(h).ok().is_some_and(|p| Some(p) == tb);
                if !IsWindow(h).as_bool() || !parent_ok {
                    with(|a| a.strip = None);
                    if IsWindow(h).as_bool() {
                        let _ = DestroyWindow(h);
                    }
                }
            }
            None => {}
        }
    }
    create_strip();
    reselect();
    check_fit(); // room next to the taskbar's buttons (also while squeezed out: no events then)
    update_hooks();
}

fn watch_config() {
    let mtime = settings::mtime();
    if with(|a| a.cfg_mtime != mtime) != Some(true) {
        return;
    }
    let cfg = Config::load();
    with(|a| {
        a.cfg = cfg.clone();
        a.cfg_mtime = settings::mtime();
        a.drawn = None;
    });
    apply_startup(cfg.start_with_windows);
}

fn set_config(f: impl FnOnce(&mut Config)) {
    let cfg = with(|a| {
        f(&mut a.cfg);
        a.cfg.save();
        a.cfg_mtime = settings::mtime();
        a.drawn = None;
        a.cfg.clone()
    });
    if let Some(cfg) = cfg {
        apply_startup(cfg.start_with_windows);
    }
    redraw_strip();
}

// ---------------------------------------------------------------- the menu panel's actions

/// "Edit settings…": the shared settings file in Notepad (edits apply live).
fn open_settings() {
    let path = wide(&settings::path().display().to_string());
    unsafe { ShellExecuteW(None, w!("open"), w!("notepad.exe"), PCWSTR(path.as_ptr()), None, SW_SHOWNORMAL); }
}

fn quit() {
    if let Some(main) = with(|a| a.main) {
        unsafe { let _ = DestroyWindow(main); }
    }
}

// ---------------------------------------------------------------- window procedures

extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MEDIA => {
                on_media(Box::from_raw(lp.0 as *mut Snapshot));
                LRESULT(0)
            }
            logos::WM_LOGO => {
                // a logo finished loading: draw it
                with(|a| a.drawn = None);
                redraw_strip();
                if let Some(Some(p)) = with(|a| a.popup.hwnd) {
                    let _ = InvalidateRect(p, None, false);
                }
                LRESULT(0)
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_WATCH => watch(),
                    TIMER_GLIDE => glide_step(),
                    TIMER_ZOOM => zoom_step(),
                    TIMER_BEAT => redraw_strip(),
                    TIMER_FADE => fade_step(),
                    TIMER_RESIZE => resize_step(),
                    TIMER_FEEL => feel_step(),
                    popup::TIMER_HOVER => popup::hover_step(),
                    popup::TIMER_FADE => popup::fade_step(),
                    popup::TIMER_FRAME => popup::frame(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_SETTINGCHANGE | WM_DISPLAYCHANGE | WM_DPICHANGED => {
                refresh_look();
                with(|a| a.drawn = None);
                redraw_strip();
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_DESTROY => {
                popup::close();
                destroy_strip();
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => {
                // Explorer restarted: the taskbar (and the strip with it) is new
                // (a floating strip survives it and attaches at the next watch)
                if with(|a| a.taskbar_created) == Some(msg) && msg != 0 {
                    // keep a strip already re-attached to the new taskbar: forgetting a live
                    // one left an untracked duplicate there (seen in weather, 2026-10-10)
                    let strip = with(|a| a.strip).flatten();
                    let current = strip.is_some_and(|h| IsWindow(h).as_bool() && GetParent(h).ok() == taskbar());
                    if with(|a| a.floating) == Some(false) && !current {
                        destroy_strip();
                        create_strip();
                    }
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
    }
}

fn contains(rc: &RECT, pt: POINT) -> bool {
    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
}

/// The part of the strip under the mouse position in a mouse message's `lp`.
fn part_at(lp: LPARAM) -> Part {
    let (x, y) = taskbar_dock::strip::client_pos(lp.0);
    with(|a| a.layout.map(|l| l.hit(x, y))).flatten().unwrap_or(Part::Body)
}

extern "system" fn strip_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONDOWN => {
                let part = part_at(lp);
                with(|a| a.pressed = Some(part));
                feel_update();
                mouse_down(hwnd);
                SetCapture(hwnd);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let part = with(|a| a.pressed.take()).flatten();
                feel_update();
                let release = mouse_up(hwnd); // a drop ends here
                let _ = ReleaseCapture(); // its WM_CAPTURECHANGED then finds nothing left to end
                match (release, part) {
                    // a button acts when released on the same button, and leaves the panel be
                    (Release::Click, Some(Part::Button(i))) => {
                        if part_at(lp) == Part::Button(i) {
                            press(i);
                        }
                        with(|a| a.panel.button_click(Instant::now()));
                    }
                    // the cover or text: the click panel opens or closes
                    (Release::Click, Some(Part::Body)) => popup::left_click(),
                    // the cover opens the app's window (when released on it)
                    (Release::Click, Some(Part::Cover)) => {
                        if part_at(lp) == Part::Cover {
                            popup::open_app();
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            // the capture lost (Esc, a window took the mouse): a drag ends like a drop, a click
            // does nothing
            WM_CAPTURECHANGED => {
                with(|a| a.pressed = None);
                feel_update();
                mouse_up(hwnd);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                popup::right_click(); // the menu panel
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let part = part_at(lp);
                if let Some((first, changed)) = with(|a| {
                    let old = a.hover.replace(part);
                    (old.is_none(), old != Some(part))
                }) {
                    if first {
                        let mut tme = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&mut tme);
                        popup::hover_started(); // the click panel opens if the mouse stays
                    }
                    if changed {
                        redraw_strip();
                    }
                }
                mouse_drag(); // button down: past the threshold it's a drag
                feel_update(); // also when the strip came to rest under a still-hovered cover
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                with(|a| a.hover = None);
                // the right-click menu also "leaves" (it takes the mouse); only a real leave,
                // with the pointer outside the strip, ends the block on hover-opening
                let (mut rc, mut pt) = (RECT::default(), POINT::default());
                let _ = GetWindowRect(hwnd, &mut rc);
                let _ = GetCursorPos(&mut pt);
                let outside = !contains(&rc, pt);
                with(|a| a.panel.mouse_left(outside));
                feel_update();
                redraw_strip();
                LRESULT(0)
            }
            WM_DESTROY => {
                with(|a| if a.strip == Some(hwnd) { a.strip = None });
                LRESULT(0)
            }
            // another dock widget opened its panel: close ours (one panel at a time)
            _ if msg == dock::close_panel_message() => {
                popup::closed(false);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn main() {
    unsafe {
        let _mutex = CreateMutexW(None, true, w!("Local\\TaskbarMedia.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        // a crash closes the widget at once (panic = abort): write why to the log first
        taskbar_dock::crash::log_panics(log);
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let hinst = hinstance();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let classes: [(PCWSTR, WNDPROC, WNDCLASS_STYLES); 3] = [
            (MAIN_CLASS, Some(main_proc), WNDCLASS_STYLES(0)),
            (STRIP_CLASS, Some(strip_proc), WNDCLASS_STYLES(0)),
            (popup::CLASS, Some(popup::proc_), CS_DROPSHADOW),
        ];
        for (name, proc_, style) in classes {
            let wc = WNDCLASSW { lpfnWndProc: proc_, hInstance: hinst, lpszClassName: name, hCursor: cursor, style, ..Default::default() };
            RegisterClassW(&wc);
        }
        let Ok(main) = CreateWindowExW(WINDOW_EX_STYLE(0), MAIN_CLASS, w!("Taskbar Media"), WS_OVERLAPPED,
            0, 0, 0, 0, None, None, hinst, None) else { return };

        let cfg = Config::load();
        apply_startup(cfg.start_with_windows);
        logos::init(main);
        let main_raw = main.0 as usize;
        let media = media::start(move |snap| {
            let p = Box::into_raw(Box::new(snap));
            if PostMessageW(HWND(main_raw as *mut c_void), WM_MEDIA, WPARAM(0), LPARAM(p as isize)).is_err() {
                drop(Box::from_raw(p));
            }
        });
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                main,
                strip: None,
                floating: false,
                force_float: std::env::args().any(|a| a == "--float"),
                attach_failed: None,
                no_taskbar_since: None,
                fs_hidden: false,
                cfg_mtime: settings::mtime(),
                cfg,
                look: Look::current(),
                media,
                sessions: Vec::new(),
                selector: select::Selector::default(),
                shown: None,
                hover: None,
                layout: None,
                rest_layout: None,
                drawn: None,
                drawn_target: None,
                fit: dock::FitState::default(),
                size: None,
                taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
                hooks: dock::Hooks::default(),
                motion: Default::default(),
                pressed: None,
                feel: anim::Feel::new(),
                feeling: false,
                pulse_since: None,
                settle: None,
                was_playing: false,
                beating: false,
                appear: Appear::hidden(Instant::now()),
                last_content: None,
                learned: learned::Learned::load(),
                live: std::collections::HashSet::new(),
                live_since: std::collections::HashMap::new(),
                played: std::collections::HashSet::new(),
                has_length: std::collections::HashSet::new(),
                brands: brands::Brands::load(),
                brands_mtime: brands_mtime(),
                tint: Appear::hidden(Instant::now()),
                panel: PanelBehaviour::default(),
                pfade: Fade::default(),
                popup: popup::Panel::default(),
            })
        });
        create_strip();
        update_hooks();
        SetTimer(main, TIMER_WATCH, 1000, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
