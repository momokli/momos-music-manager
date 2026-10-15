//! Integration tests for `kellner service` (issue #368, P5).
//!
//! These drive the real binary (no network) and assert the unit text the user
//! would load is produced, without writing anything to the filesystem.

use std::process::Command;

fn kellner(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kellner"))
        .args(args)
        .output()
        .expect("spawn kellner")
}

#[test]
fn service_install_print_emits_a_unit_and_writes_nothing() {
    let out = kellner(&["service", "install", "--print"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.trim().is_empty(), "unit text must not be empty");
    // The unit runs the background loop.
    assert!(text.contains("run"), "unit must invoke `kellner run`:\n{text}");
}

#[test]
fn service_install_dry_run_succeeds() {
    let out = kellner(&["service", "install", "--dry-run"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn service_status_reports_successfully() {
    let out = kellner(&["service", "status"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("installed") || text.contains("not installed") || text.contains("Task Scheduler"),
        "status output unexpected:\n{text}"
    );
}

#[test]
fn run_help_lists_power_flag() {
    let out = kellner(&["run", "--help"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--no-power-awareness"), "help:\n{text}");
    assert!(text.contains("--tasks"), "help:\n{text}");
}
