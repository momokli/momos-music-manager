//! macOS helpers for the Phase C self-install: `.app` bundle discovery and
//! install-target resolution.
//!
//! The packaged app is `Momo's Music Manager.app` (see
//! `scripts/package-macos.sh`) with the binary at
//! `Contents/MacOS/momos-music-manager`. The updater replaces the whole
//! bundle in the install directory (default `/Applications`, configurable
//! via `MOMOS_AUTOUPDATE_APP_DIR` / `[autoupdate] app_dir`).

use std::path::{Path, PathBuf};

/// File name of the packaged app bundle.
pub fn app_bundle_name() -> &'static str {
    "Momo's Music Manager.app"
}

/// Name of the executable inside the bundle (`Contents/MacOS/…`).
pub fn app_binary_name() -> &'static str {
    "momos-music-manager"
}

/// Default install directory for the self-installing update (system-wide
/// apps go to `/Applications`; configurable for e.g. `~/Applications`).
pub fn default_app_dir() -> PathBuf {
    PathBuf::from("/Applications")
}

/// Whether `path` looks like a `.app` bundle directory.
pub fn is_app_bundle(path: &Path) -> bool {
    path.is_dir() && path.extension().map(|e| e == "app").unwrap_or(false)
}

/// Find the top-level `.app` bundle inside `dir` (e.g. a mounted DMG).
///
/// Prefers the exact packaged name (`Momo's Music Manager.app`); falls back
/// to the first other `.app` when present.
pub fn find_app_bundle(dir: &Path) -> Option<PathBuf> {
    let mut first_other = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_app_bundle(&path) {
            continue;
        }
        if path.file_name().map(|n| n == app_bundle_name()).unwrap_or(false) {
            return Some(path);
        }
        if first_other.is_none() {
            first_other = Some(path);
        }
    }
    first_other
}

/// Bundle of the currently running app, if the executable lives inside a
/// `.app` bundle (Finder/LaunchServices launch). Dev builds run the bare
/// binary and return `None`.
pub fn running_app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    for ancestor in exe.ancestors() {
        if is_app_bundle(ancestor) {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

/// Path of the executable inside a bundle.
pub fn bundle_executable(bundle: &Path) -> PathBuf {
    bundle
        .join("Contents")
        .join("MacOS")
        .join(app_binary_name())
}

/// Whether the running executable belongs to the bundle at `candidate`
/// (same file, compared via canonicalization when both resolve).
pub fn running_bundle_matches(candidate: &Path) -> bool {
    match running_app_bundle() {
        Some(running) => {
            let a = std::fs::canonicalize(&running).unwrap_or_else(|_| running.clone());
            let b = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
            a == b
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_packaging_script() {
        assert_eq!(app_bundle_name(), "Momo's Music Manager.app");
        assert_eq!(app_binary_name(), "momos-music-manager");
        assert_eq!(default_app_dir(), PathBuf::from("/Applications"));
    }

    #[test]
    fn is_app_bundle_detects_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Momo's Music Manager.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        assert!(is_app_bundle(&dir.path().join("Momo's Music Manager.app")));
        assert!(!is_app_bundle(&dir.path().join("random.txt")));
        std::fs::write(dir.path().join("readme.txt"), "x").unwrap();
        assert!(!is_app_bundle(&dir.path().join("readme.txt")));
    }

    #[test]
    fn find_app_bundle_prefers_exact_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Other.app")).unwrap();
        std::fs::create_dir_all(dir.path().join("Momo's Music Manager.app")).unwrap();
        assert_eq!(
            find_app_bundle(dir.path()),
            Some(dir.path().join("Momo's Music Manager.app"))
        );
    }

    #[test]
    fn find_app_bundle_falls_back_to_other() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Some App.app")).unwrap();
        assert_eq!(
            find_app_bundle(dir.path()),
            Some(dir.path().join("Some App.app"))
        );
        assert_eq!(find_app_bundle(&dir.path().join("empty")), None);
    }

    #[test]
    fn find_app_bundle_none_without_apps() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "x").unwrap();
        assert_eq!(find_app_bundle(dir.path()), None);
    }

    #[test]
    fn bundle_executable_layout() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join(app_bundle_name());
        assert_eq!(
            bundle_executable(&app),
            app.join("Contents/MacOS/momos-music-manager")
        );
    }
}
