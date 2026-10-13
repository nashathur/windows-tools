//! Which site a browser is playing: the title of the browser tab whose title contains what's
//! playing, read through UI Automation (what screen readers use; read-only). The brand file then
//! finds the site in that title ("… - YouTube").
//!
//! Measured on this PC: Firefox's 28 tabs in two windows read in about 200 ms; tab titles are
//! readable for background tabs too; Firefox shows no "playing" mark through UI Automation, so
//! two tabs with the same title can't be told apart (they're on the same site anyway).
//! Runs on the media worker, only when a browser's track changes.

use windows::core::{BSTR, VARIANT};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Accessibility::*;

/// Browser window classes: Firefox, and Chrome / Edge (Chromium).
const BROWSER_WINDOWS: [&str; 2] = ["MozillaWindowClass", "Chrome_WidgetWin_1"];

/// Is this media session id a browser's (where a site can be playing)?
pub fn is_browser(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    lower.contains("firefox") || lower.contains("chrome") || lower.contains("msedge") || !crate::apps::firefox_folders(id).is_empty()
}

/// The title of a browser tab containing `media_title` (e.g. "#HiddenBeat … - YouTube").
pub fn tab_with(media_title: &str) -> Option<String> {
    let wanted = media_title.trim();
    if wanted.is_empty() {
        return None;
    }
    unsafe {
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let root = uia.GetRootElement().ok()?;
        let tab_cond = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_TabItemControlTypeId.0)).ok()?;
        for class in BROWSER_WINDOWS {
            let Ok(win_cond) = uia.CreatePropertyCondition(UIA_ClassNamePropertyId, &VARIANT::from(BSTR::from(class))) else { continue };
            let Ok(windows) = root.FindAll(TreeScope_Children, &win_cond) else { continue };
            for w in 0..windows.Length().unwrap_or(0) {
                let Ok(win) = windows.GetElement(w) else { continue };
                let Ok(tabs) = win.FindAll(TreeScope_Descendants, &tab_cond) else { continue };
                for t in 0..tabs.Length().unwrap_or(0) {
                    let Ok(name) = tabs.GetElement(t).and_then(|e| e.CurrentName()) else { continue };
                    let name = name.to_string();
                    if name.contains(wanted) {
                        return Some(name);
                    }
                }
            }
        }
        None
    }
}

/// A host when UI Automation exposed the address itself, or a standalone host in the tab title.
/// Ordinary page titles usually have no host; callers then use the app's colour until an icon is
/// available through a known site brand.
pub fn icon_host(tab: &str) -> Option<String> {
    let clean = tab.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let host = if let Some(rest) = clean.strip_prefix("https://").or_else(|| clean.strip_prefix("http://")) {
        rest.split(&['/', '?', '#'][..]).next().unwrap_or("")
    } else {
        clean.split_whitespace().find(|part| part.contains('.')).unwrap_or("")
    };
    let host = host.trim_start_matches("www.").trim_end_matches('.');
    (host.contains('.') && !host.contains(':') && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
        .then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::icon_host;

    #[test]
    fn icon_host_only_uses_an_exposed_host() {
        assert_eq!(icon_host("https://www.example.com/watch?v=1"), Some("example.com".into()));
        assert_eq!(icon_host("Playing - example.org"), Some("example.org".into()));
        assert_eq!(icon_host("A song - YouTube"), None);
    }
}
