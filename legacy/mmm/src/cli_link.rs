//! CLI access for app-style installs: keep a `momos-music-manager` symlink
//! in a PATH directory so the terminal commands work from an `.app` bundle
//! install.
//!
//! # Why this exists
//!
//! On macOS the app is installed as `Momo's Music Manager.app` (DMG
//! drag-install into `/Applications`; the autoupdater replaces the bundle in
//! place via `autoupdate::dmg`). An `.app` bundle gives the user **no CLI
//! access** — the binary lives at `…/Contents/MacOS/momos-music-manager`,
//! which is not on `PATH`. Linux tar.gz installs run the bare binary and
//! need no link.
//!
//! This module creates (idempotently, at app startup and after every
//! successful DMG self-install) a symlink that points at the *installed
//! bundle's* binary. The link target is the stable bundle path
//! (`…/Applications/Momo's Music Manager.app/Contents/MacOS/…`), so it
//! survives in-place app updates — the updater replaces the bundle content,
//! not the path.
//!
//! # Where the link goes (decision, see README "CLI-Zugriff")
//!
//! Candidate directories are probed in order and the first **writable** one
//! wins:
//!
//! 1. `/usr/local/bin` — on `PATH` by default on macOS and Linux; typically
//!    only writable on machines with a dev setup (which is exactly where a
//!    CLI is wanted).
//! 2. `/opt/homebrew/bin` (macOS only, when it exists) — Apple-Silicon
//!    Homebrew prefix, on `PATH` for Homebrew users.
//! 3. `~/.local/bin` (created on demand) — the XDG user-binary directory
//!    the app already uses for its data (`~/.local/share/…`). Per-user, no
//!    admin rights needed. Caveat: on macOS this directory is **not** on
//!    the default `PATH` — the Settings page shows the exact path and the
//!    README documents the one-line `~/.zprofile` export.
//!
//! The fallback never silently fails: when no candidate is writable the
//! startup log (and the Settings CLI card) report the reason.

use std::path::{Path, PathBuf};

/// Name of the CLI entry point (binary name and symlink file name).
pub const CLI_NAME: &str = "momos-music-manager";

/// Where the CLI symlink lives + where it points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliLinkInfo {
    /// Directory containing the symlink (a PATH dir).
    pub dir: PathBuf,
    /// The symlink itself (`<dir>/momos-music-manager`).
    pub link_path: PathBuf,
    /// The executable the symlink points at.
    pub target_path: PathBuf,
    /// Whether this call created (or repaired) the link. `false` = the
    /// link already existed and pointed at the same target.
    pub created: bool,
}

/// Read-only view for the Settings UI (`GET …/status`, `cli` object).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliLinkStatus {
    /// Platform support (symlinks need a unix-like OS).
    pub supported: bool,
    /// Reason when the CLI is not available (unsupported platform /
    /// nothing to link / no writable dir).
    pub reason: Option<String>,
    /// The symlink path when one exists.
    pub link_path: Option<String>,
    /// The executable the link points at.
    pub target_path: Option<String>,
}

/// Failures of the CLI-link setup.
#[derive(Debug, thiserror::Error)]
pub enum CliLinkError {
    /// Symlink creation needs a unix-like OS.
    #[error("CLI symlink is not supported on this platform")]
    NotSupported,
    /// The binary to link does not exist (e.g. dev build outside a bundle).
    #[error("binary to link does not exist: {0}")]
    TargetMissing(PathBuf),
    /// None of the candidate PATH directories was usable.
    #[error("no writable PATH directory found (tried {0})")]
    NoWritableDir(String),
    /// The link target exists but is a directory — never removed
    /// automatically.
    #[error("cannot replace {0}: a directory exists at the link path")]
    LinkPathIsDirectory(PathBuf),
    /// I/O failure while probing or linking.
    #[error("CLI symlink I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Candidate PATH directories in priority order (first writable wins).
pub fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // 1. `/usr/local/bin` — on the default PATH of macOS and Linux.
    dirs.push(PathBuf::from("/usr/local/bin"));

    // 2. macOS: Apple-Silicon Homebrew prefix (when Homebrew exists).
    #[cfg(target_os = "macos")]
    {
        let brew = PathBuf::from("/opt/homebrew/bin");
        if brew.exists() {
            dirs.push(brew);
        }
    }

    // 3. Per-user XDG binary dir (created on demand by the probing).
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local").join("bin"));
    }

    dirs
}

/// Probe whether `dir` is usable as link destination: it must exist (we
/// create `~/.local/bin` on demand) and accept a write probe file.
pub fn dir_is_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".mmm-cli-link-probe-{}", std::process::id()));
    let written = std::fs::write(&probe, b"x").is_ok();
    if written {
        let _ = std::fs::remove_file(&probe);
    }
    written
}

/// First writable candidate directory (see [`candidate_dirs`]).
pub fn resolve_target_dir() -> Option<PathBuf> {
    candidate_dirs().into_iter().find(|d| dir_is_writable(d))
}

/// Resolve the executable inside a `.app` bundle
/// (`Contents/MacOS/momos-music-manager`, see `autoupdate::macos`).
pub fn bundle_executable(bundle: &Path) -> PathBuf {
    bundle
        .join("Contents")
        .join("MacOS")
        .join(CLI_NAME)
}

/// Ensure the CLI symlink for a `.app` bundle install.
///
/// Call after the bundle is in its final install location (startup of a
/// bundle-launched app, or right after a DMG self-install).
pub fn ensure_for_bundle(bundle: &Path) -> Result<CliLinkInfo, CliLinkError> {
    let target = bundle_executable(bundle);
    ensure(&target)
}

/// Ensure `momos-music-manager` on the PATH points at `target_binary`.
///
/// Idempotent: an existing link to the same target is left untouched.
/// Creates the link when missing and repairs a dangling/wrong link.
pub fn ensure(target_binary: &Path) -> Result<CliLinkInfo, CliLinkError> {
    #[cfg(not(unix))]
    {
        let _ = (target_binary,);
        return Err(CliLinkError::NotSupported);
    }

    #[cfg(unix)]
    {
        if !target_binary.is_file() {
            return Err(CliLinkError::TargetMissing(target_binary.to_path_buf()));
        }
        let dir = resolve_target_dir().ok_or_else(|| {
            CliLinkError::NoWritableDir(
                candidate_dirs()
                    .iter()
                    .map(|d| d.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        })?;
        ensure_in(&dir, target_binary)
    }
}

/// Core of [`ensure`] with an explicit link directory (testable without
/// touching real PATH dirs). `dir` must already exist and be writable.
pub fn ensure_in(dir: &Path, target_binary: &Path) -> Result<CliLinkInfo, CliLinkError> {
    #[cfg(not(unix))]
    {
        let _ = (dir, target_binary);
        return Err(CliLinkError::NotSupported);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let link_path = dir.join(CLI_NAME);

        // Existing link: leave it alone when it already points at the
        // target (canonicalize resolves the link chain); repair otherwise.
        if let Ok(existing_target) = std::fs::canonicalize(&link_path) {
            if existing_target == std::fs::canonicalize(target_binary).unwrap_or_else(|_| target_binary.to_path_buf())
            {
                return Ok(CliLinkInfo {
                    dir: dir.to_path_buf(),
                    link_path,
                    target_path: target_binary.to_path_buf(),
                    created: false,
                });
            }
            // Stale/dangling link (or plain file from an old manual
            // install) — replace it. Directories are never removed.
            match std::fs::symlink_metadata(&link_path) {
                Ok(meta) if meta.is_dir() => {
                    return Err(CliLinkError::LinkPathIsDirectory(link_path));
                }
                _ => {
                    std::fs::remove_file(&link_path)?;
                }
            }
        }

        symlink(target_binary, &link_path)?;
        Ok(CliLinkInfo {
            dir: dir.to_path_buf(),
            link_path,
            target_path: target_binary.to_path_buf(),
            created: true,
        })
    }
}

/// Read-only status for the Settings UI — never writes anything. Reports
/// where the link lives when it exists and points at the running bundle's
/// binary; a `reason` explains why the CLI is not available otherwise.
pub fn status() -> CliLinkStatus {
    #[cfg(not(unix))]
    {
        return CliLinkStatus {
            supported: false,
            reason: Some("CLI symlinks are not supported on this platform".into()),
            link_path: None,
            target_path: None,
        };
    }

    #[cfg(unix)]
    {
        // The link is only meaningful for bundle installs — nothing to
        // report when the running binary is not inside an `.app` bundle
        // (dev builds, Linux tar.gz, systemd).
        let Some(bundle) = crate::autoupdate::macos::running_app_bundle() else {
            return CliLinkStatus {
                supported: true,
                reason: None,
                link_path: None,
                target_path: None,
            };
        };
        let target = bundle_executable(&bundle);
        if !target.is_file() {
            return CliLinkStatus {
                supported: true,
                reason: Some("bundle binary not found — dev build?".into()),
                link_path: None,
                target_path: Some(target.display().to_string()),
            };
        }
        // Look for an existing link in the candidate dirs (probe order
        // matches the ensure step; probing may create `~/.local/bin` but
        // never writes the link itself).
        let resolved_target = std::fs::canonicalize(&target).unwrap_or(target.clone());
        for dir in candidate_dirs() {
            let link_path = dir.join(CLI_NAME);
            if let Ok(meta) = std::fs::symlink_metadata(&link_path) {
                if meta.file_type().is_symlink() {
                    let resolved =
                        std::fs::canonicalize(&link_path).unwrap_or(link_path.clone());
                    if resolved == resolved_target {
                        return CliLinkStatus {
                            supported: true,
                            reason: None,
                            link_path: Some(link_path.display().to_string()),
                            target_path: Some(target.display().to_string()),
                        };
                    }
                }
            }
        }
        let dir = resolve_target_dir();
        CliLinkStatus {
            supported: true,
            reason: Some(match dir {
                Some(d) => format!(
                    "not installed yet — the link is created at the next app start (into {})",
                    d.display()
                ),
                None => "no writable PATH directory found — add one (see README, CLI-Zugriff)".into(),
            }),
            link_path: None,
            target_path: Some(target.display().to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_name_matches_binary() {
        assert_eq!(CLI_NAME, "momos-music-manager");
        // The packaged macOS bundle layout (scripts/package-macos.sh).
        let bundle = Path::new("/Applications/Momo's Music Manager.app");
        assert_eq!(
            bundle_executable(bundle),
            Path::new("/Applications/Momo's Music Manager.app/Contents/MacOS/momos-music-manager")
        );
    }

    #[test]
    fn candidate_dirs_lead_with_usr_local_bin_and_home_bin() {
        let dirs = candidate_dirs();
        assert_eq!(dirs[0], PathBuf::from("/usr/local/bin"));
        assert!(dirs.iter().any(|d| d.ends_with(".local/bin")));
    }

    #[test]
    fn ensure_in_creates_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real-binary");
        std::fs::write(&target, b"bin").unwrap();
        let link_dir = dir.path().join("bin");
        std::fs::create_dir_all(&link_dir).unwrap();

        let info = ensure_in(&link_dir, &target).unwrap();
        assert!(info.created);
        assert_eq!(info.link_path, link_dir.join(CLI_NAME));
        assert!(info.link_path.is_symlink());
        assert_eq!(
            std::fs::canonicalize(&info.link_path).unwrap(),
            std::fs::canonicalize(&target).unwrap()
        );
    }

    #[test]
    fn ensure_in_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real-binary");
        std::fs::write(&target, b"bin").unwrap();
        let link_dir = dir.path().join("bin");
        std::fs::create_dir_all(&link_dir).unwrap();

        ensure_in(&link_dir, &target).unwrap();
        let again = ensure_in(&link_dir, &target).unwrap();
        assert!(!again.created, "second call must not recreate the link");
    }

    #[test]
    fn ensure_in_repairs_wrong_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real-binary");
        std::fs::write(&target, b"bin").unwrap();
        let other = dir.path().join("other-binary");
        std::fs::write(&other, b"bin2").unwrap();
        let link_dir = dir.path().join("bin");
        std::fs::create_dir_all(&link_dir).unwrap();

        ensure_in(&link_dir, &other).unwrap();
        let repaired = ensure_in(&link_dir, &target).unwrap();
        assert!(repaired.created, "stale link must be replaced");
        assert_eq!(
            std::fs::canonicalize(&repaired.link_path).unwrap(),
            std::fs::canonicalize(&target).unwrap()
        );
    }

    #[test]
    fn ensure_in_refuses_directory_at_link_path() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real-binary");
        std::fs::write(&target, b"bin").unwrap();
        let link_dir = dir.path().join("bin");
        std::fs::create_dir_all(&link_dir).unwrap();
        // A *directory* named momos-music-manager at the link path must
        // never be deleted by the ensure step.
        let occupied = link_dir.join(CLI_NAME);
        std::fs::create_dir_all(&occupied).unwrap();

        let err = ensure_in(&link_dir, &target).unwrap_err();
        assert!(matches!(err, CliLinkError::LinkPathIsDirectory(p) if p == occupied));
        assert!(occupied.is_dir(), "directory must be left untouched");
    }

    #[test]
    fn ensure_fails_when_target_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let err = ensure(&missing).unwrap_err();
        assert!(matches!(err, CliLinkError::TargetMissing(_)));
    }
}
