//! Stable per-installation client id for event telemetry.
//!
//! The id is a UUID v4, generated **once** and persisted in the app data dir
//! (`~/.local/share/momos-music-manager/telemetry-client-id`). It survives
//! restarts and app updates and replaces the snapshot telemetry's
//! `instance` string for events (no hostname PII on the wire).
//!
//! Concurrency: a fresh id is written atomically (tmp file + rename), so two
//! racing processes both end up with one consistent file.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{info, warn};

use super::events::valid_client_id;

/// File name inside the app data dir that holds the client id.
pub const CLIENT_ID_FILE: &str = "telemetry-client-id";

/// Default app data dir: `~/.local/share/momos-music-manager` (same location
/// as the library DB and logs). Falls back to the current dir when no home
/// dir can be determined.
pub fn data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/momos-music-manager")
}

/// Load the persisted client id, generating + persisting a fresh UUID v4 on
/// first use. Never returns an invalid id: a corrupt/unparseable file is
/// replaced by a new id.
pub fn load_or_create() -> Result<String> {
    load_or_create_in(&data_dir())
}

/// Like [`load_or_create`] but with an explicit data dir (tests use temp dirs).
pub fn load_or_create_in(data_dir: &Path) -> Result<String> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("create telemetry data dir {}", data_dir.display()))?;
    let path = data_dir.join(CLIENT_ID_FILE);

    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim().to_string();
        if valid_client_id(&existing) {
            return Ok(existing);
        }
        warn!(
            "telemetry client id file at {} is invalid — regenerating",
            path.display()
        );
    }

    let id = uuid::Uuid::new_v4().to_string();
    persist(&path, &id)?;
    info!(
        "telemetry client id generated + persisted at {}",
        path.display()
    );
    Ok(id)
}

/// Atomically write the client id (tmp file in the same dir + rename).
fn persist(path: &Path, id: &str) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!("{CLIENT_ID_FILE}.tmp"));
    std::fs::write(&tmp, format!("{id}\n"))
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_and_persists_once() {
        let dir = tempfile::tempdir().unwrap();
        let a = load_or_create_in(dir.path()).unwrap();
        let b = load_or_create_in(dir.path()).unwrap();
        assert_eq!(a, b, "client id must be stable across calls");
        assert!(valid_client_id(&a));
        let file = dir.path().join(CLIENT_ID_FILE);
        assert!(file.exists());
        assert_eq!(std::fs::read_to_string(file).unwrap().trim(), a);
    }

    #[test]
    fn generated_id_is_uuid_v4() {
        let dir = tempfile::tempdir().unwrap();
        let id = load_or_create_in(dir.path()).unwrap();
        let uuid = uuid::Uuid::parse_str(&id).unwrap();
        assert_eq!(uuid.get_version_num(), 4);
    }

    #[test]
    fn replaces_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CLIENT_ID_FILE);
        std::fs::write(&path, "../evil/path\n").unwrap();
        let id = load_or_create_in(dir.path()).unwrap();
        assert!(valid_client_id(&id));
        assert!(!id.contains('/'));
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), id);
    }

    #[test]
    fn replaces_traversal_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CLIENT_ID_FILE);
        std::fs::write(&path, "a/b\n").unwrap();
        let id = load_or_create_in(dir.path()).unwrap();
        assert!(valid_client_id(&id));
    }

    #[test]
    fn keeps_existing_valid_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CLIENT_ID_FILE);
        std::fs::write(&path, "3f2a1b2c-1234-5678-9abc-def012345678\n").unwrap();
        let id = load_or_create_in(dir.path()).unwrap();
        assert_eq!(id, "3f2a1b2c-1234-5678-9abc-def012345678");
    }

    #[test]
    fn data_dir_points_into_local_share() {
        let d = data_dir();
        let s = d.to_string_lossy();
        assert!(s.contains("momos-music-manager"), "got {s}");
    }
}
