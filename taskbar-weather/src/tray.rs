//! The tray icon, shown only while the taskbar label is off.

use super::*;

/// The tray icon only appears when the taskbar label is turned off, so the app always
/// has exactly one place on the taskbar (and its menu stays reachable).
pub(crate) fn update_tray(add: bool) {
    let Some((main, label_on)) = with(|a| (a.main, a.cfg.taskbar_label)) else { return };
    if label_on {
        remove_tray(main);
        if let Some(Some(old)) = with(|a| a.icon.take()) {
            unsafe { let _ = DestroyIcon(old); }
        }
        return;
    }
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) };
    let Some((main, old, nid)) = with(|a| {
        let (text, tip) = match &a.weather {
            Some(wx) => {
                let (_, desc) = describe(wx.code, wx.is_day);
                let mut tip = format!(
                    "{}{} {} – {}\nFeels like {} · Humidity {:.0}% · Wind {:.0} {}",
                    deg(wx.temp), &wx.unit()[2..], desc, wx.location,
                    deg(wx.feels), wx.humidity, wx.wind, wx.wind_unit()
                );
                match (&wx.observed, &wx.obs_error) {
                    (Some(o), _) => tip.push_str(&format!("\nMeasured: KNMI {o}")),
                    (None, Some(e)) => tip.push_str(&format!("\n{e}")),
                    _ => {}
                }
                if let Some(e) = &a.error {
                    tip.push_str(&format!("\nUpdate failed: {e}"));
                }
                (deg(wx.temp), tip)
            }
            None => match &a.error {
                Some(e) => ("?".to_string(), format!("Weather unavailable: {e}")),
                None => ("…".to_string(), "Loading weather…".to_string()),
            },
        };
        let icon = render::tray_icon(&text, size, a.look.light);
        let old = std::mem::replace(&mut a.icon, icon);
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: a.main,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_TRAY,
            hIcon: icon.unwrap_or_default(),
            ..Default::default()
        };
        for (dst, src) in nid.szTip.iter_mut().take(127).zip(tip.encode_utf16()) {
            *dst = src;
        }
        (a.main, old, nid)
    }) else { return };
    let _ = main;
    unsafe {
        if add || !Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() {
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        }
        if let Some(old) = old {
            let _ = DestroyIcon(old);
        }
    }
}

pub(crate) fn remove_tray(main: HWND) {
    let nid = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: main, uID: 1, ..Default::default() };
    unsafe { let _ = Shell_NotifyIconW(NIM_DELETE, &nid); }
}
