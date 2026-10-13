use crate::{
    history::{History, Point},
    mem, net, nvml, pdh, temp,
};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::Win32::{
    Foundation::{FILETIME, HWND, LPARAM, WPARAM},
    System::Threading::GetSystemTimes,
    UI::WindowsAndMessaging::PostMessageW,
};
pub const WM_SAMPLE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 8;
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub sampled_at_ms: u64,
    pub cpu: Option<f64>,
    pub gpu: Option<f64>,
    pub ram: Option<f64>,
    pub down: Option<f64>,
    pub up: Option<f64>,
    pub cpu_temp: Option<f64>,
    pub gpu_temp: Option<f64>,
    pub cpu_clock: Option<f64>,
    pub cpu_threads: [Option<f64>; 128],
    pub gpu_clock: Option<f64>,
    pub gpu_power: Option<f64>,
    pub gpu_fan: Option<f64>,
    pub gpu_mem_used: Option<u64>,
    pub gpu_mem_total: Option<u64>,
    pub memory: mem::Memory,
    pub network: net::Network,
    pub cpu_name: Option<Arc<str>>,
    pub gpu_name: Option<Arc<str>>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            sampled_at_ms: 0,
            cpu: None,
            gpu: None,
            ram: None,
            down: None,
            up: None,
            cpu_temp: None,
            gpu_temp: None,
            cpu_clock: None,
            cpu_threads: [None; 128],
            gpu_clock: None,
            gpu_power: None,
            gpu_fan: None,
            gpu_mem_used: None,
            gpu_mem_total: None,
            memory: mem::Memory::default(),
            network: net::Network::default(),
            cpu_name: None,
            gpu_name: None,
        }
    }
}
impl Snapshot {
    pub fn json(&self, speed: &str, paused: bool, idle: bool, len: usize) -> serde_json::Value {
        json!({"sampled_at_ms":self.sampled_at_ms,"cpu_pct":self.cpu,"gpu_pct":self.gpu,"ram_pct":self.ram,"down_bytes_s":self.down,"up_bytes_s":self.up,"cpu_temp_c":self.cpu_temp,"gpu_temp_c":self.gpu_temp,"cpu_clock_ghz":self.cpu_clock,"gpu_clock_ghz":self.gpu_clock,"gpu_power_w":self.gpu_power,"gpu_fan_pct":self.gpu_fan,"gpu_vram_used":self.gpu_mem_used,"gpu_vram_total":self.gpu_mem_total,"memory_used":self.memory.used,"memory_total":self.memory.total,"memory_committed":self.memory.committed,"memory_commit_limit":self.memory.commit_limit,"memory_cached":self.memory.cached,"adapter":self.network.alias,"link_bps":self.network.link,"session_down":self.network.total_down,"session_up":self.network.total_up,"cpu_name":self.cpu_name.as_deref(),"gpu_name":self.gpu_name.as_deref(),"speed":speed,"paused":paused,"idle":idle,"history_len":len})
    }
}
#[derive(Clone)]
pub struct Shared {
    pub data: Arc<Mutex<Data>>,
    control: Arc<(Mutex<Control>, Condvar)>,
    stop: Arc<AtomicBool>,
    reset_epoch: Arc<AtomicU64>,
}
#[derive(Default)]
pub struct Data {
    pub latest: Snapshot,
    pub history: History,
}
#[derive(Clone)]
struct Control {
    speed: String,
    paused: bool,
    idle: bool,
}
pub struct Worker {
    shared: Shared,
    join: Option<JoinHandle<()>>,
}
impl Shared {
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(Data::default())),
            control: Arc::new((
                Mutex::new(Control {
                    speed: "normal".into(),
                    paused: false,
                    idle: false,
                }),
                Condvar::new(),
            )),
            stop: Arc::new(AtomicBool::new(false)),
            reset_epoch: Arc::new(AtomicU64::new(0)),
        }
    }
    pub fn set(&self, speed: String, paused: bool, idle: bool) {
        let (lock, cv) = &*self.control;
        let mut c = lock.lock().unwrap();
        c.speed = speed;
        c.paused = paused;
        c.idle = idle;
        cv.notify_all()
    }
    pub fn snapshot(&self) -> Snapshot {
        self.data.lock().unwrap().latest.clone()
    }
    pub fn clear(&self) {
        let mut d = self.data.lock().unwrap();
        d.history.clear();
        d.latest.network.total_down = 0;
        d.latest.network.total_up = 0;
        self.reset_epoch.fetch_add(1, Ordering::Relaxed);
    }
    pub fn fake_spike(&self) {
        let mut d = self.data.lock().unwrap();
        d.history.push(Point {
            at: Instant::now(),
            interval: Duration::from_secs(1),
            values: [
                Some(99.),
                Some(99.),
                Some(97.),
                Some(1_200_000.),
                Some(80_000.),
            ],
        });
    }
}
impl Worker {
    pub fn start(shared: Shared, hwnd: isize, include_virtual: Arc<AtomicBool>) -> Self {
        let sh = shared.clone();
        let join = thread::spawn(move || run(sh, hwnd, include_virtual));
        Self {
            shared,
            join: Some(join),
        }
    }
    pub fn set(&self, speed: String, paused: bool, idle: bool) {
        self.shared.set(speed, paused, idle)
    }
    pub fn stop(&mut self) {
        {
            // under the condvar's mutex, so a worker between its check and its wait can't miss it
            let _guard = self.shared.control.0.lock().unwrap();
            self.shared.stop.store(true, Ordering::Relaxed);
            self.shared.control.1.notify_all();
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
fn run(shared: Shared, hwnd: isize, include_virtual: Arc<AtomicBool>) {
    let gpu = nvml::Nvml::open();
    if gpu.is_none() {
        crate::log("NVML unavailable; GPU use from Windows' GPU engine counters, other GPU readings unavailable");
    }
    let mut cpu = pdh::Query::open(gpu.is_none());
    if cpu.is_none() {
        crate::log("PDH CPU utility counter unavailable; using GetSystemTimes");
    }
    let cpu_name: Arc<str> = Arc::from(cpu_name());
    let base_mhz = cpu_mhz();
    let gpu_name: Option<Arc<str>> =
        gpu.as_ref().and_then(|g| g.name()).map(|n| Arc::from(crate::format::gpu_name(&n)));
    let mut network = net::Sampler::default();
    let mut last = Instant::now();
    let mut cpu_prev = cpu_times();
    let mut was_active = false;
    let mut resume_sample = true;
    let mut last_signature = None::<([i16; 180], [u64; 5])>;
    let mut reset_seen = 0;
    loop {
        let ctl = {
            let (l, cv) = &*shared.control;
            let c = l.lock().unwrap().clone();
            if c.paused || c.idle {
                was_active = false;
                cpu_prev = cpu_times();
                network.rebase(); // session totals survive a pause or idle
                let guard = l.lock().unwrap();
                let _wait = cv
                    .wait_while(guard, |x| {
                        !shared.stop.load(Ordering::Relaxed)
                            && x.paused == c.paused
                            && x.idle == c.idle
                            && x.speed == c.speed
                    })
                    .unwrap();
                if shared.stop.load(Ordering::Relaxed) {
                    break;
                }
                continue;
            }
            c
        };
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        let interval = match ctl.speed.as_str() {
            "high" => Duration::from_millis(500),
            "low" => Duration::from_secs(4),
            _ => Duration::from_secs(1),
        };
        if !was_active {
            cpu_prev = cpu_times();
            network.rebase();
            last = Instant::now();
            was_active = true;
            resume_sample = true;
            if let Some(q) = cpu.as_mut() {
                q.prime();
            }
        }
        let reset_now = shared.reset_epoch.load(Ordering::Relaxed);
        if reset_now != reset_seen {
            network.reset();
            reset_seen = reset_now;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(last).as_secs_f64().max(0.05);
        last = now;
        let memory = mem::sample();
        let ram = memory
            .total
            .zip(memory.used)
            .map(|(t, u)| u as f64 * 100. / t.max(1) as f64);
        let (mut pdh_cpu, mut cpu_clock, mut cpu_threads, pdh_gpu) = cpu
            .as_mut()
            .map(|q| q.sample(base_mhz))
            .unwrap_or((None, None, [None; 128], None));
        if resume_sample {
            pdh_cpu = None;
            cpu_clock = None;
            cpu_threads = [None; 128];
            resume_sample = false;
        }
        let cpu_pct = pdh_cpu.or_else(|| cpu_delta(&mut cpu_prev));
        let (nvml_util, gm, gt, gp, gc, gf) = gpu.as_ref().map(|g| g.sample()).unwrap_or_default();
        let gu = nvml_util.or_else(|| if gpu.is_none() { pdh_gpu } else { None });
        let network = network.sample(include_virtual.load(Ordering::Relaxed), elapsed);
        let sampled_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let snap = Snapshot {
            sampled_at_ms,
            cpu: cpu_pct,
            gpu: gu,
            ram,
            down: network.down,
            up: network.up,
            cpu_temp: temp::read(),
            gpu_temp: gt,
            cpu_clock,
            cpu_threads,
            gpu_clock: gc,
            gpu_power: gp,
            gpu_fan: gf,
            gpu_mem_used: gm.map(|m| m.used),
            gpu_mem_total: gm.map(|m| m.total),
            memory,
            network,
            cpu_name: Some(cpu_name.clone()),
            gpu_name: gpu_name.clone(),
        };
        let mut pixels = [i16::MIN; 180];
        {
            let mut d = shared.data.lock().unwrap();
            // Reset graphs ran while this sample was being taken: drop it (its totals are pre-reset)
            if shared.reset_epoch.load(Ordering::Relaxed) != reset_now {
                continue;
            }
            d.latest = snap.clone();
            d.history.push(Point {
                at: now,
                interval,
                values: [snap.cpu, snap.gpu, snap.ram, snap.down, snap.up],
            });
            for ch in 0..3 {
                d.history
                    .pixel_signature(60, ch, &mut pixels[ch * 60..(ch + 1) * 60]);
            }
        }
        let rate_sig = |v: Option<f64>| {
            v.filter(|x| x.is_finite() && *x >= 0.)
                .map_or(u64::MAX, |x| {
                    if x < 1_000_000. {
                        (x / 1000.).round() as u64
                    } else {
                        1_000_000 + (x / 100_000.).round() as u64
                    }
                })
        };
        let text = [
            snap.cpu.map_or(u64::MAX, |v| v.round() as u64),
            snap.gpu.map_or(u64::MAX, |v| v.round() as u64),
            snap.ram.map_or(u64::MAX, |v| v.round() as u64),
            rate_sig(snap.down),
            rate_sig(snap.up),
        ];
        let signature = (pixels, text);
        let changed = Some(signature) != last_signature;
        last_signature = Some(signature);
        unsafe {
            let _ = PostMessageW(
                HWND(hwnd as *mut _),
                WM_SAMPLE,
                WPARAM(changed as usize),
                LPARAM(0),
            );
        }
        let spent = now.elapsed();
        let (l, cv) = &*shared.control;
        let guard = l.lock().unwrap();
        if !guard.paused && !guard.idle && !shared.stop.load(Ordering::Relaxed) {
            let _wait = cv
                .wait_timeout_while(guard, interval.saturating_sub(spent), |x| {
                    !x.paused
                        && !x.idle
                        && x.speed == ctl.speed
                        && !shared.stop.load(Ordering::Relaxed)
                })
                .unwrap();
        }
    }
}
#[derive(Default)]
struct CpuTimes {
    idle: u64,
    kernel: u64,
    user: u64,
}
fn cpu_times() -> CpuTimes {
    unsafe {
        let (mut i, mut k, mut u) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)).is_ok() {
            CpuTimes {
                idle: ft(i),
                kernel: ft(k),
                user: ft(u),
            }
        } else {
            CpuTimes::default()
        }
    }
}
fn ft(x: FILETIME) -> u64 {
    ((x.dwHighDateTime as u64) << 32) | x.dwLowDateTime as u64
}
fn cpu_delta(prev: &mut CpuTimes) -> Option<f64> {
    let n = cpu_times();
    let total = n.kernel.saturating_sub(prev.kernel) + n.user.saturating_sub(prev.user);
    let idle = n.idle.saturating_sub(prev.idle);
    *prev = n;
    (total > 0).then_some((total.saturating_sub(idle) as f64 * 100. / total as f64).clamp(0., 100.))
}
fn cpu_name() -> String {
    unsafe {
        use windows::{core::PCWSTR, Win32::Foundation::WIN32_ERROR, Win32::System::Registry::*};
        let path: Vec<u16> = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let name: Vec<u16> = "ProcessorNameString"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut size = 0u32;
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        ) != WIN32_ERROR(0)
            || size < 2
        {
            return "CPU".into();
        }
        let mut b = vec![0u16; (size as usize / 2) + 1];
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(b.as_mut_ptr() as *mut _),
            Some(&mut size),
        ) != WIN32_ERROR(0)
        {
            return "CPU".into();
        }
        crate::format::cpu_name(&String::from_utf16_lossy(
            &b[..b.iter().position(|x| *x == 0).unwrap_or(b.len())],
        ))
    }
}
fn cpu_mhz() -> Option<f64> {
    unsafe {
        use windows::{core::PCWSTR, Win32::Foundation::WIN32_ERROR, Win32::System::Registry::*};
        let path: Vec<u16> = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let name: Vec<u16> = "~MHz".encode_utf16().chain(Some(0)).collect();
        let mut mhz = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        (RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_DWORD,
            None,
            Some(&mut mhz as *mut _ as *mut _),
            Some(&mut size),
        ) == WIN32_ERROR(0))
        .then_some(mhz as f64)
    }
}

#[cfg(test)]
mod live_source_tests {
    use super::*;
    #[test]
    #[ignore]
    fn live_sources() {
        let mut cpu = pdh::Query::open(true).expect("open PDH query");
        let base_mhz = cpu_mhz();
        let gpu = nvml::Nvml::open().expect("open NVML (RTX 3070 expected)");
        let mut network = net::Sampler::default();
        for index in 0..2 {
            if index != 0 {
                std::thread::sleep(Duration::from_secs(1));
            }
            let (cpu_pct, cpu_clock, threads, fallback_gpu) = cpu.sample(base_mhz);
            let live_threads: Vec<f64> = threads.iter().flatten().copied().collect();
            let (gpu_pct, gpu_mem, gpu_temp, gpu_power, gpu_clock, gpu_fan) = gpu.sample();
            let net = network.sample(false, if index == 0 { 1.0 } else { 1.0 });
            println!("live_sources sample {index}: ~MHz={base_mhz:?}, cpu_pct={cpu_pct:?}, cpu_clock_ghz={cpu_clock:?}, cpu_threads_pct={live_threads:?}, pdh_gpu={fallback_gpu:?}, gpu_name={:?}, gpu_pct={gpu_pct:?}, gpu_mem={gpu_mem:?}, gpu_temp_c={gpu_temp:?}, gpu_power_w={gpu_power:?}, gpu_clock_ghz={gpu_clock:?}, gpu_fan_pct={gpu_fan:?}, net={net:?}", gpu.name());
            assert!(gpu_power.is_some(), "RTX 3070 NVML power reading missing");
            assert!(cpu_clock.is_some(), "CPU clock needs both PDH performance and registry ~MHz");
        }
    }
}
