//! The Windows settings the widgets follow live. All reads are cheap (registry / system
//! parameters), so a widget can call [`read`] every second and react to any change, besides
//! handling WM_SETTINGCHANGE for an instant theme switch.

use crate::wide;
use std::ffi::c_void;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;

/// A snapshot of the Windows settings the widgets use.
#[derive(Clone, PartialEq, Debug)]
pub struct System {
    /// Taskbar in light mode (Settings > Personalization > Colors > "Choose your mode").
    pub light: bool,
    /// Settings > Accessibility > Visual effects > Animation effects.
    pub animations: bool,
    /// How long the mouse rests before "hover" (Windows default 400 ms).
    pub hover_ms: u32,
    /// Settings > Accessibility > Text size, as a factor (1.0 = 100%).
    pub text_scale: f32,
    /// Short time pattern from the region format, e.g. "h:mm tt" or "HH:mm".
    pub time_pattern: String,
    /// AM / PM designators.
    pub am: String,
    pub pm: String,
}

fn reg_dword(key: &str, name: &str) -> Option<u32> {
    let (mut v, mut sz) = (0u32, 4u32);
    let (k, n) = (wide(key), wide(name));
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut v as *mut u32 as *mut c_void,
            &mut sz,
        )
    };
    (r == 0).then_some(v)
}

fn reg_string(key: &str, name: &str) -> Option<String> {
    let (k, n) = (wide(key), wide(name));
    let mut buf = [0u16; 128];
    let mut sz = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut sz,
        )
    };
    if r != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn spi_u32(action: u32) -> Option<u32> {
    let mut v: u32 = 0;
    let ok = unsafe { SystemParametersInfoW(action, 0, &mut v as *mut u32 as *mut c_void, 0) };
    (ok != 0).then_some(v)
}

/// Reads the current Windows settings.
pub fn read() -> System {
    const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;
    const SPI_GETMOUSEHOVERTIME: u32 = 0x0066;
    const INTL: &str = r"Control Panel\International";
    System {
        light: reg_dword(
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "SystemUsesLightTheme",
        ) == Some(1),
        animations: spi_u32(SPI_GETCLIENTAREAANIMATION).map_or(true, |v| v != 0),
        hover_ms: spi_u32(SPI_GETMOUSEHOVERTIME)
            .unwrap_or(400)
            .clamp(50, 5000),
        text_scale: reg_dword(r"Software\Microsoft\Accessibility", "TextScaleFactor")
            .map_or(1.0, |v| v.clamp(100, 225) as f32 / 100.0),
        time_pattern: reg_string(INTL, "sShortTime").unwrap_or_else(|| "HH:mm".into()),
        am: reg_string(INTL, "s1159").unwrap_or_else(|| "AM".into()),
        pm: reg_string(INTL, "s2359").unwrap_or_else(|| "PM".into()),
    }
}

/// Display scale of a window (1.0 = 100%), live: follows the monitor it's on.
pub fn scale_for(hwnd: HWND) -> f32 {
    let dpi = unsafe {
        if hwnd != 0 {
            GetDpiForWindow(hwnd)
        } else {
            0
        }
    };
    let dpi = if dpi == 0 {
        unsafe { GetDpiForSystem() }
    } else {
        dpi
    };
    dpi.max(96) as f32 / 96.0
}

/// Formats a time of day with a Windows time pattern (h, hh, H, HH, m, mm, s, ss, t, tt;
/// text in single quotes is copied). `seconds`: also show seconds (inserted after the minutes
/// when the pattern has none).
pub fn format_time(
    pattern: &str,
    am: &str,
    pm: &str,
    hour: u32,
    minute: u32,
    seconds: Option<u32>,
) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut n = 1;
        while i + n < chars.len() && chars[i + n] == c {
            n += 1;
        }
        match c {
            'h' => {
                let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
                out += &if n >= 2 {
                    format!("{h12:02}")
                } else {
                    h12.to_string()
                };
            }
            'H' => {
                out += &if n >= 2 {
                    format!("{hour:02}")
                } else {
                    hour.to_string()
                }
            }
            'm' => {
                out += &if n >= 2 {
                    format!("{minute:02}")
                } else {
                    minute.to_string()
                };
                if let (Some(s), false) = (seconds, pattern.contains('s')) {
                    out += &format!(":{s:02}");
                }
            }
            's' => {
                if let Some(s) = seconds {
                    out += &if n >= 2 {
                        format!("{s:02}")
                    } else {
                        s.to_string()
                    };
                } else {
                    // no seconds wanted: drop the separator before them too
                    while out.ends_with(|ch: char| !ch.is_alphanumeric()) {
                        out.pop();
                    }
                }
            }
            't' => {
                let d = if hour < 12 { am } else { pm };
                out += if n >= 2 { d } else { &d[..d.len().min(1)] };
            }
            '\'' => {
                let end = chars[i + 1..]
                    .iter()
                    .position(|&ch| ch == '\'')
                    .map_or(chars.len(), |p| i + 1 + p);
                out.extend(&chars[i + 1..end]);
                i = end + 1;
                continue;
            }
            _ => out.extend(std::iter::repeat(c).take(n)),
        }
        i += n;
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::format_time;

    #[test]
    fn time_patterns() {
        assert_eq!(format_time("h:mm tt", "AM", "PM", 19, 45, None), "7:45 PM");
        assert_eq!(format_time("h:mm tt", "AM", "PM", 0, 5, None), "12:05 AM");
        assert_eq!(format_time("HH:mm", "", "", 7, 5, None), "07:05");
        assert_eq!(format_time("H:mm", "", "", 7, 5, None), "7:05");
        assert_eq!(
            format_time("h:mm tt", "AM", "PM", 19, 45, Some(12)),
            "7:45:12 PM"
        );
        assert_eq!(format_time("HH:mm:ss", "", "", 19, 45, None), "19:45");
        assert_eq!(format_time("HH:mm:ss", "", "", 19, 45, Some(3)), "19:45:03");
    }
}
