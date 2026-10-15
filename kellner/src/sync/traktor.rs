//! Traktor `collection.nml` detection and mtime watch (issue #364, leaf P1-2).
//!
//! Detection order: an explicit override (path / `KELLNER_TRAKTOR_NML`), else a
//! platform default — macOS `~/Music/Native Instruments/Traktor*/collection.nml`,
//! Windows `%USERPROFILE%\Documents\Native Instruments\Traktor*/collection.nml`.
//! Linux has no default, so an override is required there. A missing file is
//! reported as `None`, never a panic.

use std::path::{Path, PathBuf};

/// Resolve the NML path to watch, or `None` when nothing was found.
///
/// An explicit `override_path` must exist to be used; the platform defaults are
/// probed only when no override is given.
pub fn detect(override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = override_path {
        return p.is_file().then(|| p.to_path_buf());
    }
    default_candidates().into_iter().next()
}

/// Platform default candidates, newest-modification first (may be empty).
fn default_candidates() -> Vec<PathBuf> {
    // `mut` is only exercised on macOS/Windows (the cfg branches below).
    #[allow(unused_mut)]
    let mut out = Vec::new();
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        collect_under(&home.join("Music").join("Native Instruments"), &mut out);
    }
    #[cfg(target_os = "windows")]
    if let Some(profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        collect_under(
            &profile.join("Documents").join("Native Instruments"),
            &mut out,
        );
    }
    out
}

/// Append `<base>/Traktor*/collection.nml` (newest first) to `out`.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn collect_under(base: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    let mut hits: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("Traktor") {
            continue;
        }
        let nml = entry.path().join("collection.nml");
        if nml.is_file() {
            let mtime = std::fs::metadata(&nml)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            hits.push((mtime, nml));
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    out.extend(hits.into_iter().map(|(_, p)| p));
}

/// mtime of `path` in nanoseconds since the Unix epoch (`None` when unstat-able).
pub fn mtime_nanos(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since.as_nanos()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_is_used_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let nml = dir.path().join("collection.nml");
        std::fs::write(&nml, "<NML/>").unwrap();
        assert_eq!(detect(Some(&nml)), Some(nml.clone()));
    }

    #[test]
    fn missing_override_reports_none() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.nml");
        assert_eq!(detect(Some(&missing)), None);
    }

    #[test]
    fn mtime_is_present_for_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x");
        std::fs::write(&f, "hi").unwrap();
        assert!(mtime_nanos(&f).is_some());
        assert!(mtime_nanos(&dir.path().join("absent")).is_none());
    }
}
