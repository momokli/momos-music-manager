//! Minimal persisted sync state for kellner (issue #364, leaf P1-3).
//!
//! A tiny JSON file (`state.json`, sibling of `config.toml`) records what has
//! already been synced. The Traktor watcher stores the mtime of the last
//! successfully uploaded `collection.nml` so an unchanged file is **not**
//! re-uploaded. Deliberately no embedded database (no sqlx/rusqlite): the
//! `P0-2` `state.db` manifest is not part of this issue.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Persisted sync state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SyncState {
    /// mtime (nanoseconds since the Unix epoch) of the last uploaded NML.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_upload_mtime: Option<i64>,
}

/// `state.json` next to the resolved `config.toml`.
pub fn state_path() -> Result<PathBuf> {
    let cfg = crate::config::config_path()?;
    Ok(cfg.with_file_name("state.json"))
}

/// Load the state, or the empty default when the file does not exist yet.
pub fn load() -> Result<SyncState> {
    let path = state_path()?;
    if !path.exists() {
        return Ok(SyncState::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading state {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing state {}", path.display()))
}

/// Persist the state (creating the directory if needed).
pub fn save(state: &SyncState) -> Result<()> {
    let path = state_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating state directory {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(state).context("serialising state")?;
    std::fs::write(&path, text).with_context(|| format!("writing state {}", path.display()))?;
    Ok(())
}

/// True when `mtime` differs from the last uploaded mtime (i.e. an upload is due).
pub fn needs_upload(state: &SyncState, mtime: i64) -> bool {
    state.last_upload_mtime != Some(mtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_upload_only_when_mtime_changed() {
        let mut s = SyncState::default();
        assert!(needs_upload(&s, 100), "nothing uploaded yet");
        s.last_upload_mtime = Some(100);
        assert!(!needs_upload(&s, 100), "same mtime -> skip");
        assert!(needs_upload(&s, 101), "changed mtime -> upload");
    }

    #[test]
    fn roundtrip_via_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kellner").join("state.json");
        let state = SyncState {
            last_upload_mtime: Some(42),
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&state).unwrap()).unwrap();
        let loaded: SyncState =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded, state);
    }
}
