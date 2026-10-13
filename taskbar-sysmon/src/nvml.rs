use std::{
    ffi::{c_void, CString},
    mem,
};
#[repr(C)]
#[derive(Default)]
struct Util {
    gpu: u32,
    memory: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Mem {
    pub total: u64,
    pub free: u64,
    pub used: u64,
}
type H = *mut c_void;
type Dev = *mut c_void;
type FanFn = unsafe extern "C" fn(Dev, *mut u32) -> i32;
#[allow(non_camel_case_types)]
type Init = unsafe extern "C" fn() -> i32;
type Count = unsafe extern "C" fn(*mut u32) -> i32;
type Handle = unsafe extern "C" fn(u32, *mut Dev) -> i32;
type UtilFn = unsafe extern "C" fn(Dev, *mut Util) -> i32;
type MemFn = unsafe extern "C" fn(Dev, *mut Mem) -> i32;
type TempFn = unsafe extern "C" fn(Dev, u32, *mut u32) -> i32;
type PowerFn = unsafe extern "C" fn(Dev, *mut u32) -> i32;
type U32Fn = unsafe extern "C" fn(Dev, u32, *mut u32) -> i32;
type NameFn = unsafe extern "C" fn(Dev, *mut i8, u32) -> i32;
pub struct Nvml {
    _dll: H,
    dev: Dev,
    util: UtilFn,
    mem: MemFn,
    temp: TempFn,
    power: PowerFn,
    clock: U32Fn,
    fan: FanFn,
    name: NameFn,
}
impl Nvml {
    pub fn open() -> Option<Self> {
        unsafe {
            use windows::core::PCWSTR;
            use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
            let name: Vec<u16> = "nvml.dll".encode_utf16().chain(Some(0)).collect();
            let dll = LoadLibraryW(PCWSTR(name.as_ptr())).ok()?;
            macro_rules! sym {
                ($n:literal,$t:ty) => {{
                    let c = CString::new($n).ok()?;
                    mem::transmute::<_, $t>(GetProcAddress(
                        dll,
                        windows::core::PCSTR(c.as_ptr() as _),
                    )?)
                }};
            }
            let init: Init = sym!("nvmlInit_v2", Init);
            if init() != 0 {
                return None;
            }
            let count: Count = sym!("nvmlDeviceGetCount_v2", Count);
            let mut n = 0;
            if count(&mut n) != 0 || n == 0 {
                return None;
            }
            let handle: Handle = sym!("nvmlDeviceGetHandleByIndex_v2", Handle);
            let mut dev = std::ptr::null_mut();
            if handle(0, &mut dev) != 0 {
                return None;
            }
            Some(Self {
                _dll: dll.0,
                dev,
                util: sym!("nvmlDeviceGetUtilizationRates", UtilFn),
                mem: sym!("nvmlDeviceGetMemoryInfo", MemFn),
                temp: sym!("nvmlDeviceGetTemperature", TempFn),
                power: sym!("nvmlDeviceGetPowerUsage", PowerFn),
                clock: sym!("nvmlDeviceGetClockInfo", U32Fn),
                fan: sym!("nvmlDeviceGetFanSpeed", FanFn),
                name: sym!("nvmlDeviceGetName", NameFn),
            })
        }
    }
    pub fn name(&self) -> Option<String> {
        unsafe {
            let mut buf = [0i8; 96];
            if (self.name)(self.dev, buf.as_mut_ptr(), buf.len() as u32) != 0 {
                return None;
            }
            let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
            Some(
                String::from_utf8_lossy(std::slice::from_raw_parts(buf.as_ptr() as *const u8, end))
                    .trim()
                    .to_owned(),
            )
        }
    }
    pub fn sample(
        &self,
    ) -> (
        Option<f64>,
        Option<Mem>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    ) {
        unsafe {
            let mut u = Util::default();
            let mut m = Mem::default();
            let (mut t, mut p, mut c, mut f) = (0, 0, 0, 0);
            let util = (self.util)(self.dev, &mut u).eq(&0).then_some(u.gpu as f64);
            let mem = (self.mem)(self.dev, &mut m).eq(&0).then_some(m);
            let temp = (self.temp)(self.dev, 0, &mut t).eq(&0).then_some(t as f64);
            let power = (self.power)(self.dev, &mut p)
                .eq(&0)
                .then_some(p as f64 / 1000.);
            let clock = (self.clock)(self.dev, 0, &mut c)
                .eq(&0)
                .then_some(c as f64 / 1000.);
            let fan = (self.fan)(self.dev, &mut f).eq(&0).then_some(f as f64);
            (util, mem, temp, power, clock, fan)
        }
    }
}
