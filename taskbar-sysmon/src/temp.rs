#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TempBlock {
    pub magic: u32,
    pub version: u32,
    pub status: u32,
    pub temp_centi_c: i32,
    pub unix_ms: u64,
}
const MAGIC: u32 = 0x504D_5453;
pub fn validate(a: TempBlock, b: TempBlock, now_ms: u64) -> Option<f64> {
    // unix_ms 0: the helper is mid-write; a changed unix_ms: a write happened during the read
    if a.unix_ms != b.unix_ms
        || b.unix_ms == 0
        || b.magic != MAGIC
        || b.version != 1
        || b.status != 0
        || now_ms.abs_diff(b.unix_ms) > 3000
    {
        None
    } else {
        Some(b.temp_centi_c as f64 / 100.)
    }
}
pub fn read() -> Option<f64> {
    #[cfg(windows)]
    unsafe {
        use std::{ffi::c_void, ptr};
        type H = *mut c_void;
        #[link(name = "kernel32")]
        extern "system" {
            fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> H;
            fn MapViewOfFile(h: H, access: u32, hi: u32, lo: u32, size: usize) -> *const c_void;
            fn UnmapViewOfFile(p: *const c_void) -> i32;
            fn CloseHandle(h: H) -> i32;
        }
        let name: Vec<u16> = "Local\\TaskbarSysmon.Temp"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let h = OpenFileMappingW(4, 0, name.as_ptr());
        if h.is_null() {
            return None;
        }
        let p = MapViewOfFile(h, 4, 0, 0, 24);
        if p.is_null() {
            CloseHandle(h);
            return None;
        }
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        };
        let mut out = None;
        for _ in 0..2 {
            let first = ptr::read_volatile(p.add(16) as *const u64);
            let mut block = ptr::read_volatile(p as *const TempBlock);
            let last = ptr::read_volatile(p.add(16) as *const u64);
            block.unix_ms = last;
            out = validate(
                TempBlock {
                    unix_ms: first,
                    ..block
                },
                block,
                now(),
            );
            if first == last {
                break;
            }
        }
        UnmapViewOfFile(p);
        CloseHandle(h);
        out
    }
    #[cfg(not(windows))]
    {
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_bad_temp_blocks() {
        let good = TempBlock {
            magic: MAGIC,
            version: 1,
            status: 0,
            temp_centi_c: 5400,
            unix_ms: 10_000,
        };
        assert_eq!(validate(good, good, 10_500), Some(54.));
        assert_eq!(validate(good, TempBlock { magic: 0, ..good }, 10_500), None);
        assert_eq!(
            validate(good, TempBlock { version: 2, ..good }, 10_500),
            None
        );
        assert_eq!(
            validate(
                TempBlock { unix_ms: 1, ..good },
                TempBlock { unix_ms: 1, ..good },
                10_000
            ),
            None
        )
    }
}
