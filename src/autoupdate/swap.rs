//! Atomic binary swap with backup, update marker state machine and rollback.
//!
//! # Model
//!
//! `update apply` performs, next to the running binary (same directory so the
//! final `rename` is atomic on POSIX filesystems):
//!
//! 1. write `update-state.json` (marker: old version, new version, attempt count)
//! 2. rename current binary → `momos-music-manager.bak`
//! 3. write new binary to a temp file, `chmod +x`, rename → current binary
//!
//! On the next `serve` start the new binary runs `recovery_action`:
//!
//! * marker with `start_count == 0` → **first start** of the new binary:
//!   arm a health grace timer (default 60 s); if the process survives, the
//!   update is *committed* (`.bak` + marker removed).
//! * marker with `start_count >= MAX_UNHEALTHY_STARTS` → the new binary never
//!   became healthy → **auto-rollback** (restore `.bak` over the binary).
//! * marker with `committed == true` or whose versions match neither binary →
//!   stale state → cleanup.
//!
//! Manual `update rollback` restores `.bak` at any time.
//!
//! # Windows note
//!
//! Renaming a *running* executable is not allowed on Windows. `update apply`
//! therefore requires the server to be stopped (`systemctl stop …` on Linux /
//! `sc stop` or Task Manager on Windows); the error message tells the user
//! exactly that. Rollback at startup suffers the same limitation and is
//! reported with a hint to run `update rollback` manually.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Name of the marker file written next to the binary.
pub const MARKER_FILE: &str = "update-state.json";
/// Backup suffix for the previous binary.
pub const BACKUP_SUFFIX: &str = ".bak";
/// Starts a new binary may attempt (without a committed health check) before
/// the updater auto-rolls back.
pub const MAX_UNHEALTHY_STARTS: u32 = 2;

#[derive(Debug, Error)]
pub enum SwapError {
    #[error("current executable path is unavailable: {0}")]
    NoCurrentExe(std::io::Error),
    #[error("cannot write marker file: {0}")]
    MarkerWrite(std::io::Error),
    #[error("cannot read marker file: {0}")]
    MarkerRead(std::io::Error),
    #[error("marker file is corrupt: {0}")]
    MarkerParse(serde_json::Error),
    #[error("cannot move current binary to backup: {0}")]
    Backup(std::io::Error),
    #[error("cannot write new binary: {0}")]
    WriteNew(std::io::Error),
    #[error("cannot replace current binary: {0}")]
    Replace(std::io::Error),
    #[error("no backup binary found at {0} — nothing to roll back to")]
    NoBackup(PathBuf),
    #[error("rollback failed: {0}")]
    Rollback(std::io::Error),
    #[error("cleanup of {path} failed: {error}")]
    Cleanup { path: PathBuf, error: std::io::Error },
}

/// Persisted state of an in-flight update.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateMarker {
    pub old_version: String,
    pub new_version: String,
    /// Unix timestamp of `update apply`.
    pub created_at_unix: i64,
    /// Number of times the new binary has been started without committing.
    pub start_count: u32,
    /// Set once the health check passed (then marker+backup are removed).
    pub committed: bool,
}

/// What `serve` startup should do about a pending update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Nothing pending.
    None,
    /// First start of the new binary — wait the configured health grace
    /// period, then commit.
    CommitAfterGrace { new_version: String },
    /// Repeated unhealthy starts — restore the previous binary.
    AutoRollback { old_version: String },
    /// Marker/backup left over from an already-finished or aborted update.
    CleanupStale,
}

/// Directory of the currently running executable (falls back to the working
/// directory if `current_exe` is unavailable).
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// File name of the currently running executable.
pub fn exe_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "momos-music-manager".to_string())
}

pub fn marker_path(dir: &Path) -> PathBuf {
    dir.join(MARKER_FILE)
}

pub fn backup_path(dir: &Path, binary_name: &str) -> PathBuf {
    dir.join(format!("{binary_name}{BACKUP_SUFFIX}"))
}

pub fn read_marker(dir: &Path) -> Result<Option<UpdateMarker>, SwapError> {
    let path = marker_path(dir);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(SwapError::MarkerRead)?;
    let marker = serde_json::from_str(&text).map_err(SwapError::MarkerParse)?;
    Ok(Some(marker))
}

pub fn write_marker(dir: &Path, marker: &UpdateMarker) -> Result<(), SwapError> {
    let text = serde_json::to_string_pretty(marker).expect("marker serializes");
    std::fs::write(marker_path(dir), text).map_err(SwapError::MarkerWrite)
}

/// Remove the marker file (used when committing).
pub fn remove_marker(dir: &Path) -> Result<(), SwapError> {
    let path = marker_path(dir);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(SwapError::Cleanup { path, error: e }),
    }
}

/// Determine the startup recovery action for a pending update.
///
/// `current_version` must be `env!("CARGO_PKG_VERSION")` of the *running*
/// binary. Side effect: increments `start_count` when the running binary is
/// the new, uncommitted version (and persists the marker).
pub fn recovery_action(
    dir: &Path,
    current_version: &str,
    binary_name: &str,
) -> Result<RecoveryAction, SwapError> {
    let Some(mut marker) = read_marker(dir)? else {
        return Ok(RecoveryAction::None);
    };

    if marker.committed {
        // Commit already recorded — drop leftovers.
        let _ = remove_marker(dir);
        let _ = remove_file_if_present(&backup_path(dir, binary_name));
        return Ok(RecoveryAction::CleanupStale);
    }

    if current_version == marker.old_version {
        // The old binary is still in place (swap never happened or was
        // rolled back) — the marker is stale.
        let _ = remove_marker(dir);
        let _ = remove_file_if_present(&backup_path(dir, binary_name));
        return Ok(RecoveryAction::CleanupStale);
    }

    if current_version == marker.new_version {
        marker.start_count += 1;
        if marker.start_count > MAX_UNHEALTHY_STARTS {
            // The new binary has been started repeatedly and never committed
            // — roll back to the previous version.
            write_marker(dir, &marker)?;
            return Ok(RecoveryAction::AutoRollback {
                old_version: marker.old_version.clone(),
            });
        }
        write_marker(dir, &marker)?;
        return Ok(RecoveryAction::CommitAfterGrace {
            new_version: marker.new_version.clone(),
        });
    }

    // Version matches neither old nor new — foreign/unknown binary.
    let _ = remove_marker(dir);
    let _ = remove_file_if_present(&backup_path(dir, binary_name));
    Ok(RecoveryAction::CleanupStale)
}

/// Commit an update: remove the backup binary and the marker.
pub fn commit_update(dir: &Path, binary_name: &str) -> Result<(), SwapError> {
    let _ = remove_file_if_present(&backup_path(dir, binary_name));
    remove_marker(dir)
}

/// Restore the backup binary over the current one (rollback).
pub fn rollback(dir: &Path, binary_name: &str) -> Result<(), SwapError> {
    let bak = backup_path(dir, binary_name);
    if !bak.exists() {
        return Err(SwapError::NoBackup(bak));
    }
    let target = dir.join(binary_name);
    // POSIX: rename overwrites. Windows: the running exe cannot be replaced;
    // remove first (only possible when the server is stopped).
    match std::fs::rename(&bak, &target) {
        Ok(()) => {}
        Err(e) => {
            if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied {
                return Err(SwapError::Rollback(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "cannot replace the running executable on Windows — stop the server, \
                         then run `momos-music-manager update rollback` ({e})"
                    ),
                )));
            }
            return Err(SwapError::Rollback(e));
        }
    }
    remove_marker(dir)
}

/// Swap the running binary with `new_binary_bytes`:
/// 1. write marker (caller responsibility — done before this),
/// 2. move current binary → `.bak`,
/// 3. write new binary to temp, make executable, rename over current.
pub fn swap_binary(dir: &Path, binary_name: &str, new_binary_bytes: &[u8]) -> Result<(), SwapError> {
    let target = dir.join(binary_name);
    let bak = backup_path(dir, binary_name);

    // 1. Move the current binary aside.
    match std::fs::rename(&target, &bak) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No current binary at the expected path — nothing to back up.
        }
        Err(e) => {
            return Err(SwapError::Backup(std::io::Error::new(
                e.kind(),
                format!("cannot move current binary to backup ({e}) — on Windows stop the server first"),
            )));
        }
    }

    // 2. Write the new binary to a temp file in the same directory (same
    //    filesystem → atomic rename), then replace the target.
    let tmp = dir.join(format!(".update-new-{}", std::process::id()));
    let write_result = (|| -> Result<(), SwapError> {
        std::fs::write(&tmp, new_binary_bytes).map_err(SwapError::WriteNew)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&tmp)
                .map_err(SwapError::WriteNew)?
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&tmp, perms).map_err(SwapError::WriteNew)?;
        }
        match std::fs::rename(&tmp, &target) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(SwapError::Replace(e))
            }
        }
    })();

    if let Err(e) = write_result {
        // Best-effort restore so the previous binary is not lost.
        if bak.exists() {
            let _ = std::fs::rename(&bak, &target);
        }
        return Err(e);
    }
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<(), SwapError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(SwapError::Cleanup {
            path: path.to_path_buf(),
            error: e,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn marker(old: &str, new: &str, start_count: u32, committed: bool) -> UpdateMarker {
        UpdateMarker {
            old_version: old.to_string(),
            new_version: new.to_string(),
            created_at_unix: now(),
            start_count,
            committed,
        }
    }

    #[test]
    fn marker_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        write_marker(dir.path(), &marker("1.0.1", "1.1.0", 0, false)).unwrap();
        let read = read_marker(dir.path()).unwrap().unwrap();
        assert_eq!(read.old_version, "1.0.1");
        assert_eq!(read.new_version, "1.1.0");
        assert_eq!(read.start_count, 0);
    }

    #[test]
    fn no_marker_means_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            recovery_action(dir.path(), "1.0.1", "mmm-test-bin").unwrap(),
            RecoveryAction::None
        );
    }

    #[test]
    fn first_start_arms_commit() {
        let dir = tempfile::tempdir().unwrap();
        write_marker(dir.path(), &marker("1.0.1", "1.1.0", 0, false)).unwrap();
        let action = recovery_action(dir.path(), "1.1.0", "mmm-test-bin").unwrap();
        assert!(matches!(action, RecoveryAction::CommitAfterGrace { .. }));
        // start_count was incremented and persisted.
        let read = read_marker(dir.path()).unwrap().unwrap();
        assert_eq!(read.start_count, 1);
    }

    #[test]
    fn repeated_unhealthy_starts_roll_back() {
        let dir = tempfile::tempdir().unwrap();
        write_marker(dir.path(), &marker("1.0.1", "1.1.0", 2, false)).unwrap();
        let action = recovery_action(dir.path(), "1.1.0", "mmm-test-bin").unwrap();
        assert_eq!(
            action,
            RecoveryAction::AutoRollback {
                old_version: "1.0.1".to_string()
            }
        );
    }

    #[test]
    fn old_binary_running_cleans_up_stale_marker() {
        let dir = tempfile::tempdir().unwrap();
        write_marker(dir.path(), &marker("1.0.1", "1.1.0", 0, false)).unwrap();
        assert_eq!(
            recovery_action(dir.path(), "1.0.1", "mmm-test-bin").unwrap(),
            RecoveryAction::CleanupStale
        );
        assert!(read_marker(dir.path()).unwrap().is_none());
    }

    #[test]
    fn committed_marker_is_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        write_marker(dir.path(), &marker("1.0.1", "1.1.0", 1, true)).unwrap();
        assert_eq!(
            recovery_action(dir.path(), "1.1.0", "mmm-test-bin").unwrap(),
            RecoveryAction::CleanupStale
        );
        assert!(read_marker(dir.path()).unwrap().is_none());
    }

    #[test]
    fn swap_creates_backup_and_commit_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let binary = "mmm-test-bin";
        let target = dir.path().join(binary);
        std::fs::write(&target, b"old-binary-content").unwrap();

        write_marker(dir.path(), &marker("1.0.1", "1.1.0", 0, false)).unwrap();
        swap_binary(dir.path(), binary, b"new-binary-content").unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new-binary-content");
        assert_eq!(
            std::fs::read(dir.path().join(format!("{binary}.bak"))).unwrap(),
            b"old-binary-content"
        );

        commit_update(dir.path(), binary).unwrap();
        assert!(!dir.path().join(format!("{binary}.bak")).exists());
        assert!(!marker_path(dir.path()).exists());
    }

    #[test]
    fn rollback_restores_previous_binary() {
        let dir = tempfile::tempdir().unwrap();
        let binary = "mmm-test-bin";
        let target = dir.path().join(binary);
        std::fs::write(&target, b"old-binary-content").unwrap();

        write_marker(dir.path(), &marker("1.0.1", "1.1.0", 0, false)).unwrap();
        swap_binary(dir.path(), binary, b"new-binary-content").unwrap();
        rollback(dir.path(), binary).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"old-binary-content");
        assert!(!marker_path(dir.path()).exists());
    }

    #[test]
    fn rollback_without_backup_fails() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mmm-test-bin"), b"x").unwrap();
        assert!(rollback(dir.path(), "mmm-test-bin").is_err());
    }
}
