use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
#[derive(Clone, Copy, Default, Debug)]
pub struct Memory {
    pub used: Option<u64>,
    pub total: Option<u64>,
    pub committed: Option<u64>,
    pub commit_limit: Option<u64>,
    pub cached: Option<u64>,
}
pub fn sample() -> Memory {
    unsafe {
        let mut m = MEMORYSTATUSEX::default();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let ok = GlobalMemoryStatusEx(&mut m).is_ok();
        let mut p = PERFORMANCE_INFORMATION::default();
        p.cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
        let pok = GetPerformanceInfo(&mut p, p.cb).is_ok();
        Memory {
            used: ok.then_some(m.ullTotalPhys - m.ullAvailPhys),
            total: ok.then_some(m.ullTotalPhys),
            committed: pok.then_some(p.CommitTotal as u64 * p.PageSize as u64),
            commit_limit: pok.then_some(p.CommitLimit as u64 * p.PageSize as u64),
            cached: pok.then_some(p.SystemCache as u64 * p.PageSize as u64),
        }
    }
}
