//! DMG handling for the Phase C macOS self-install.
//!
//! Pipeline (all steps run on macOS only; non-macOS builds fail fast with
//! [`DmgError::NotMacOS`], which keeps the module fully unit-testable on
//! CI):
//!
//! 1. `hdiutil attach` the verified DMG (read-only, no Finder browse) and
//!    parse the mount point from the output;
//! 2. locate the `.app` bundle inside the mount (see
//!    [`crate::autoupdate::macos::find_app_bundle`]);
//! 3. copy the bundle to a staging directory next to the target, then
//!    atomically replace the installed app: current `…/X.app` →
//!    `…/X.app.updater-bak`, staging → `…/X.app`. On any copy/rename failure
//!    the previous bundle is restored;
//! 4. `hdiutil detach` the DMG (best effort, `-force` retry).
//!
//! The `.updater-bak` of the previous version is kept as a manual-recovery
//! fallback (the swap-based health grace does not apply to whole-bundle
//! replacement) and is removed automatically by the next successful
//! install. The scheduler's crash-loop breaker (`autoupdate::update_auto`)
//! prevents endless re-install cycles of a version whose relaunch never
//! sticks.

use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

use super::macos::find_app_bundle;

/// Errors of the DMG install pipeline.
#[derive(Debug, Error)]
pub enum DmgError {
    /// DMG self-install is a macOS feature (hdiutil/ditto).
    #[error("DMG self-install requires macOS — hdiutil is unavailable on {os}/{arch}")]
    NotMacOS { os: &'static str, arch: &'static str },
    /// `hdiutil`/`ditto` could not be spawned or exited non-zero.
    #[error("failed to run `{command}`: {source}")]
    Command {
        command: &'static str,
        source: std::io::Error,
    },
    /// `hdiutil attach` ran but its output contained no mount point.
    #[error("hdiutil attach did not report a mount point:\n{output}")]
    NoMountPoint { output: String },
    /// No `.app` bundle inside the mounted image.
    #[error("no .app bundle found inside the mounted DMG ({mount})")]
    NoAppBundle { mount: PathBuf },
    /// General I/O error (copy/rename/cleanup).
    #[error("I/O error during DMG install: {0}")]
    Io(#[from] std::io::Error),
}

/// Full self-install: mount → replace the app bundle → unmount.
///
/// Returns the installed bundle path. The caller is responsible for the
/// verified DMG file afterwards (removed on success, kept otherwise).
pub fn install_dmg(dmg_path: &Path, app_dir: &Path) -> Result<PathBuf, DmgError> {
    let mount = mount_dmg(dmg_path)?;
    let result = (|| {
        let src = find_app_bundle(&mount).ok_or(DmgError::NoAppBundle {
            mount: mount.clone(),
        })?;
        install_bundle(&src, app_dir)
    })();
    // Unmount in every case (best effort — the mount must not linger after a
    // failed install either).
    if let Err(e) = detach_mount(&mount) {
        tracing::warn!("autoupdate: dmg detach failed (best effort): {e}");
    }
    result
}

/// Mount a DMG read-only and return its mount point.
fn mount_dmg(dmg_path: &Path) -> Result<PathBuf, DmgError> {
    ensure_macos()?;
    let output = run_capture("hdiutil")
        .with_arg("attach")
        .with_arg("-nobrowse")
        .with_arg("-readonly")
        .with_arg(dmg_path)
        .run()?;
    parse_mount_point(&output)
        .map(PathBuf::from)
        .ok_or(DmgError::NoMountPoint { output })
}

/// Unmount a mounted DMG (`-force` retry on a busy volume).
fn detach_mount(mount: &Path) -> Result<(), DmgError> {
    ensure_macos()?;
    let plain = run_detached_status("hdiutil", &["detach", &mount.to_string_lossy()]);
    if plain {
        return Ok(());
    }
    let forced = run_detached_status("hdiutil", &["detach", "-force", &mount.to_string_lossy()]);
    if forced {
        Ok(())
    } else {
        Err(DmgError::Command {
            command: "hdiutil detach",
            source: std::io::Error::new(
                std::io::ErrorKind::Other,
                "detach failed (plain and forced)",
            ),
        })
    }
}

/// Replace the app bundle at `app_dir/<name>` with the one at `src_app`.
///
/// Copy → staging, swap with backup, restore on failure (see module docs).
pub fn install_bundle(src_app: &Path, app_dir: &Path) -> Result<PathBuf, DmgError> {
    std::fs::create_dir_all(app_dir)?;
    let name = src_app
        .file_name()
        .ok_or_else(|| {
            DmgError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("bundle path {} has no file name", src_app.display()),
            ))
        })?
        .to_os_string();
    let target = app_dir.join(&name);
    let staging = app_dir.join(format!(
        ".{}.updating-{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let backup = app_dir.join(format!("{}.updater-bak", name.to_string_lossy()));

    // Clean leftovers of previous runs (a stale backup is superseded by
    // this install; a stale staging dir must not shadow the rename).
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    if backup.exists() {
        std::fs::remove_dir_all(&backup)?;
    }

    // 1. Copy the verified bundle into a staging dir next to the target
    //    (same filesystem → atomic rename afterwards).
    copy_bundle(src_app, &staging)?;

    // 2. Move the current app aside (atomic) …
    let had_previous = target.exists();
    if had_previous {
        std::fs::rename(&target, &backup).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staging);
            DmgError::Io(e)
        })?;
    }

    // 3. … and promote the staged bundle. On failure, restore the previous
    //    version so a failed install never leaves the app missing.
    if let Err(e) = std::fs::rename(&staging, &target) {
        let _ = std::fs::remove_dir_all(&staging);
        if had_previous {
            let _ = std::fs::rename(&backup, &target);
        }
        return Err(DmgError::Io(e));
    }

    Ok(target)
}

/// Copy an app bundle preserving symlinks/permissions/extended attributes.
///
/// Uses `/usr/bin/ditto` on macOS (the standard tool for bundle copies);
/// falls back to a plain recursive copy elsewhere (used by unit tests and
/// as a safety net).
pub fn copy_bundle(src: &Path, dst: &Path) -> Result<(), DmgError> {
    if cfg!(target_os = "macos") {
        return run_detached_status("/usr/bin/ditto", &[
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
        ])
        .then_some(())
        .ok_or_else(|| DmgError::Command {
            command: "/usr/bin/ditto",
            source: std::io::Error::new(std::io::ErrorKind::Other, "ditto exited non-zero"),
        });
    }
    copy_dir_all(src, dst)
}

/// Parse the mount point from `hdiutil attach` output.
///
/// The plain output format is one line per mounted volume, e.g.:
///
/// ```text
/// /dev/disk4s1   	Apple_HFS   	/Volumes/Momo's Music Manager
/// ```
///
/// The mount path starts at the first `/Volumes/` marker and runs to the
/// end of its line (volume names may contain spaces). When several volumes
/// are reported, the last one wins.
pub fn parse_mount_point(output: &str) -> Option<String> {
    output
        .lines()
        .filter(|l| l.contains("/Volumes/"))
        .last()
        .and_then(|line| {
            let idx = line.find("/Volumes/")?;
            let mount = line[idx..].trim();
            if mount.is_empty() {
                None
            } else {
                Some(mount.to_string())
            }
        })
}

/// Run a command with no stdin/stdout/stderr inheritance and return whether
/// it exited successfully.
fn run_detached_status(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Capture `stdout` of a command (stderr included in the error case).
fn run_capture(program: &'static str) -> Capture {
    Capture::new(program)
}

/// Recursive directory copy (symlink-aware fallback for non-macOS/test runs).
fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), DmgError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&path, &target)?;
        } else if file_type.is_symlink() {
            // Unix: recreate the symlink. Windows (or other non-unix):
            // `fs::copy` follows the link and copies the target file.
            #[cfg(unix)]
            {
                let link = std::fs::read_link(&path)?;
                std::os::unix::fs::symlink(link, &target)?;
            }
            #[cfg(not(unix))]
            {
                std::fs::copy(&path, &target)?;
            }
        } else {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

fn ensure_macos() -> Result<(), DmgError> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(DmgError::NotMacOS {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        })
    }
}

/// Tiny builder for `Command::output()` so commands read declaratively.
struct Capture {
    program: &'static str,
    args: Vec<std::ffi::OsString>,
}

impl Capture {
    fn new(program: &'static str) -> Self {
        Self {
            program,
            args: Vec::new(),
        }
    }

    fn with_arg(mut self, arg: impl AsRef<std::ffi::OsStr>) -> Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    fn run(self) -> Result<String, DmgError> {
        let mut cmd = Command::new(self.program);
        cmd.args(&self.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let output = cmd
            .output()
            .map_err(|source| DmgError::Command {
                command: self.program,
                source,
            })?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(DmgError::Command {
                command: self.program,
                source: std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("exited with {:?}: {}", output.status, err),
                ),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mount_point_typical_output() {
        let out = "/dev/disk4s1   	Apple_HFS   	/Volumes/Momo's Music Manager\n";
        assert_eq!(
            parse_mount_point(out),
            Some("/Volumes/Momo's Music Manager".to_string())
        );
    }

    #[test]
    fn parse_mount_point_ignores_noise_lines_and_takes_last() {
        let out = "\nprevious line\n/dev/disk3s1 Apple_HFS /Volumes/Other\n/dev/disk4s1 Apple_HFS /Volumes/Momo's Music Manager\n";
        assert_eq!(
            parse_mount_point(out),
            Some("/Volumes/Momo's Music Manager".to_string())
        );
    }

    #[test]
    fn parse_mount_point_none_without_volumes_line() {
        assert_eq!(parse_mount_point("hdiutil: attach failed"), None);
        assert_eq!(parse_mount_point(""), None);
        assert_eq!(parse_mount_point("volume at /not-volumes/x"), None);
    }

    #[test]
    fn install_bundle_fresh_install() {
        let work = tempfile::tempdir().unwrap();
        let app_dir = work.path().join("Applications");
        let src = work.path().join("Momo's Music Manager.app");
        std::fs::create_dir_all(src.join("Contents/MacOS")).unwrap();
        std::fs::write(src.join("Contents/MacOS/momos-music-manager"), b"new-binary").unwrap();

        let installed = install_bundle(&src, &app_dir).unwrap();
        assert_eq!(installed, app_dir.join("Momo's Music Manager.app"));
        assert_eq!(
            std::fs::read(installed.join("Contents/MacOS/momos-music-manager")).unwrap(),
            b"new-binary"
        );
        // No previous version → no backup left behind.
        assert!(!app_dir.join("Momo's Music Manager.app.updater-bak").exists());
    }

    #[test]
    fn install_bundle_over_existing_keeps_previous_as_updater_bak() {
        let work = tempfile::tempdir().unwrap();
        let app_dir = work.path().join("Applications");
        std::fs::create_dir_all(&app_dir).unwrap();
        let target = app_dir.join("Momo's Music Manager.app");
        std::fs::create_dir_all(target.join("Contents/MacOS")).unwrap();
        std::fs::write(target.join("Contents/MacOS/momos-music-manager"), b"old-binary").unwrap();

        // Source bundle with the same file name in a different directory (the
        // installed bundle is replaced under its own name).
        let src_dir = work.path().join("staged");
        let src = src_dir.join("Momo's Music Manager.app");
        std::fs::create_dir_all(src.join("Contents/MacOS")).unwrap();
        std::fs::write(src.join("Contents/MacOS/momos-music-manager"), b"new-binary").unwrap();

        install_bundle(&src, &app_dir).unwrap();
        assert_eq!(
            std::fs::read(target.join("Contents/MacOS/momos-music-manager")).unwrap(),
            b"new-binary"
        );
        assert_eq!(
            std::fs::read(
                app_dir
                    .join("Momo's Music Manager.app.updater-bak")
                    .join("Contents/MacOS/momos-music-manager")
            )
            .unwrap(),
            b"old-binary"
        );
    }

    #[test]
    fn install_bundle_cleans_stale_backup_and_staging() {
        let work = tempfile::tempdir().unwrap();
        let app_dir = work.path().join("Applications");
        std::fs::create_dir_all(&app_dir).unwrap();
        let stale_bak = app_dir.join("Momo's Music Manager.app.updater-bak");
        std::fs::create_dir_all(&stale_bak).unwrap();
        std::fs::write(stale_bak.join("stale"), b"x").unwrap();
        // Stale staging dir (leftover of a crashed run — same name pattern as
        // the current run so the cleanup branch is exercised).
        let stale_staging = app_dir.join(format!(
            ".Momo's Music Manager.app.updating-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&stale_staging).unwrap();

        let src = work.path().join("Momo's Music Manager.app");
        std::fs::create_dir_all(src.join("Contents/MacOS")).unwrap();
        std::fs::write(src.join("Contents/MacOS/momos-music-manager"), b"bin").unwrap();

        install_bundle(&src, &app_dir).unwrap();
        assert!(!stale_bak.exists());
        assert!(!stale_staging.exists());
        assert!(app_dir.join("Momo's Music Manager.app").exists());
    }

    #[cfg(unix)]
    #[test]
    fn copy_failure_is_surfaced_and_previous_stays_intact() {
        use std::os::unix::fs::PermissionsExt;
        let work = tempfile::tempdir().unwrap();
        let app_dir = work.path().join("Applications");
        let target = app_dir.join("Momo's Music Manager.app");
        std::fs::create_dir_all(target.join("Contents/MacOS")).unwrap();
        std::fs::write(target.join("Contents/MacOS/momos-music-manager"), b"old").unwrap();

        // A source bundle whose contents cannot be read (dir without read
        // permission) makes the staging copy fail.
        let src = work.path().join("broken.app");
        std::fs::create_dir_all(src.join("Contents/MacOS")).unwrap();
        std::fs::write(src.join("Contents/MacOS/momos-music-manager"), b"new").unwrap();
        std::fs::set_permissions(src.join("Contents/MacOS"), std::fs::Permissions::from_mode(0o000))
            .unwrap();

        let result = install_bundle(&src, &app_dir);
        std::fs::set_permissions(src.join("Contents/MacOS"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(matches!(result, Err(DmgError::Io(_))));
        // Previous version untouched, no backup/staging litter.
        assert_eq!(
            std::fs::read(target.join("Contents/MacOS/momos-music-manager")).unwrap(),
            b"old"
        );
        assert!(!app_dir.join("Momo's Music Manager.app.updater-bak").exists());
    }

    #[test]
    fn non_macos_runtime_errors_are_explicit() {
        if cfg!(target_os = "macos") {
            return;
        }
        assert!(matches!(
            install_dmg(Path::new("/tmp/x.dmg"), Path::new("/Applications")),
            Err(DmgError::NotMacOS { .. })
        ));
    }
}
