//! One-line failures for CLIs Claude runs (dockctl, the launcher, sysmon-temp keeps a copy).
//!
//! stderr gets `ERROR <tool> <CODE>: <what> | fix: <hint> | log: <path>` and the exit is non-zero
//! (2 for BAD_ARG); the command line and detail go to a log under `C:\Tools\logs\<tool>\` (last 50
//! kept). Warnings: `WARN <tool> <CODE>: <what> | fix: <hint>`. Codes: NOT_FOUND, LOCKED, BAD_ARG,
//! BAD_INPUT, TOOL_FAILED, TIMEOUT, INTERRUPTED, INTERNAL, plus tool-specific ones.
//!
//! Std only, so a crate outside the shared library can copy this file as is.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const KEEP: usize = 50;
const MAX_LOG: usize = 200_000;
const TAIL: usize = 30;

/// A failure with a code and a fix hint; `detail` goes to the log only.
#[derive(Debug, Clone)]
pub struct Fail {
    pub code: &'static str,
    pub what: String,
    pub fix: String,
    pub detail: String,
}

impl Fail {
    pub fn new(code: &'static str, what: impl Into<String>, fix: impl Into<String>) -> Self {
        Fail { code, what: what.into(), fix: fix.into(), detail: String::new() }
    }

    /// Adds text for the log's `== detail ==` section.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// TOOL_FAILED for an external program: quotes one stderr line, logs the exit code and the last
    /// 30 lines of stderr.
    pub fn tool(name: &str, exit: Option<i32>, stderr: &str, fix: impl Into<String>) -> Self {
        let exit = exit.map_or("killed".to_string(), |c| c.to_string());
        let lines: Vec<&str> = stderr.lines().collect();
        let tail = lines[lines.len().saturating_sub(TAIL)..].join("\n");
        Fail::new("TOOL_FAILED", format!("{name} exit {exit}: {}", relevant_line(stderr)), fix)
            .detail(format!("exit {exit}\n== tool stderr (last {TAIL}) ==\n{tail}"))
    }
}

/// The log folder root: `CLIERR_LOG_ROOT` when set (tests), else `C:\Tools\logs`.
pub fn log_root() -> PathBuf {
    std::env::var_os("CLIERR_LOG_ROOT").map_or_else(|| PathBuf::from(r"C:\Tools\logs"), PathBuf::from)
}

/// The first line mentioning error/fatal/failed/exception, else the last non-empty one.
pub fn relevant_line(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let hit = lines.iter().find(|l| {
        let l = l.to_ascii_lowercase();
        ["error", "fatal", "failed", "exception"].iter().any(|w| l.contains(w))
    });
    let line = hit.or(lines.last()).copied().unwrap_or("(no output)");
    line.chars().take(300).collect()
}

pub fn warn(tool: &str, code: &str, what: &str, fix: &str) {
    eprintln!("WARN {tool} {code}: {what} | fix: {fix}");
}

/// The ERROR line for a failure whose log is at `log`.
pub fn line(tool: &str, fail: &Fail, log: &str) -> String {
    let one = |s: &str| s.split(['\r', '\n']).map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ");
    format!("ERROR {tool} {}: {} | fix: {} | log: {log}", fail.code, one(&fail.what), one(&fail.fix))
}

/// Writes one log under `root\<tool>` and keeps the newest 50; returns its path, or a note when the
/// log can't be written so the ERROR line still prints.
pub fn write_log(root: &Path, tool: &str, fail: &Fail) -> String {
    let folder = root.join(tool);
    let base = format!("{}-{}", stamp(SystemTime::now()), std::process::id());
    let command: Vec<String> = std::env::args().collect();
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let mut text = format!(
        "== command ==\n{}\ncwd {cwd}\n\n== error ==\n{}: {}\nfix: {}\n\n",
        command.join(" "), fail.code, fail.what, fail.fix
    );
    if !fail.detail.is_empty() {
        text.push_str(&format!("== detail ==\n{}\n\n", fail.detail.trim_end()));
    }
    if text.len() > MAX_LOG {
        let cut = |i: usize| (i..text.len()).find(|&j| text.is_char_boundary(j)).unwrap_or(text.len());
        let (head, tail) = (cut(MAX_LOG / 2), cut(text.len() - MAX_LOG / 2));
        text = format!("{}\n... {} bytes cut ...\n{}", &text[..head], tail - head, &text[tail..]);
    }
    if let Err(e) = std::fs::create_dir_all(&folder) {
        return format!("(log not written: {e})");
    }
    // Exclusive create: two failures in one second keep both logs.
    let mut path = folder.join(format!("{base}.log"));
    for n in 2.. {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(text.as_bytes()) {
                    return format!("(log not written: {e})");
                }
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 100 => {
                path = folder.join(format!("{base}-{n}.log"));
            }
            Err(e) => return format!("(log not written: {e})"),
        }
    }
    prune(&folder);
    path.display().to_string()
}

fn prune(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "log"))
        .collect();
    logs.sort();
    for old in &logs[..logs.len().saturating_sub(KEEP)] {
        let _ = std::fs::remove_file(old);
    }
}

/// Prints the ERROR line, writes the log and exits 1 (2 for BAD_ARG).
pub fn exit(tool: &str, fail: Fail) -> ! {
    let log = write_log(&log_root(), tool, &fail);
    eprintln!("{}", line(tool, &fail, &log));
    std::process::exit(if fail.code == "BAD_ARG" { 2 } else { 1 })
}

/// Makes a panic end as one INTERNAL line, with the panic message and backtrace in the log.
pub fn install_panic_hook(tool: &'static str) {
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into());
        let at = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        let fail = Fail::new("INTERNAL", format!("panic: {message}{at}"), "a bug: read the log's detail")
            .detail(std::backtrace::Backtrace::force_capture().to_string());
        exit(tool, fail)
    }));
}

/// `YYYYMMDD-HHMMSS` in UTC (names sort by time; std has no local time zone).
fn stamp(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil-from-days (H. Hinnant): days since 1970-01-01 to year/month/day.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", rest / 3600, rest / 60 % 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir() -> PathBuf {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("clierr-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn stamp_formats_utc() {
        assert_eq!(stamp(UNIX_EPOCH), "19700101-000000");
        assert_eq!(stamp(UNIX_EPOCH + Duration::from_secs(1_772_581_567)), "20260303-234607");
    }

    #[test]
    fn relevant_line_prefers_error_lines() {
        assert_eq!(relevant_line("noise\r\nfatal: disk full\r\nmore\r\n"), "fatal: disk full");
        assert_eq!(relevant_line("a\nlast\n\n"), "last");
        assert_eq!(relevant_line(""), "(no output)");
    }

    #[test]
    fn writes_log_with_sections_and_keeps_50() {
        let root = temp_dir();
        let folder = root.join("demo");
        std::fs::create_dir_all(&folder).unwrap();
        for i in 0..60 {
            std::fs::write(folder.join(format!("20000101-0000{i:02}-1.log")), "old").unwrap();
        }
        let fail = Fail::tool("helper", Some(3), "noise\nfatal: disk full\n", "retry");
        let log = write_log(&root, "demo", &fail);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("== command ==") && text.contains("== tool stderr (last 30) =="));
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 50);
        let line = line("demo", &fail, &log);
        assert!(line.starts_with("ERROR demo TOOL_FAILED: helper exit 3: fatal: disk full | fix: retry | log: "));
        let again = write_log(&root, "demo", &fail);
        assert_ne!(again, log, "a second failure in the same second gets its own log");
        let multi = Fail::new("X", "two\r\nlines", "fix\nthis");
        assert_eq!(super::line("demo", &multi, "p"), "ERROR demo X: two lines | fix: fix this | log: p");
        let _ = std::fs::remove_dir_all(root);
    }
}
