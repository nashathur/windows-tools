//! The tray icon's window side: the widget's drawing ([`Content::tray_icon`]) as an HICON, and
//! adding / changing / removing it. When it shows is `core::tray_shown`; `win.rs` keeps it in sync
//! (every redraw, theme change, Explorer restart, Quit) and handles its clicks ([`WM_TRAY`]).

use super::*;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO, SM_CXSMICON, WM_APP};

/// The tray icon's callback message to Block's main window (widgets use `WM_APP + n` well below).
pub(crate) const WM_TRAY: u32 = WM_APP + 0x3F00;
const ID: u32 = 1;

/// The tray's icon size, px.
pub(crate) fn size() -> i32 {
    unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) }.max(16)
}

/// Asks the Content for its logo at `size` px: None when it has no tray icon (or drawing failed),
/// else the icon (0 when making it failed).
pub(crate) fn draw(content: &mut dyn Content, look: &Look, size: i32) -> Option<HICON> {
    let mut cv = Canvas::new(size, size)?;
    if !content.tray_icon(&mut cv, look) {
        return None;
    }
    // the drawing is premultiplied (as the strip's); an icon takes straight alpha
    let px: Vec<u32> = cv.pixels().iter().map(|&p| {
        let a = p >> 24;
        if a == 0 || a == 255 {
            return p;
        }
        let ch = |s: u32| (((p >> s) & 0xff) * 255 / a).min(255) << s;
        a << 24 | ch(16) | ch(8) | ch(0)
    }).collect();
    unsafe {
        let color = CreateBitmap(size, size, 1, 32, px.as_ptr() as *const _);
        let mask_bits = vec![0u8; ((size + 15) / 16 * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, mask_bits.as_ptr() as *const _);
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(mask);
        DeleteObject(color);
        Some(icon)
    }
}

/// Adds the icon (`add`, or when changing it fails: Explorer restarted) or changes it.
pub(crate) unsafe fn set(main: HWND, icon: HICON, tip: &str, add: bool) {
    let mut nid = data(main);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon;
    for (dst, src) in nid.szTip.iter_mut().take(127).zip(tip.encode_utf16()) {
        *dst = src;
    }
    if add || Shell_NotifyIconW(NIM_MODIFY, &nid) == 0 {
        Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

pub(crate) unsafe fn remove(main: HWND) {
    Shell_NotifyIconW(NIM_DELETE, &data(main));
}

pub(crate) unsafe fn destroy(icon: HICON) {
    if icon != 0 {
        DestroyIcon(icon);
    }
}

fn data(main: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = main;
    nid.uID = ID;
    nid
}
