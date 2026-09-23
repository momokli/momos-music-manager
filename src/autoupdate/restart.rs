//! Self-restart after a successful auto-apply (Phase C).
//!
//! The update is already on disk when this runs (`verify::apply` swapped the
//! binary or the DMG install replaced the `.app` bundle). Restarting means:
//! spawn a *detached* relauncher that waits [`RESTART_DELAY_SECS`] (so the
//! old process can release the HTTP port / DB handles), then starts the
//! updated executable; the old process exits right after. On the next boot
//! the swap-based health grace commits (or rolls back) the update — the
//! crash-loop breaker in `autoupdate::update_auto` is the second line of
//! defense and prevents endless apply → restart cycles.
//!
//! Platform decision ([`plan_auto_restart`]):
//!
//! - **systemd** (detected via `INVOCATION_ID`, set for every systemd
//!   unit): no relauncher — `Restart=always` in `deploy/momos-music-manager.service`
//!   restarts the service with the new binary. Exiting the process is
//!   enough; the plan tells the caller to exit.
//! - **macOS `.app`**: relaunch via LaunchServices (`open`) so the menu-bar
//!   app re-registers properly — but only when the running bundle is the one
//!   that was just replaced (same install directory); otherwise the update
//!   target differs from the running app and auto-relaunch is skipped with a
//!   hint.
//! - **plain executable** (Linux/macOS dev runs, Windows): re-exec the
//!   current executable path (which now points at the new binary) with the
//!   original arguments.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::autoupdate::macos;

/// Seconds the detached relauncher waits before starting the new process —
/// long enough for the old process to exit and free the HTTP port.
pub const RESTART_DELAY_SECS: u64 = 2;

/// What `serve()` should do after a successful auto-apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartPlan {
    /// Running as a systemd service (`Restart=always` in the shipped unit):
    /// just exit — the service manager starts the new binary.
    ManagedBySystemd,
    /// Re-exec the current executable path (new binary in place) with the
    /// original arguments after a short delay.
    RelaunchExec {
        program: PathBuf,
        args: Vec<String>,
    },
    /// Relaunch the (just replaced) app bundle via LaunchServices.
    RelaunchApp(PathBuf),
    /// No automatic restart (e.g. the updated app bundle is not the one
    /// currently running) — the caller keeps serving and logs the reason.
    Skip { reason: &'static str },
}

impl RestartPlan {
    /// Whether the caller should terminate the process to complete the
    /// restart (systemd case included — the unit restarts the service).
    pub fn requires_process_exit(&self) -> bool {
        matches!(
            self,
            RestartPlan::ManagedBySystemd
                | RestartPlan::RelaunchExec { .. }
                | RestartPlan::RelaunchApp(_)
        )
    }
}

/// Whether the process runs as a systemd service unit.
///
/// systemd exports `INVOCATION_ID` to every unit process; plain shell/launch
/// agent starts do not have it.
pub fn is_systemd_managed() -> bool {
    std::env::var("INVOCATION_ID").is_ok()
}

/// Decide how to restart after an auto-apply.
///
/// `app_install_dir` is the configured macOS app install directory (from
/// `MOMOS_AUTOUPDATE_APP_DIR` / `[autoupdate] app_dir`); `None` means the
/// default `/Applications`.
pub fn plan_auto_restart(app_install_dir: Option<&Path>) -> RestartPlan {
    if cfg!(target_os = "macos") {
        // The DMG install replaced `app_dir/<name>.app`. Relaunch it — but
        // only when it *is* the running bundle: relaunching a different copy
        // would start the old version we just replaced elsewhere.
        if let Some(running) = macos::running_app_bundle() {
            let install_dir = app_install_dir
                .map(|p| p.to_path_buf())
                .unwrap_or_else(macos::default_app_dir);
            let running_in_install_dir = running
                .parent()
                .map(|p| p == install_dir.as_path())
                .unwrap_or(false);
            if running_in_install_dir {
                return RestartPlan::RelaunchApp(running);
            }
            return RestartPlan::Skip {
                reason: "the running app bundle is outside the update install directory — start the updated app manually",
            };
        }
        // Bare binary on macOS (dev run): re-exec like Linux.
        return exec_plan();
    }

    if is_systemd_managed() {
        return RestartPlan::ManagedBySystemd;
    }
    exec_plan()
}

/// Relaunch plan for the plain-executable case.
fn exec_plan() -> RestartPlan {
    let program = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("momos-music-manager"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    RestartPlan::RelaunchExec { program, args }
}

/// Execute a restart plan (spawn the detached relauncher; never blocks).
pub fn execute_plan(plan: &RestartPlan) -> std::io::Result<()> {
    match plan {
        RestartPlan::ManagedBySystemd | RestartPlan::Skip { .. } => Ok(()),
        RestartPlan::RelaunchApp(bundle) => spawn_delayed_open(bundle),
        RestartPlan::RelaunchExec { program, args } => spawn_delayed_exec(program, args),
    }
}

/// Detached relaunch of an executable after [`RESTART_DELAY_SECS`].
///
/// Unix: a detached `/bin/sh` (new process group, no stdio) that sleeps and
/// then `exec`s the program with the original arguments. Windows: a
/// detached `cmd` with `timeout` (best effort — Windows swaps require a
/// stopped server anyway, see `swap.rs`).
fn spawn_delayed_exec(program: &Path, args: &[String]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(format!("sleep {RESTART_DELAY_SECS}; exec \"$0\" \"$@\""))
            .arg(program) // $0
            .args(args)
            .process_group(0) // detach from the dying parent's session
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.spawn()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // Windows: cmd /C "timeout /t N /nobreak >nul & start "" program args"
        let mut quoted = format!(
            "timeout /t {} /nobreak >nul & start \"\" \"{}\"",
            RESTART_DELAY_SECS,
            program.display()
        );
        for a in args {
            quoted.push(' ');
            quoted.push('"');
            quoted.push_str(a);
            quoted.push('"');
        }
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", &quoted]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
        }
        cmd.spawn()?;
        Ok(())
    }
}

/// Detached `open` of an app bundle after [`RESTART_DELAY_SECS`] (macOS).
///
/// The delay is essential: `open` while the old process is still running
/// would activate the *old* instance (same bundle id) instead of launching
/// the new one.
fn spawn_delayed_open(bundle: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(format!("sleep {RESTART_DELAY_SECS}; exec /usr/bin/open \"$1\""))
            .arg("mmm-open") // $0
            .arg(bundle) // $1
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.spawn()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = bundle;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "open-based relaunch is macOS-only",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_requires_exit_only_for_real_relaunches() {
        assert!(RestartPlan::ManagedBySystemd.requires_process_exit());
        assert!(
            RestartPlan::RelaunchExec {
                program: PathBuf::from("/x"),
                args: vec![],
            }
            .requires_process_exit()
        );
        assert!(RestartPlan::RelaunchApp(PathBuf::from("/x.app")).requires_process_exit());
        assert!(!RestartPlan::Skip { reason: "x" }.requires_process_exit());
    }

    #[test]
    fn exec_plan_keeps_original_argv() {
        // Not easily asserted against env::args from a test — verify the
        // shape only.
        let plan = exec_plan();
        match plan {
            RestartPlan::RelaunchExec { program, .. } => {
                assert!(!program.as_os_str().is_empty());
            }
            other => panic!("expected exec plan, got {other:?}"),
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_default_is_exec_or_systemd() {
        let plan = plan_auto_restart(None);
        match plan {
            RestartPlan::ManagedBySystemd | RestartPlan::RelaunchExec { .. } => {}
            other => panic!("unexpected plan: {other:?}"),
        }
    }
}
