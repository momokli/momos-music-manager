//! Minimal persisted sync state for kellner (issue #364, leaf P1-3).
//!
//! A tiny JSON file (`state.json`, sibling of `config.toml`) records what has
//! already been synced. The Traktor watcher stores the mtime of the last
//! successfully uploaded `collection.nml` so an unchanged file is **not**
//! re-uploaded. Deliberately no embedded database (no sqlx/rusqlite): the
//! `P0-2` `state.db` manifest is not part of this issue.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Persisted sync state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SyncState {
    /// mtime (nanoseconds since the Unix epoch) of the last uploaded NML.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_upload_mtime: Option<i64>,
    /// Orders placed against music-api, newest last (#366).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orders: Vec<OrderRecord>,
    /// Per-ISRC provenance of files already fetched into the collection (#366).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub downloads: BTreeMap<String, DownloadRecord>,
}

/// One `POST /orders` remembered for the status report (#366).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderRecord {
    /// Server order id.
    pub order_id: String,
    /// ISRCs in the order (sorted, deduplicated).
    pub isrcs: Vec<String>,
    /// Order priority at creation.
    #[serde(default)]
    pub priority: i64,
    /// Unix epoch seconds when kellner placed the order.
    pub created_at: i64,
    /// Last known per-ISRC state from polling (`ready`/`pending`/`failed`/…).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub states: BTreeMap<String, String>,
}

/// Provenance of one fetched file (#366).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadRecord {
    /// Absolute path of the flat target file (`<isrc>.<ext>`).
    pub path: PathBuf,
    /// Delivery format actually fetched (`flac` | `320` | `128`).
    pub format: String,
    /// Order this file was acquired under.
    pub order_id: String,
    /// Unix epoch seconds of the successful fetch.
    pub fetched_at: i64,
}

impl SyncState {
    /// `true` when the ISRC was already fetched and recorded (#366).
    pub fn is_fetched(&self, isrc: &str) -> bool {
        self.downloads.contains_key(isrc)
    }

    /// Remember a successful download (idempotent per ISRC).
    pub fn record_download(
        &mut self,
        isrc: &str,
        path: PathBuf,
        format: &str,
        order_id: &str,
        fetched_at: i64,
    ) {
        self.downloads.insert(
            isrc.to_string(),
            DownloadRecord {
                path,
                format: format.to_string(),
                order_id: order_id.to_string(),
                fetched_at,
            },
        );
    }

    /// Remember a placed order.
    pub fn record_order(&mut self, order: OrderRecord) {
        self.orders.push(order);
    }
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
    fn old_state_without_download_fields_still_parses() {
        // A state.json written by #364/#365 (only `last_upload_mtime`) must stay readable.
        let parsed: SyncState = serde_json::from_str(r#"{"last_upload_mtime":42}"#).unwrap();
        assert_eq!(parsed.last_upload_mtime, Some(42));
        assert!(parsed.orders.is_empty());
        assert!(parsed.downloads.is_empty());
        assert!(!parsed.is_fetched("ABCDEFGHIJKL"));
    }

    #[test]
    fn record_download_and_order_roundtrip() {
        let mut s = SyncState::default();
        s.record_order(OrderRecord {
            order_id: "o1".into(),
            isrcs: vec!["A".into()],
            priority: 3,
            created_at: 111,
            states: BTreeMap::from([("A".to_string(), "ready".to_string())]),
        });
        s.record_download("A", PathBuf::from("/music/A.flac"), "flac", "o1", 222);
        assert!(s.is_fetched("A"));
        assert!(!s.is_fetched("B"));

        let text = serde_json::to_string(&s).unwrap();
        let back: SyncState = serde_json::from_str(&text).unwrap();
        assert_eq!(back, s);
        let rec = back.downloads.get("A").unwrap();
        assert_eq!(rec.format, "flac");
        assert_eq!(rec.order_id, "o1");
        assert_eq!(rec.fetched_at, 222);
    }

    #[test]
    fn roundtrip_via_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kellner").join("state.json");
        let state = SyncState {
            last_upload_mtime: Some(42),
            ..SyncState::default()
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&state).unwrap()).unwrap();
        let loaded: SyncState =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded, state);
    }
}
