//! Durable JSONL spool for telemetry events.
//!
//! Every event is appended as one JSON line (append-only, fsync'ed) before it
//! is flushed — so events **survive a restart**. Once the server acknowledged
//! a batch (or the client dropped it as permanently rejected), the
//! acknowledged events are removed from the spool via an atomic rewrite
//! (temp file + rename). A crash in between is safe: acknowledged events are
//! simply re-sent after restart and deduplicated server-side by `event_id`.
//!
//! Corrupt trailing lines (crash during an append) are skipped with a warning.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tracing::warn;

use super::events::TelemetryEvent;

/// File name of the spool inside the data dir.
pub const SPOOL_FILE: &str = "telemetry-events.jsonl";

/// Line-oriented append-only spool of serialized events.
#[derive(Debug, Clone)]
pub struct EventSpool {
    path: PathBuf,
}

impl EventSpool {
    /// Create a spool handle at `data_dir/telemetry-events.jsonl`.
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(SPOOL_FILE),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append events as JSONL (one event per line). Durable: writes are
    /// flushed + fsync'ed before returning.
    pub async fn append(&self, events: &[TelemetryEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("create spool dir {}", parent.display())
            })?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .with_context(|| format!("open spool {}", self.path.display()))?;
        let mut buf = Vec::new();
        for event in events {
            serde_json::to_writer(&mut buf, event).context("serialize spool event")?;
            buf.push(b'\n');
        }
        file.write_all(&buf)
            .await
            .with_context(|| format!("append to spool {}", self.path.display()))?;
        file.flush().await?;
        file.sync_data().await?;
        Ok(())
    }

    /// Load all spooled events (FIFO). Corrupt lines are skipped with a
    /// warning. Missing file → empty.
    pub async fn load(&self) -> Vec<TelemetryEvent> {
        let Ok(content) = tokio::fs::read_to_string(&self.path).await else {
            return Vec::new();
        };
        let mut events = Vec::new();
        for (idx, line) in content.lines().enumerate() {
            match serde_json::from_str::<TelemetryEvent>(line) {
                Ok(event) => events.push(event),
                Err(e) => warn!(
                    "telemetry spool: skipping corrupt line {} in {}: {e}",
                    idx + 1,
                    self.path.display()
                ),
            }
        }
        events
    }

    /// Drop all events except the ones with the given ids, atomically.
    /// Used after a successful flush (ACK) or a permanent drop: the spool is
    /// rewritten without the acknowledged/dropped events.
    pub async fn retain_ids(&self, keep_ids: &std::collections::HashSet<String>) -> Result<()> {
        let events = self.load().await;
        let remaining: Vec<TelemetryEvent> = events
            .into_iter()
            .filter(|e| keep_ids.contains(&e.event_id))
            .collect();
        self.replace_all(&remaining).await
    }

    /// Atomically replace the whole spool content (temp file + rename).
    pub async fn replace_all(&self, events: &[TelemetryEvent]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("create spool dir {}", parent.display())
            })?;
        }
        if events.is_empty() {
            // Removing the file is the cleanest "empty" state.
            let _ = tokio::fs::remove_file(&self.path).await;
            return Ok(());
        }
        let tmp = self.path.with_extension("jsonl.tmp");
        {
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&tmp)
                .await
                .with_context(|| format!("open spool tmp {}", tmp.display()))?;
            let mut buf = Vec::new();
            for event in events {
                serde_json::to_writer(&mut buf, event).context("serialize spool event")?;
                buf.push(b'\n');
            }
            file.write_all(&buf).await?;
            file.flush().await?;
            file.sync_data().await?;
        }
        tokio::fs::rename(&tmp, &self.path)
            .await
            .with_context(|| format!("rename spool tmp → {}", self.path.display()))?;
        Ok(())
    }

    /// Remove all spooled events (used when disabled at startup).
    pub async fn clear(&self) -> Result<()> {
        let _ = tokio::fs::remove_file(&self.path).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::events::EventType;

    fn ev(id: &str) -> TelemetryEvent {
        let mut e = TelemetryEvent::new(EventType::TaskCompleted, serde_json::json!({"n": 1}));
        e.event_id = id.to_string();
        e
    }

    #[tokio::test]
    async fn append_then_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let spool = EventSpool::in_data_dir(dir.path());
        spool.append(&[ev("a"), ev("b")]).await.unwrap();
        let loaded = spool.load().await;
        let ids: Vec<_> = loaded.iter().map(|e| e.event_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[tokio::test]
    async fn load_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let spool = EventSpool::in_data_dir(dir.path());
        assert!(spool.load().await.is_empty());
    }

    #[tokio::test]
    async fn events_survive_spool_recreation() {
        let dir = tempfile::tempdir().unwrap();
        {
            let spool = EventSpool::in_data_dir(dir.path());
            spool.append(&[ev("a")]).await.unwrap();
        }
        // "restart": new handle reads the same file
        let spool = EventSpool::in_data_dir(dir.path());
        let loaded = spool.load().await;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].event_id, "a");
    }

    #[tokio::test]
    async fn corrupt_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let spool = EventSpool::in_data_dir(dir.path());
        spool.append(&[ev("a")]).await.unwrap();
        // Append garbage directly to the file
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(spool.path())
            .await
            .unwrap()
            .write_all(b"{not-json}\n")
            .await
            .unwrap();
        let loaded = spool.load().await;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].event_id, "a");
    }

    #[tokio::test]
    async fn retain_ids_removes_acked_events() {
        let dir = tempfile::tempdir().unwrap();
        let spool = EventSpool::in_data_dir(dir.path());
        spool.append(&[ev("a"), ev("b"), ev("c")]).await.unwrap();

        let keep: std::collections::HashSet<String> =
            ["b", "c"].iter().map(|s| s.to_string()).collect();
        spool.retain_ids(&keep).await.unwrap();

        let loaded = spool.load().await;
        let ids: Vec<_> = loaded.iter().map(|e| e.event_id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);
    }

    #[tokio::test]
    async fn retain_ids_empty_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let spool = EventSpool::in_data_dir(dir.path());
        spool.append(&[ev("a")]).await.unwrap();
        spool
            .retain_ids(&std::collections::HashSet::new())
            .await
            .unwrap();
        assert!(!spool.path().exists());
        assert!(spool.load().await.is_empty());
    }

    #[tokio::test]
    async fn append_is_append_only() {
        let dir = tempfile::tempdir().unwrap();
        let spool = EventSpool::in_data_dir(dir.path());
        spool.append(&[ev("a")]).await.unwrap();
        spool.append(&[ev("b")]).await.unwrap();
        let loaded = spool.load().await;
        let ids: Vec<_> = loaded.iter().map(|e| e.event_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }
}
