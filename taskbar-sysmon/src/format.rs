pub fn rate(bytes: Option<f64>) -> String {
    let Some(v) = bytes.filter(|v| v.is_finite() && *v >= 0.0) else {
        return "—".into();
    };
    if v < 1_000_000.0 {
        format!("{:.0} KB/s", v / 1_000.0)
    } else {
        format!("{:.1} MB/s", v / 1_000_000.0)
    }
}
pub fn compact_rate(bytes: Option<f64>) -> String {
    let Some(v) = bytes.filter(|v| v.is_finite() && *v >= 0.0) else {
        return "—".into();
    };
    if v < 1_000_000.0 {
        format!("{:.0}K", v / 1_000.0)
    } else {
        format!("{:.1}M", v / 1_000_000.0)
    }
}
pub fn pct(v: Option<f64>) -> String {
    v.map(|x| format!("{:.0}%", x.clamp(0.0, 100.0)))
        .unwrap_or_else(|| "—".into())
}
pub fn gb(v: Option<u64>) -> String {
    v.map(|x| format!("{:.1} GB", x as f64 / 1_073_741_824.0))
        .unwrap_or_else(|| "—".into())
}
pub fn bytes(v: u64) -> String {
    if v < 1_000_000 {
        format!("{:.0} KB", v as f64 / 1_000.0)
    } else if v < 1_000_000_000 {
        format!("{:.1} MB", v as f64 / 1_000_000.0)
    } else {
        format!("{:.2} GB", v as f64 / 1_000_000_000.0)
    }
}
pub fn level(value: Option<f64>, warn: f64, crit: f64) -> u8 {
    match value {
        Some(v) if v > crit => 2,
        Some(v) if v > warn => 1,
        _ => 0,
    }
}
pub fn cpu_name(s: &str) -> String {
    let s = s.trim();
    let s = if let Some((prefix, suffix)) = s.rsplit_once(' ') {
        if suffix == "Processor" {
            if let Some((name, cores)) = prefix.rsplit_once(' ') {
                if cores.ends_with("-Core") && cores[..cores.len() - 5].parse::<u32>().is_ok() {
                    name
                } else {
                    prefix
                }
            } else {
                prefix
            }
        } else {
            s
        }
    } else {
        s
    };
    s.strip_prefix("AMD ").unwrap_or(s).to_string()
}
/// "NVIDIA GeForce RTX 3070" → "RTX 3070" (the panel card's title has little room).
pub fn gpu_name(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("NVIDIA ").unwrap_or(s);
    let s = s.strip_prefix("GeForce ").unwrap_or(s);
    let s = s.strip_prefix("AMD ").unwrap_or(s);
    s.strip_prefix("Radeon ").filter(|r| r.starts_with("RX")).unwrap_or(s).to_string()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rates_use_kb_until_one_mb() {
        assert_eq!(rate(Some(999_999.0)), "1000 KB/s");
        assert_eq!(rate(Some(1_000_000.0)), "1.0 MB/s");
        assert_eq!(compact_rate(Some(1_200_000.0)), "1.2M");
    }
    #[test]
    fn thresholds_and_cpu_names() {
        assert_eq!(level(Some(80.1), 80.0, 95.0), 1);
        assert_eq!(level(Some(95.1), 80.0, 95.0), 2);
        assert_eq!(
            cpu_name("AMD Ryzen 5 5600X 6-Core Processor"),
            "Ryzen 5 5600X"
        );
        assert_eq!(gpu_name("NVIDIA GeForce RTX 3070"), "RTX 3070");
        assert_eq!(gpu_name("AMD Radeon RX 7800 XT"), "RX 7800 XT");
        assert_eq!(gpu_name("Intel(R) Arc(TM) A770"), "Intel(R) Arc(TM) A770");
    }
}
