use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_log_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("dockctl-clierr-{}-{nonce}", std::process::id()))
}

fn run_dockctl(root: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dockctl"))
        .args(args)
        .env("CLIERR_LOG_ROOT", root)
        .output()
        .unwrap()
}

fn assert_bad_arg(root: &PathBuf, args: &[&str]) {
    let output = run_dockctl(root, args);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());

    let stderr = String::from_utf8(output.stderr).unwrap();
    let lines: Vec<_> = stderr.lines().collect();
    assert_eq!(lines.len(), 1, "stderr: {stderr:?}");
    assert!(
        lines[0].starts_with("ERROR dockctl BAD_ARG: "),
        "{stderr:?}"
    );
    let log = lines[0]
        .split_once(" | log: ")
        .expect("ERROR line includes log path")
        .1;
    assert!(PathBuf::from(log).is_file(), "missing log: {log}");
}

#[test]
fn usage_errors_are_one_line_and_logged_before_widget_lookup() {
    let root = temp_log_root();
    assert_bad_arg(&root, &[]);
    assert_bad_arg(&root, &["weather", "definitely-not-a-command"]);
    let _ = std::fs::remove_dir_all(root);
}
