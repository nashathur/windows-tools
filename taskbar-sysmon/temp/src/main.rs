#![windows_subsystem = "windows"]

use std::ffi::{c_void, CStr};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::mem::{size_of, transmute_copy};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{
    CloseHandle, FreeLibrary, GetLastError, LocalFree, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
    WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, REG_SZ,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

const MAP_NAME: &str = "Local\\TaskbarSysmon.Temp";
const INSTANCE_MUTEX: &str = "Local\\TaskbarSysmon.TempHelper";
const SMN_TCTL: u64 = 0x0005_9800;
const STATUS_OK: u32 = 0;
const STATUS_PAWNIO_MISSING: u32 = 1;
const STATUS_UNSUPPORTED_CPU: u32 = 2;
const STATUS_READ_ERROR: u32 = 3;
const MAPPING_SIZE: usize = 24;
const PAWNIO_PATH: &str = r"C:\Program Files\PawnIO\PawnIOLib.dll";
const SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GR;;;AU)";

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SharedBlock {
    magic: u32,
    version: u32,
    status: u32,
    temp_centi_c: i32,
    unix_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reading {
    status: u32,
    temp_centi_c: i32,
    hresult: Option<i32>,
}

#[derive(Debug)]
struct CpuInfo {
    vendor: String,
    family: u32,
    brand: String,
    offset: i32,
}

#[cfg(target_arch = "x86_64")]
fn cpuid(leaf: u32) -> std::arch::x86_64::CpuidResult {
    std::arch::x86_64::__cpuid(leaf)
}
#[cfg(target_arch = "x86")]
fn cpuid(leaf: u32) -> std::arch::x86::CpuidResult {
    unsafe { std::arch::x86::__cpuid(leaf) }
}

fn cpuid_string(words: [u32; 3]) -> String {
    let mut bytes = Vec::with_capacity(12);
    for w in words {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    String::from_utf8_lossy(&bytes)
        .trim_matches('\0')
        .trim()
        .to_string()
}

fn cpu_identity() -> (String, u32, String) {
    let v = cpuid(0);
    let vendor = cpuid_string([v.ebx, v.edx, v.ecx]);
    let base = cpuid(1);
    let base_family = (base.eax >> 8) & 0xf;
    let ext_family = (base.eax >> 20) & 0xff;
    let family = if base_family == 0xf {
        base_family + ext_family
    } else {
        base_family
    };
    let mut brand_bytes = Vec::with_capacity(48);
    for leaf in 0x8000_0002..=0x8000_0004 {
        let r = cpuid(leaf);
        for word in [r.eax, r.ebx, r.ecx, r.edx] {
            brand_bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    let brand = String::from_utf8_lossy(&brand_bytes)
        .trim_matches('\0')
        .trim()
        .to_string();
    (vendor, family, brand)
}

fn is_supported_cpu(vendor: &str, family: u32) -> bool {
    vendor == "AuthenticAMD" && (0x17..=0x1a).contains(&family)
}

fn model_offset(brand: &str) -> i32 {
    let b = brand.to_ascii_uppercase();
    if ["1600X", "1700X", "1800X"].iter().any(|s| b.contains(s)) {
        20
    } else if b.contains("2700X") {
        10
    } else if b.contains("THREADRIPPER 19") || b.contains("THREADRIPPER 29") {
        27
    } else {
        0
    }
}

fn decode_tctl(raw: u64) -> f64 {
    let mut c = (((raw >> 21) & 0x7ff) as f64) * 0.125;
    if raw & (1 << 19) != 0 {
        c -= 49.0;
    }
    c
}

fn serialize_block(status: u32, temp_centi_c: i32, unix_ms: u64) -> [u8; MAPPING_SIZE] {
    let b = SharedBlock {
        magic: 0x504d_5453,
        version: 1,
        status,
        temp_centi_c,
        unix_ms,
    };
    let mut out = [0; MAPPING_SIZE];
    out[0..4].copy_from_slice(&b.magic.to_le_bytes());
    out[4..8].copy_from_slice(&b.version.to_le_bytes());
    out[8..12].copy_from_slice(&b.status.to_le_bytes());
    out[12..16].copy_from_slice(&b.temp_centi_c.to_le_bytes());
    out[16..24].copy_from_slice(&b.unix_ms.to_le_bytes());
    out
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

struct Mapping {
    handle: HANDLE,
    view: *mut u8,
}
impl Mapping {
    fn create() -> Result<Self, String> {
        unsafe {
            let mut sd = null_mut();
            let sddl = wide(SDDL);
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut sd,
                null_mut(),
            ) == 0
            {
                return Err(format!("security descriptor failed: {}", GetLastError()));
            }
            let mut sa = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd,
                bInheritHandle: 0,
            };
            let name = wide(MAP_NAME);
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                &mut sa,
                PAGE_READWRITE,
                0,
                MAPPING_SIZE as u32,
                name.as_ptr(),
            );
            LocalFree(sd as HLOCAL);
            if handle == 0 {
                return Err(format!("CreateFileMappingW failed: {}", GetLastError()));
            }
            let mapped = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, MAPPING_SIZE);
            let view = mapped.Value as *mut u8;
            if view.is_null() {
                CloseHandle(handle);
                return Err(format!("MapViewOfFile failed: {}", GetLastError()));
            }
            Ok(Self { handle, view })
        }
    }
    fn publish(&self, reading: Reading) {
        let bytes = serialize_block(reading.status, reading.temp_centi_c, unix_ms());
        let u32_at = |i| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let i32_at = |i| i32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        unsafe {
            // unix_ms 0 marks the block invalid while the fields change (the widget rejects 0 and
            // a unix_ms that differs between its two reads), so it never pairs a new status with
            // an old temperature
            std::ptr::write_volatile(self.view.add(16) as *mut u64, 0);
            std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
            std::ptr::write_volatile(self.view as *mut u32, u32_at(0));
            std::ptr::write_volatile(self.view.add(4) as *mut u32, u32_at(4));
            std::ptr::write_volatile(self.view.add(8) as *mut u32, u32_at(8));
            std::ptr::write_volatile(self.view.add(12) as *mut i32, i32_at(12));
            // unix_ms is written last as the publication marker consumed by the widget.
            std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
            std::ptr::write_volatile(
                self.view.add(16) as *mut u64,
                u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            );
        }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view as *mut c_void,
            });
            CloseHandle(self.handle);
        }
    }
}

type PawnOpen = unsafe extern "system" fn(*mut HANDLE) -> i32;
type PawnLoad = unsafe extern "system" fn(HANDLE, *const u8, usize) -> i32;
type PawnExecute = unsafe extern "system" fn(
    HANDLE,
    *const i8,
    *const u64,
    usize,
    *mut u64,
    usize,
    *mut usize,
) -> i32;
type PawnClose = unsafe extern "system" fn(HANDLE) -> i32;

struct PawnIo {
    library: isize,
    handle: HANDLE,
    execute: PawnExecute,
    close: PawnClose,
    pci_mutex: HANDLE,
}
impl PawnIo {
    fn open() -> Result<Self, String> {
        unsafe {
            let library = load_pawnio_library()?;
            let open: PawnOpen = symbol(library, b"pawnio_open\0")?;
            let load: PawnLoad = symbol(library, b"pawnio_load\0")?;
            let execute: PawnExecute = symbol(library, b"pawnio_execute\0")?;
            let close: PawnClose = symbol(library, b"pawnio_close\0")?;
            let mut handle = 0;
            // `\Device\PawnIO` exists only while PawnIO's setup-created `Root\PawnIO` device is
            // present (Device Manager, Software devices); not found means a broken PawnIO install.
            let hr = open(&mut handle);
            if hr < 0 {
                FreeLibrary(library);
                return Err(format!("PawnIO open failed (HRESULT 0x{:08X}): 0x80070002 means PawnIO's device is missing (reinstall PawnIO), 0x80070005 means not elevated", hr as u32));
            }
            let blob = include_bytes!("../modules/AMDFamily17.bin");
            let hr = load(handle, blob.as_ptr(), blob.len());
            if hr < 0 {
                close(handle);
                FreeLibrary(library);
                return Err(format!("PawnIO module load failed; this helper needs elevation for PawnIO access (HRESULT 0x{:08X})", hr as u32));
            }
            let pci_name = wide("Global\\Access_PCI");
            let pci_mutex = CreateMutexW(null(), 0, pci_name.as_ptr());
            if pci_mutex == 0 {
                close(handle);
                FreeLibrary(library);
                return Err(format!("Access_PCI mutex failed: {}", GetLastError()));
            }
            Ok(Self {
                library,
                handle,
                execute,
                close,
                pci_mutex,
            })
        }
    }
    fn read_raw(&self) -> Result<u64, i32> {
        unsafe {
            let wait = WaitForSingleObject(self.pci_mutex, 100);
            if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
                return Err(if wait == WAIT_TIMEOUT {
                    0x8007_05B4u32 as i32
                } else {
                    0x8007_0000u32.wrapping_add(GetLastError()) as i32
                });
            }
            let name = b"ioctl_read_smn\0";
            let input = [SMN_TCTL];
            let mut output = [0u64];
            let mut returned = 0usize;
            let hr = (self.execute)(
                self.handle,
                name.as_ptr() as *const i8,
                input.as_ptr(),
                1,
                output.as_mut_ptr(),
                1,
                &mut returned,
            );
            ReleaseMutex(self.pci_mutex);
            if hr < 0 {
                return Err(hr);
            }
            if returned < 1 {
                return Err(0x8000_4005u32 as i32);
            }
            Ok(output[0])
        }
    }
}
impl Drop for PawnIo {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.pci_mutex);
            (self.close)(self.handle);
            FreeLibrary(self.library);
        }
    }
}

unsafe fn symbol<T: Copy>(library: isize, name: &'static [u8]) -> Result<T, String> {
    let proc = GetProcAddress(library, name.as_ptr());
    match proc {
        Some(p) => Ok(transmute_copy(&p)),
        None => Err(format!(
            "PawnIO export {} missing",
            CStr::from_bytes_with_nul_unchecked(name).to_string_lossy()
        )),
    }
}

unsafe fn load_pawnio_library() -> Result<isize, String> {
    let path = PathBuf::from(PAWNIO_PATH);
    let mut wide_path = wide(&path.to_string_lossy());
    let mut lib = LoadLibraryExW(wide_path.as_ptr(), 0, 0);
    if lib == 0 {
        if let Some(location) = registry_install_location() {
            let candidate = PathBuf::from(location).join("PawnIOLib.dll");
            wide_path = wide(&candidate.to_string_lossy());
            lib = LoadLibraryExW(wide_path.as_ptr(), 0, 0);
        }
    }
    if lib == 0 {
        Err(format!(
            "PawnIO missing / LoadLibraryExW failed: {}",
            GetLastError()
        ))
    } else {
        Ok(lib)
    }
}

fn registry_install_location() -> Option<String> {
    unsafe {
        let subkey = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO");
        let value = wide("InstallLocation");
        let mut key: HKEY = 0;
        if RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        ) != 0
        {
            return None;
        }
        let mut ty = 0u32;
        let mut bytes = 0u32;
        if RegQueryValueExW(key, value.as_ptr(), null(), &mut ty, null_mut(), &mut bytes) != 0
            || ty != REG_SZ
            || bytes < 2
        {
            RegCloseKey(key);
            return None;
        }
        let mut data = vec![0u16; (bytes as usize + 1) / 2];
        let rc = RegQueryValueExW(
            key,
            value.as_ptr(),
            null(),
            &mut ty,
            data.as_mut_ptr() as *mut u8,
            &mut bytes,
        );
        RegCloseKey(key);
        if rc != 0 {
            return None;
        }
        Some(String::from_utf16_lossy(
            &data[..data.iter().position(|&c| c == 0).unwrap_or(data.len())],
        ))
    }
}

fn log_line(line: &str) {
    // beside the exe (installed in Program Files: admin-only), never under the user's
    // writable %APPDATA%, where a planted link could make this elevated process overwrite a file
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) else {
        return;
    };
    let path = dir.join("sysmon-temp.log");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > 256 * 1024 {
            if let Ok(bytes) = fs::read(&path) {
                let start = bytes.len().saturating_sub(128 * 1024);
                let tail = &bytes[start..];
                let line_start = tail
                    .iter()
                    .position(|&b| b == b'\n')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let _ = fs::write(&path, &tail[line_start..]);
            }
        }
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let now = unix_ms();
        let _ = writeln!(f, "{} {}", now, line);
    }
}

fn status_name(status: u32) -> &'static str {
    match status {
        0 => "ok",
        1 => "PawnIO missing",
        2 => "unsupported CPU",
        _ => "read error",
    }
}

fn once(cpu: &CpuInfo, supported: bool) -> i32 {
    if !supported {
        println!(
            "status=2 (unsupported CPU), vendor={}, family={}",
            cpu.vendor, cpu.family
        );
        return 1;
    }
    let pawn = match PawnIo::open() {
        Ok(p) => p,
        Err(e) => {
            println!("status=1 (PawnIO missing), {}", e);
            return 1;
        }
    };
    match pawn.read_raw() {
        Ok(raw) => {
            let c = decode_tctl(raw) - cpu.offset as f64;
            println!("status=0, {:.2} °C", c);
            0
        }
        Err(hr) => {
            println!("status=3 (read error), HRESULT 0x{:08X}", hr as u32);
            1
        }
    }
}

fn run_loop(mapping: Mapping, cpu: CpuInfo, supported: bool) {
    let mut pawn: Option<PawnIo> = None;
    let mut last_open = UNIX_EPOCH;
    let mut last_status = u32::MAX;
    let mut last_read_error = None;
    loop {
        let reading = if !supported {
            Reading {
                status: STATUS_UNSUPPORTED_CPU,
                temp_centi_c: 0,
                hresult: None,
            }
        } else {
            if pawn.is_none()
                && SystemTime::now()
                    .duration_since(last_open)
                    .unwrap_or_default()
                    >= Duration::from_secs(60)
            {
                last_open = SystemTime::now();
                match PawnIo::open() {
                    Ok(p) => {
                        pawn = Some(p);
                        last_read_error = None;
                    }
                    Err(e) => {
                        last_read_error = None;
                        log_line(&e);
                    }
                }
            }
            match pawn.as_ref().map(PawnIo::read_raw) {
                Some(Ok(raw)) => {
                    last_read_error = None;
                    let c = decode_tctl(raw) - cpu.offset as f64;
                    Reading {
                        status: STATUS_OK,
                        temp_centi_c: (c * 100.0).round() as i32,
                        hresult: None,
                    }
                }
                Some(Err(hr)) => {
                    last_read_error = Some(hr);
                    log_line(&format!("read HRESULT 0x{:08X}", hr as u32));
                    pawn = None;
                    last_open = SystemTime::now();
                    Reading {
                        status: STATUS_READ_ERROR,
                        temp_centi_c: 0,
                        hresult: Some(hr),
                    }
                }
                None => match last_read_error {
                    Some(hr) => Reading {
                        status: STATUS_READ_ERROR,
                        temp_centi_c: 0,
                        hresult: Some(hr),
                    },
                    None => Reading {
                        status: STATUS_PAWNIO_MISSING,
                        temp_centi_c: 0,
                        hresult: None,
                    },
                },
            }
        };
        if reading.status != last_status {
            log_line(&format!("status {}", status_name(reading.status)));
            last_status = reading.status;
        }
        mapping.publish(reading);
        thread::sleep(
            if reading.status == STATUS_OK || reading.status == STATUS_READ_ERROR {
                Duration::from_secs(1)
            } else {
                Duration::from_secs(5)
            },
        );
    }
}

fn acquire_instance_mutex() -> Option<HANDLE> {
    unsafe {
        let name = wide(INSTANCE_MUTEX);
        let handle = CreateMutexW(null(), 1, name.as_ptr());
        if handle == 0 {
            return None;
        }
        // CreateMutex with initial ownership: ERROR_ALREADY_EXISTS means another instance owns it.
        if GetLastError() == 183 {
            CloseHandle(handle);
            return None;
        }
        Some(handle)
    }
}

fn main() {
    let once_mode = std::env::args().any(|a| a == "--once");
    if once_mode {
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
    let (vendor, family, brand) = cpu_identity();
    let cpu = CpuInfo {
        vendor,
        family,
        offset: model_offset(&brand),
        brand,
    };
    let supported = is_supported_cpu(&cpu.vendor, cpu.family);
    if once_mode {
        std::process::exit(once(&cpu, supported));
    }
    let Some(mutex) = acquire_instance_mutex() else {
        return;
    };
    let mapping = match Mapping::create() {
        Ok(m) => m,
        Err(e) => {
            log_line(&format!("mapping error: {}", e));
            unsafe {
                ReleaseMutex(mutex);
                CloseHandle(mutex);
            }
            return;
        }
    };
    log_line(&format!(
        "start CPU='{}' family={} offset={} supported={}",
        cpu.brand, cpu.family, cpu.offset, supported
    ));
    run_loop(mapping, cpu, supported);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_tctl_with_and_without_range_select() {
        assert!((decode_tctl(400u64 << 21) - 50.0).abs() < f64::EPSILON);
        assert!((decode_tctl((400u64 << 21) | (1 << 19)) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn applies_model_offsets() {
        assert_eq!(model_offset("AMD Ryzen 5 1600X Six-Core"), 20);
        assert_eq!(model_offset("AMD Ryzen 7 1700X"), 20);
        assert_eq!(model_offset("AMD Ryzen 7 1800X"), 20);
        assert_eq!(model_offset("AMD Ryzen 7 2700X"), 10);
        assert_eq!(model_offset("AMD Ryzen Threadripper 1950X"), 27);
        assert_eq!(model_offset("AMD Ryzen Threadripper 2950X"), 27);
        assert_eq!(model_offset("AMD Ryzen 5 5600X"), 0);
    }

    #[test]
    fn serializes_exact_24_byte_contract() {
        let bytes = serialize_block(3, -125, 0x0102_0304_0506_0708);
        assert_eq!(bytes.len(), 24);
        assert_eq!(&bytes[0..4], &[0x53, 0x54, 0x4d, 0x50]);
        assert_eq!(&bytes[4..8], &[1, 0, 0, 0]);
        assert_eq!(&bytes[8..12], &[3, 0, 0, 0]);
        assert_eq!(&bytes[12..16], &(-125i32).to_le_bytes());
        assert_eq!(&bytes[16..24], &0x0102_0304_0506_0708u64.to_le_bytes());
        let block = std::mem::MaybeUninit::<SharedBlock>::uninit();
        let base = block.as_ptr() as usize;
        unsafe {
            assert_eq!(
                std::ptr::addr_of!((*block.as_ptr()).magic) as usize - base,
                0
            );
            assert_eq!(
                std::ptr::addr_of!((*block.as_ptr()).version) as usize - base,
                4
            );
            assert_eq!(
                std::ptr::addr_of!((*block.as_ptr()).status) as usize - base,
                8
            );
            assert_eq!(
                std::ptr::addr_of!((*block.as_ptr()).temp_centi_c) as usize - base,
                12
            );
            assert_eq!(
                std::ptr::addr_of!((*block.as_ptr()).unix_ms) as usize - base,
                16
            );
        }
        assert_eq!(size_of::<SharedBlock>(), 24);
    }

    #[test]
    fn supports_only_amd_families_17_through_1a() {
        for family in 0x17..=0x1a {
            assert!(is_supported_cpu("AuthenticAMD", family));
        }
        assert!(!is_supported_cpu("GenuineIntel", 0x17));
        assert!(!is_supported_cpu("AuthenticAMD", 0x16));
        assert!(!is_supported_cpu("AuthenticAMD", 0x1b));
    }
}
