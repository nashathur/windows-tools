//! The tray icon, shown only while the taskbar label is off.

use super::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, SM_CXSMICON};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};

/// The tray icon only appears when the taskbar label is turned off, so the app always has exactly
/// one place on the taskbar (and its menu stays reachable). `add`: (re-)add it (start, Explorer
/// restart). The shell call is made after Block's borrow ([`Ctx::after`]).
pub(crate) fn update(a: &mut App, ctx: &mut Ctx, add: bool) {
    if a.cfg.taskbar_label {
        let (main, old) = (a.main, a.icon.take());
        ctx.after(move || unsafe {
            remove_icon(main);
            if let Some(old) = old {
                let _ = DestroyIcon(old);
            }
        });
        return;
    }
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) };
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
        hWnd: hwnd(a.main),
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAY,
        hIcon: icon.unwrap_or_default(),
        ..Default::default()
    };
    for (dst, src) in nid.szTip.iter_mut().take(127).zip(tip.encode_utf16()) {
        *dst = src;
    }
    ctx.after(move || unsafe {
        if add || !Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() {
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        }
        if let Some(old) = old {
            let _ = DestroyIcon(old);
        }
    });
}

/// The program is ending.
pub(crate) fn remove(a: &mut App) {
    unsafe {
        remove_icon(a.main);
        if let Some(icon) = a.icon.take() {
            let _ = DestroyIcon(icon);
        }
    }
}

unsafe fn remove_icon(main: isize) {
    let nid = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd(main), uID: 1, ..Default::default() };
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}
