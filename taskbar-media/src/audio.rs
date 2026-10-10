//! Per-app volume and mute: the same per-app entries as Windows' Volume Mixer (WASAPI audio
//! sessions), on every active output device, linked to a media session by the app's program
//! path (apps::path_belongs). Setting the volume sets every entry of the app together, so
//! switching output devices doesn't bring back an old level.
//!
//! Measured on this PC: Spotify and Firefox each have their own entries (Firefox on two
//! devices); FxSound is in the audio path and has its own entry, which never matches an app,
//! so it's never touched. System sounds (process 0) never match either.
//!
//! Runs on the UI thread (COM is initialised there); all calls are quick.

use windows::core::{Interface, PWSTR};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};

/// The app's volume (0..1, the first entry's) and whether it's muted (every entry).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Volume {
    pub level: f32,
    pub muted: bool,
}

fn exe_of(pid: u32) -> Option<String> {
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(p);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// The volume controls of every audio entry of the app with this media session id (the
/// panel keeps them, refreshed every second, rather than looking them up on every change).
pub fn entries(id: &str) -> Vec<ISimpleAudioVolume> {
    let ff = crate::apps::firefox_folders(id);
    let mut out = Vec::new();
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return out };
        let Ok(devices) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return out };
        for d in 0..devices.GetCount().unwrap_or(0) {
            let Ok(dev) = devices.Item(d) else { continue };
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let Ok(list) = mgr.GetSessionEnumerator() else { continue };
            for i in 0..list.GetCount().unwrap_or(0) {
                let Ok(ctl) = list.GetSession(i) else { continue };
                let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
                if ctl.GetState().is_ok_and(|s| s == AudioSessionStateExpired) {
                    continue;
                }
                let pid = ctl2.GetProcessId().unwrap_or(0);
                if pid == 0 {
                    continue; // system sounds
                }
                if exe_of(pid).is_some_and(|p| crate::apps::path_belongs(id, &p, &ff)) {
                    if let Ok(v) = ctl.cast::<ISimpleAudioVolume>() {
                        out.push(v);
                    }
                }
            }
        }
    }
    out
}

/// The app's volume, or None if it has no entry in the Volume Mixer.
pub fn get(all: &[ISimpleAudioVolume]) -> Option<Volume> {
    let first = all.first()?;
    unsafe {
        let level = first.GetMasterVolume().ok()?;
        let muted = all.iter().all(|v| v.GetMute().is_ok_and(|m| m.as_bool()));
        Some(Volume { level, muted })
    }
}

/// Sets the app's volume (0..1) on every entry; unmutes it if it was muted and the level is
/// above 0 (like the Volume Mixer's slider).
pub fn set_level(all: &[ISimpleAudioVolume], level: f32) {
    let level = level.clamp(0.0, 1.0);
    for v in all {
        unsafe {
            let _ = v.SetMasterVolume(level, std::ptr::null());
            if level > 0.0 {
                let _ = v.SetMute(false, std::ptr::null());
            }
        }
    }
}

pub fn set_muted(all: &[ISimpleAudioVolume], muted: bool) {
    for v in all {
        unsafe { let _ = v.SetMute(muted, std::ptr::null()); }
    }
}
