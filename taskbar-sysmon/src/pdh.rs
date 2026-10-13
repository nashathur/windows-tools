type H = isize;
const THREADS: usize = 128;
#[repr(C)]
struct Value {
    status: u32,
    value: f64,
}
#[link(name = "pdh")]
unsafe extern "system" {
    fn PdhOpenQueryW(src: *const u16, data: usize, q: *mut H) -> u32;
    fn PdhAddEnglishCounterW(q: H, path: *const u16, user: usize, c: *mut H) -> u32;
    fn PdhCollectQueryData(q: H) -> u32;
    fn PdhGetFormattedCounterValue(c: H, fmt: u32, ty: *mut u32, v: *mut Value) -> u32;
    fn PdhGetFormattedCounterArrayW(
        c: H,
        fmt: u32,
        size: *mut u32,
        count: *mut u32,
        items: *mut FormattedItem,
    ) -> u32;
    fn PdhCloseQuery(q: H) -> u32;
}
#[repr(C)]
struct FormattedItem {
    name: *mut u16,
    value: Value,
}
unsafe fn add(q: H, path: &str) -> Option<H> {
    let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut c = 0;
    (PdhAddEnglishCounterW(q, path.as_ptr(), 0, &mut c) == 0).then_some(c)
}
/// PDH_FMT_DOUBLE | PDH_FMT_NOCAP100: `% Processor Performance` passes 100 under boost.
const FMT_DOUBLE_NOCAP: u32 = 0x200 | 0x8000;
unsafe fn read(c: H) -> Option<f64> {
    let mut v = Value {
        status: 0,
        value: 0.,
    };
    if PdhGetFormattedCounterValue(c, FMT_DOUBLE_NOCAP, std::ptr::null_mut(), &mut v) == 0 && v.status == 0 {
        Some(v.value)
    } else {
        None
    }
}
pub struct Query {
    q: H,
    cpu: H,
    perf: Option<H>,
    threads: Vec<H>,
    gpu3d: Option<H>,
}
impl Query {
    /// `gpu_fallback`: also query every GPU engine (costly; only when NVML is missing).
    pub fn open(gpu_fallback: bool) -> Option<Self> {
        unsafe {
            let mut q = 0;
            if PdhOpenQueryW(std::ptr::null(), 0, &mut q) != 0 {
                return None;
            }
            let Some(cpu) = add(q, r"\Processor Information(_Total)\% Processor Utility") else {
                PdhCloseQuery(q);
                return None;
            };
            let perf = add(q, r"\Processor Information(_Total)\% Processor Performance");
            let gpu3d = if gpu_fallback {
                add(q, r"\GPU Engine(*)\Utilization Percentage")
            } else {
                None
            };
            let count = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .min(THREADS);
            let mut threads = Vec::with_capacity(count);
            for i in 0..count {
                if let Some(c) = add(
                    q,
                    &format!(r"\Processor Information(0,{i})\% Processor Utility"),
                ) {
                    threads.push(c)
                }
            }
            PdhCollectQueryData(q);
            Some(Self {
                q,
                cpu,
                perf,
                threads,
                gpu3d,
            })
        }
    }
    pub fn sample(
        &mut self,
        base_mhz: Option<f64>,
    ) -> (
        Option<f64>,
        Option<f64>,
        [Option<f64>; THREADS],
        Option<f64>,
    ) {
        unsafe {
            if PdhCollectQueryData(self.q) != 0 {
                return (None, None, [None; THREADS], None);
            }
            let cpu = read(self.cpu).map(|v| v.clamp(0., 100.));
            let clock = self
                .perf
                .and_then(|counter| read(counter))
                .zip(base_mhz)
                .map(|(p, mhz)| p.max(0.) * mhz / 100_000.);
            let mut threads = [None; THREADS];
            for (i, c) in self.threads.iter().enumerate() {
                threads[i] = read(*c).map(|v| v.clamp(0., 100.));
            }
            (
                cpu,
                clock,
                threads,
                self.gpu3d.and_then(|counter| read_gpu3d(counter)),
            )
        }
    }
    pub fn prime(&mut self) {
        unsafe {
            PdhCollectQueryData(self.q);
        }
    }
}
unsafe fn read_gpu3d(counter: H) -> Option<f64> {
    let mut bytes = 0u32;
    let mut count = 0u32;
    let status =
        PdhGetFormattedCounterArrayW(counter, 0x200, &mut bytes, &mut count, std::ptr::null_mut());
    if status != 0x800007D2 || count == 0 {
        return None;
    }
    let mut storage = vec![0u64; (bytes as usize).div_ceil(std::mem::size_of::<u64>())];
    let status = PdhGetFormattedCounterArrayW(
        counter,
        0x200,
        &mut bytes,
        &mut count,
        storage.as_mut_ptr() as *mut FormattedItem,
    );
    if status != 0 {
        return None;
    }
    let items =
        std::slice::from_raw_parts(storage.as_ptr() as *const FormattedItem, count as usize);
    let pairs = items.iter().filter_map(|item| {
        if item.name.is_null() || item.value.status != 0 {
            return None;
        }
        let mut n = 0usize;
        while *item.name.add(n) != 0 {
            n += 1;
        }
        let name = String::from_utf16_lossy(std::slice::from_raw_parts(item.name, n));
        Some((name, item.value.value))
    });
    busiest_gpu_adapter(pairs)
}
fn gpu_adapter(name: &str) -> Option<String> {
    let start = name.find("luid_")?;
    let tail = &name[start..];
    let phys = tail.find("_phys_")? + "_phys_".len();
    let end = tail[phys..]
        .find('_')
        .map(|i| phys + i)
        .unwrap_or(tail.len());
    (end > phys).then(|| tail[..end].to_owned())
}
fn busiest_gpu_adapter(values: impl IntoIterator<Item = (String, f64)>) -> Option<f64> {
    let mut totals = std::collections::HashMap::<String, f64>::new();
    for (name, value) in values {
        // only the 3D engines (video decode, copy … are other work and would add up past 100)
        if !name.contains("engtype_3D") {
            continue;
        }
        if let Some(adapter) = gpu_adapter(&name) {
            *totals.entry(adapter).or_default() += value.max(0.0);
        }
    }
    totals
        .into_values()
        .max_by(f64::total_cmp)
        .map(|v| v.clamp(0.0, 100.0))
}
impl Drop for Query {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.q);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn groups_gpu_engine_instances_by_adapter_and_sums() {
        // instance names as Windows gives them
        let v = vec![
            ("pid_1_luid_0x00000000_0x0000D1A2_phys_0_eng_0_engtype_3D".to_string(), 24.0),
            ("pid_2_luid_0x00000000_0x0000D1A2_phys_0_eng_0_engtype_3D".to_string(), 31.0),
            ("pid_2_luid_0x00000000_0x0000D1A2_phys_0_eng_3_engtype_VideoDecode".to_string(), 60.0),
            ("pid_3_luid_0x00000000_0x0000E3B4_phys_1_eng_0_engtype_3D".to_string(), 40.0),
        ];
        assert_eq!(busiest_gpu_adapter(v), Some(55.0), "3D engines only, summed per adapter");
    }
}
