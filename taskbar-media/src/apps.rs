//! The apps behind media sessions: a readable name, and bringing the app's window to the front.
//!
//! A session names its app by AppUserModelID. Store apps' ids are "PackageFamily!App"
//! (Spotify: "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"); Firefox's is a hash of its
//! install folder, kept in the registry (HKCU\Software\Mozilla\Firefox\TaskBarIDs: folder →
//! id), and its windows carry that id as a property.

use crate::render::wide;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};

/// Firefox install folders by their AppUserModelID (from the registry).
fn firefox_ids() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        unsafe {
            let mut key = HKEY::default();
            let path = wide(r"Software\Mozilla\Firefox\TaskBarIDs");
            if RegOpenKeyExW(root, PCWSTR(path.as_ptr()), 0, KEY_READ, &mut key).is_err() {
                continue;
            }
            for i in 0.. {
                let mut name = [0u16; 512];
                let mut name_len = name.len() as u32;
                let mut data = [0u16; 128];
                let mut data_len = (data.len() * 2) as u32;
                let mut kind = 0u32;
                if RegEnumValueW(key, i, PWSTR(name.as_mut_ptr()), &mut name_len, None, Some(&mut kind),
                    Some(data.as_mut_ptr() as *mut u8), Some(&mut data_len)).is_err() {
                    break;
                }
                let folder = String::from_utf16_lossy(&name[..name_len as usize]);
                let id: String = String::from_utf16_lossy(&data[..(data_len as usize / 2)]).trim_end_matches('\0').to_string();
                out.push((id, folder));
            }
            let _ = RegCloseKey(key);
        }
    }
    out
}

/// "Spotify", "Firefox", or the best guess from the id.
pub fn display_name(id: &str) -> String {
    let lower = id.to_ascii_lowercase();
    if lower.contains("spotify") {
        return "Spotify".into();
    }
    if lower.contains("firefox") || firefox_ids().iter().any(|(i, _)| i.eq_ignore_ascii_case(id)) {
        return "Firefox".into();
    }
    if lower.contains("chrome") {
        return "Chrome".into();
    }
    if lower.contains("msedge") {
        return "Edge".into();
    }
    if lower.contains("zunemusic") {
        return "Media Player".into();
    }
    // "Publisher.AppName_hash!App" → "AppName"; "C:\…\vlc.exe" → "vlc"
    let base = id.rsplit(['\\', '/']).next().unwrap_or(id);
    let base = base.split('!').next().unwrap_or(base);
    let base = base.split('_').next().unwrap_or(base);
    let base = base.trim_end_matches(".exe");
    base.rsplit('.').next().filter(|s| !s.is_empty()).unwrap_or(base).to_string()
}

/// The AppUserModelID set on a window (Firefox sets one), if any.
unsafe fn window_id(h: HWND) -> Option<String> {
    let store: IPropertyStore = SHGetPropertyStoreForWindow(h).ok()?;
    let v = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
    let s = windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc(&v).ok()?;
    let out = s.to_string().ok();
    windows::Win32::System::Com::CoTaskMemFree(Some(s.0 as *const _));
    out.filter(|s| !s.is_empty())
}

fn process_path(h: HWND) -> Option<String> {
    taskbar_dock::window::exe_path(h.0 as isize)
}

/// Does window `h` belong to the app with this id?
unsafe fn belongs(h: HWND, id: &str, ff_folders: &[String]) -> bool {
    if window_id(h).is_some_and(|w| w.eq_ignore_ascii_case(id)) {
        return true;
    }
    process_path(h).is_some_and(|p| path_belongs(id, &p, ff_folders))
}

/// Firefox install folders whose AppUserModelID is `id` (from the registry).
pub fn firefox_folders(id: &str) -> Vec<String> {
    firefox_ids().into_iter().filter(|(i, _)| i.eq_ignore_ascii_case(id)).map(|(_, f)| f).collect()
}

/// Is the program at `path` (e.g. "C:\Program Files\Mozilla Firefox\firefox.exe") the app with
/// this media session id? `ff_folders`: Firefox install folders with this id (firefox_folders).
pub fn path_belongs(id: &str, path: &str, ff_folders: &[String]) -> bool {
    let path = path.to_ascii_lowercase();
    // a Store app: its exe lives in a folder named after the package (name and publisher id)
    if let Some(family) = id.split('!').next().filter(|_| id.contains('!')) {
        if let Some((name, publisher)) = family.rsplit_once('_') {
            return path.contains(&name.to_ascii_lowercase()) && path.contains(&publisher.to_ascii_lowercase());
        }
    }
    if ff_folders.iter().any(|f| path.starts_with(&(f.to_ascii_lowercase().trim_end_matches('\\').to_string() + "\\"))) {
        return true;
    }
    // otherwise an id naming the program, e.g. "Chrome" or "C:\…\vlc.exe": the exe's name in it
    let stem = path.rsplit('\\').next().unwrap_or(&path).trim_end_matches(".exe");
    stem.len() >= 3 && id.to_ascii_lowercase().contains(stem)
}

/// Brings the app's main window to the front (restored if minimised); if none is found, asks
/// the shell to open the app, which does the same for a running one.
pub fn activate(id: &str) {
    let ff_folders = firefox_folders(id);
    let is_app = |h: isize| unsafe { belongs(HWND(h as *mut std::ffi::c_void), id, &ff_folders) };
    taskbar_dock::window::raise(&is_app, &format!("shell:AppsFolder\\{id}"));
}

#[cfg(test)]
mod tests {
    use super::{display_name, path_belongs};

    #[test]
    fn names() {
        assert_eq!(display_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), "Spotify");
        assert_eq!(display_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "Media Player");
        assert_eq!(display_name(r"C:\Program Files\VideoLAN\VLC\vlc.exe"), "vlc");
        assert_eq!(display_name("Contoso.Player_abc123!App"), "Player");
    }

    #[test]
    fn programs_belong_to_their_apps() {
        let spotify = "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify";
        let sp_exe = r"C:\Program Files\WindowsApps\SpotifyAB.SpotifyMusic_1.301.234.0_x64__zpdnekdrzrea0\Spotify.exe";
        assert!(path_belongs(spotify, sp_exe, &[]));
        let ff = vec![r"C:\Program Files\Mozilla Firefox".to_string()];
        let ff_exe = r"C:\Program Files\Mozilla Firefox\firefox.exe";
        assert!(path_belongs("308046B0AF4A39CB", ff_exe, &ff));
        // never FxSound or another app
        let fx = r"C:\Program Files\FxSound LLC\FxSound\FxSound.exe";
        assert!(!path_belongs(spotify, fx, &[]) && !path_belongs("308046B0AF4A39CB", fx, &ff));
        assert!(!path_belongs(spotify, ff_exe, &[]));
        assert!(!path_belongs("308046B0AF4A39CB", r"C:\Program Files\Mozilla Firefox Nightly\firefox.exe", &ff));
        // an id naming the program
        assert!(path_belongs("Chrome", r"C:\Program Files\Google\Chrome\Application\chrome.exe", &[]));
    }
}
