use std::collections::HashMap;
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};

#[derive(Clone, Default, Debug)]
pub struct Network {
    pub down: Option<f64>,
    pub up: Option<f64>,
    pub alias: Option<String>,
    pub link: Option<u64>,
    pub total_down: u64,
    pub total_up: u64,
}
#[derive(Clone, Copy, Debug)]
struct Counters {
    rx: u64,
    tx: u64,
    link: u64,
}
#[derive(Clone, Copy, Debug)]
struct RowKind {
    hardware: bool,
    filter: bool,
    oper_up: bool,
    interface_type: u32,
}
fn count_row(row: RowKind, include_virtual: bool) -> bool {
    !row.filter
        && row.oper_up
        && row.interface_type != 24
        && row.interface_type != 131
        && (row.hardware || include_virtual)
}
#[derive(Default)]
pub struct Sampler {
    last: HashMap<u64, Counters>,
    current: HashMap<u64, Counters>,
    pub total_down: u64,
    pub total_up: u64,
    logged_error: bool,
}

fn delta(old: Option<u64>, new: u64, max: u64) -> u64 {
    old.and_then(|v| new.checked_sub(v))
        .filter(|v| *v <= max)
        .unwrap_or(0)
}

impl Sampler {
    /// Reset graphs: a new baseline and the session totals back to zero.
    pub fn reset(&mut self) {
        self.rebase();
        self.total_down = 0;
        self.total_up = 0;
    }
    /// After a pause or idle: a new baseline (no spike for the time away), totals kept.
    pub fn rebase(&mut self) {
        self.last.clear();
    }
    pub fn sample(&mut self, include_virtual: bool, elapsed: f64) -> Network {
        unsafe {
            let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
            if GetIfTable2(&mut table) != windows::Win32::Foundation::WIN32_ERROR(0)
                || table.is_null()
            {
                self.last.clear();
                if !self.logged_error {
                    crate::log("GetIfTable2 network counters unavailable");
                    self.logged_error = true;
                }
                return Network::default();
            }
            let rows =
                std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
            self.current.clear();
            let mut down = 0u64;
            let mut up = 0u64;
            let mut busiest: Option<(u64, String, u64)> = None;
            let max = (elapsed.max(0.05) * 12_500_000_000.0) as u64;
            for row in rows {
                let kind = RowKind {
                    hardware: row.InterfaceAndOperStatusFlags._bitfield & 1 != 0,
                    filter: row.InterfaceAndOperStatusFlags._bitfield & 2 != 0,
                    oper_up: row.OperStatus.0 == 1,
                    interface_type: row.Type,
                };
                if !count_row(kind, include_virtual) {
                    continue;
                }
                let alias = String::from_utf16_lossy(&row.Alias)
                    .trim_end_matches('\0')
                    .to_owned();
                if alias.is_empty() {
                    continue;
                }
                let key = row.InterfaceLuid.Value;
                let now = Counters {
                    rx: row.InOctets,
                    tx: row.OutOctets,
                    link: row.ReceiveLinkSpeed,
                };
                self.current.insert(key, now);
                let rx = delta(self.last.get(&key).map(|v| v.rx), now.rx, max);
                let tx = delta(self.last.get(&key).map(|v| v.tx), now.tx, max);
                down = down.saturating_add(rx);
                up = up.saturating_add(tx);
                let busy = rx.saturating_add(tx);
                if busiest.as_ref().is_none_or(|v| busy > v.0) {
                    busiest = Some((busy, alias, now.link));
                }
            }
            FreeMibTable(table as *const _);
            std::mem::swap(&mut self.last, &mut self.current);
            self.total_down = self.total_down.saturating_add(down);
            self.total_up = self.total_up.saturating_add(up);
            Network {
                down: Some(down as f64 / elapsed.max(0.05)),
                up: Some(up as f64 / elapsed.max(0.05)),
                alias: busiest.as_ref().map(|v| v.1.clone()),
                link: busiest.map(|v| v.2),
                total_down: self.total_down,
                total_up: self.total_up,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counter_reset_never_wraps_or_spikes() {
        assert_eq!(delta(Some(100), 10, u64::MAX), 0);
        assert_eq!(delta(Some(10), 9_000_000_000, 1_000), 0);
    }
    #[test]
    fn counted_rows_follow_hardware_filter_virtual_type_and_status_rules() {
        let row = |hardware, filter, oper_up, interface_type| RowKind {
            hardware,
            filter,
            oper_up,
            interface_type,
        };
        assert!(count_row(row(true, false, true, 6), false));
        assert!(!count_row(row(true, true, true, 6), true));
        assert!(!count_row(row(false, false, true, 6), false));
        assert!(count_row(row(false, false, true, 6), true));
        assert!(!count_row(row(true, false, true, 24), true));
        assert!(!count_row(row(true, false, true, 131), true));
        assert!(!count_row(row(true, false, false, 6), true));
    }
    #[test]
    fn removed_adapter_contributes_nothing() {
        let mut old = HashMap::new();
        old.insert(
            1,
            Counters {
                rx: 100,
                tx: 100,
                link: 1,
            },
        );
        old.insert(
            2,
            Counters {
                rx: 200,
                tx: 200,
                link: 2,
            },
        );
        let now = HashMap::from([(
            1,
            Counters {
                rx: 120,
                tx: 130,
                link: 1,
            },
        )]);
        let totals: (u64, u64) = now
            .iter()
            .map(|(id, c)| {
                (
                    delta(old.get(id).map(|v| v.rx), c.rx, 1000),
                    delta(old.get(id).map(|v| v.tx), c.tx, 1000),
                )
            })
            .fold((0, 0), |(a, b), (x, y)| (a + x, b + y));
        assert_eq!(totals, (20, 30));
    }
}
