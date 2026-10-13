//! Logos for media without a cover picture, from the brand file's `logo` column:
//! - `app`: the installed app's own icon, as the Start menu shows it (nothing downloaded);
//! - a domain (`youtube.com`): the site's own icon, found in its home page (iconurl.rs);
//! - a full https URL to a PNG or ICO.
//!
//! Downloads go into `logos\` next to the exe and are refreshed after 30 days. Only a picture
//! that decodes and is at most 1 MB is kept. A failed download keeps any old copy and isn't
//! tried again for a day. Everything happens on background threads; when a logo is ready the
//! widget is told to redraw.

use crate::cover::{self, Cover};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Foundation::Uri;
use windows::Storage::Streams::{DataReader, DataWriter, IRandomAccessStream, InMemoryRandomAccessStream};
use windows::Web::Http::HttpClient;
use windows::Win32::Foundation::{HWND, LPARAM, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_RESIZETOFIT};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Posted to the main window when a logo has loaded.
pub const WM_LOGO: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;
const MAX_BYTES: u32 = 1024 * 1024;
const REFRESH: Duration = Duration::from_secs(30 * 24 * 3600);
const RETRY: Duration = Duration::from_secs(24 * 3600);
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0";

enum Slot {
    Loading,
    Ready(Option<Arc<Cover>>),
}

static SLOTS: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
static MAIN: AtomicUsize = AtomicUsize::new(0);
static IDLE: AtomicBool = AtomicBool::new(false);
pub fn set_idle(idle: bool) { IDLE.store(idle, Ordering::Relaxed); }

fn slots() -> &'static Mutex<HashMap<String, Slot>> {
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The window to tell when a logo is ready.
pub fn init(main: HWND) {
    MAIN.store(main.0 as usize, Ordering::Relaxed);
}

/// The brand file changed: forget what was loaded, so edits apply.
pub fn forget() {
    slots().lock().unwrap().retain(|_, s| matches!(s, Slot::Loading));
}

/// The logo for a brand's `logo` value (`app_id`: the media session's app, for `app`). None
/// while it loads (the widget is told when it's ready) or if there is none.
pub fn get(logo: &str, app_id: &str) -> Option<Arc<Cover>> {
    if IDLE.load(Ordering::Relaxed) { return None; }
    let key = match logo.trim() {
        "" => return None,
        "app" => format!("app:{app_id}"),
        l => l.to_string(),
    };
    let mut map = slots().lock().unwrap();
    match map.get(&key) {
        Some(Slot::Ready(c)) => c.clone(),
        Some(Slot::Loading) => None,
        None => {
            map.insert(key.clone(), Slot::Loading);
            std::thread::spawn(move || {
                unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
                let logo = load(&key).map(Arc::new);
                slots().lock().unwrap().insert(key, Slot::Ready(logo));
                let main = MAIN.load(Ordering::Relaxed);
                if main != 0 {
                    unsafe { let _ = PostMessageW(HWND(main as *mut _), WM_LOGO, WPARAM(0), LPARAM(0)); }
                }
            });
            None
        }
    }
}

fn load(key: &str) -> Option<Cover> {
    if let Some(id) = key.strip_prefix("app:") {
        return app_icon(id);
    }
    let file = cache_file(key)?;
    let fail = file.with_extension("fail");
    let age = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
    let cached = || std::fs::read(&file).ok().and_then(|b| decode_bytes(&b));
    let fresh = age(&file).is_some_and(|a| a < REFRESH);
    let failed_recently = age(&fail).is_some_and(|a| a < RETRY);
    if fresh || failed_recently {
        return cached();
    }
    match download(key) {
        Some(bytes) => {
            let tmp = file.with_extension("tmp");
            if std::fs::write(&tmp, &bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &file);
            }
            let _ = std::fs::remove_file(&fail);
            decode_bytes(&bytes)
        }
        None => {
            crate::log(&format!("logo for {key}: couldn't download one; trying again tomorrow"));
            let _ = std::fs::write(&fail, b"");
            cached()
        }
    }
}

fn cache_file(key: &str) -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.with_file_name("logos");
    std::fs::create_dir_all(&dir).ok()?;
    let name: String = key.trim_start_matches("https://").chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).take(80).collect();
    Some(dir.join(format!("{name}.img")))
}

fn http() -> Option<HttpClient> {
    let c = HttpClient::new().ok()?;
    let _ = c.DefaultRequestHeaders().ok()?.UserAgent().ok()?.TryParseAdd(&HSTRING::from(USER_AGENT));
    Some(c)
}

fn get_bytes(c: &HttpClient, url: &str) -> Option<Vec<u8>> {
    let uri = Uri::CreateUri(&HSTRING::from(url)).ok()?;
    let buf = c.GetBufferAsync(&uri).ok()?.get().ok()?;
    let len = buf.Length().ok()?;
    if len == 0 || len > MAX_BYTES {
        return None;
    }
    let reader = DataReader::FromBuffer(&buf).ok()?;
    let mut out = vec![0u8; len as usize];
    reader.ReadBytes(&mut out).ok()?;
    Some(out)
}

/// The site's icon (or the URL's picture): the first candidate that downloads and decodes.
fn download(key: &str) -> Option<Vec<u8>> {
    let c = http()?;
    let urls = if key.starts_with("https://") {
        vec![key.to_string()]
    } else {
        let page = Uri::CreateUri(&HSTRING::from(format!("https://{key}/"))).ok()
            .and_then(|u| c.GetStringAsync(&u).ok()?.get().ok())
            .map(|h| h.to_string())
            .unwrap_or_default();
        crate::iconurl::candidates(&page, key)
    };
    urls.iter().find_map(|u| get_bytes(&c, u).filter(|b| decode_bytes(b).is_some()))
}

fn decode_bytes(bytes: &[u8]) -> Option<Cover> {
    let stream = InMemoryRandomAccessStream::new().ok()?;
    let w = DataWriter::CreateDataWriter(&stream).ok()?;
    w.WriteBytes(bytes).ok()?;
    w.StoreAsync().ok()?.get().ok()?;
    w.FlushAsync().ok()?.get().ok()?;
    let _ = w.DetachStream();
    stream.Seek(0).ok()?;
    let s: IRandomAccessStream = stream.cast().ok()?;
    crate::media::decode_logo(&s).ok()
}

/// The app's icon as the Start menu shows it ("shell:AppsFolder\<id>"), 256 px.
fn app_icon(id: &str) -> Option<Cover> {
    let path: Vec<u16> = format!("shell:AppsFolder\\{id}").encode_utf16().chain(Some(0)).collect();
    unsafe {
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None).ok()?;
        let bmp = f.GetImage(SIZE { cx: 256, cy: 256 }, SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK).ok()?;
        let mut info = BITMAP::default();
        GetObjectW(bmp, std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut _));
        let (w, h) = (info.bmWidth, info.bmHeight.abs());
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u32; (w * h).max(0) as usize];
        let dc = CreateCompatibleDC(None);
        let rows = GetDIBits(dc, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        let _ = DeleteObject(bmp);
        if rows == 0 || w <= 0 || h <= 0 {
            return None;
        }
        Some(cover::fit(&cover::premultiply(px), w, h, cover::KEEP))
    }
}
