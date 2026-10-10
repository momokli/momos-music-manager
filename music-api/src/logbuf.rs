//! Small in-memory ring buffer of recent service events, surfaced via `GET /logs`
//! so the hub can show a live activity feed (orders, resolutions, downloads).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    /// Unix seconds.
    pub at: i64,
    /// `info` | `warn` | `error`.
    pub level: String,
    pub msg: String,
}

/// Bounded, cheap-to-clone handle to the shared log buffer.
#[derive(Clone, Default)]
pub struct LogBuffer {
    inner: Arc<Mutex<VecDeque<LogEntry>>>,
}

const CAP: usize = 500;

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(CAP))),
        }
    }

    pub fn push(&self, level: &str, msg: impl Into<String>) {
        let entry = LogEntry {
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            level: level.to_string(),
            msg: msg.into(),
        };
        if let Ok(mut q) = self.inner.lock() {
            if q.len() >= CAP {
                q.pop_front();
            }
            q.push_back(entry);
        }
    }

    /// Newest first, up to `limit`.
    pub fn recent(&self, limit: usize) -> Vec<LogEntry> {
        match self.inner.lock() {
            Ok(q) => q.iter().rev().take(limit).cloned().collect(),
            Err(_) => Vec::new(),
        }
    }
}
