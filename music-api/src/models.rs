//! Wire + storage models.

use serde::{Deserialize, Serialize};

/// Lifecycle of a single ISRC.
pub mod state {
    pub const PENDING: &str = "pending";
    pub const DOWNLOADING: &str = "downloading";
    pub const READY: &str = "ready";
    pub const ABSENT: &str = "absent";
    pub const FAILED: &str = "failed";
}

/// A track row keyed by ISRC (the primary key of the whole system).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub isrc: String,
    pub deezer_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub state: String,
    /// What deemix actually delivered: `flac` or `mp3`.
    pub source_format: Option<String>,
    pub deemix_uuid: Option<String>,
    pub path_flac: Option<String>,
    pub path_320: Option<String>,
    pub path_128: Option<String>,
    pub error: Option<String>,
    /// Transient failures so far (bounded retries).
    pub attempts: i64,
    /// `pending` rows become due at this epoch; `NULL` = due now.
    pub next_attempt_at: Option<i64>,
    /// Set while `downloading`, refreshed as a heartbeat; drives the reaper.
    pub lease_expires_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Track {
    /// Formats that can currently be served for this track.
    pub fn formats(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.path_flac.is_some() {
            v.push("flac");
        }
        if self.path_320.is_some() {
            v.push("320");
        }
        if self.path_128.is_some() {
            v.push("128");
        }
        v
    }

    pub fn path_for(&self, format: &str) -> Option<&str> {
        match format {
            "flac" => self.path_flac.as_deref(),
            "320" => self.path_320.as_deref(),
            "128" => self.path_128.as_deref(),
            _ => None,
        }
    }
}

/// An order: a batch of ISRCs placed by a consumer (MMM).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub id: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Higher = processed earlier.
    pub priority: i64,
}

/// One ISRC inside an order, joined with its current track state.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderItem {
    pub isrc: String,
    pub state: String,
    pub deezer_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub formats: Vec<String>,
    pub error: Option<String>,
}

/// `POST /orders` body.
#[derive(Debug, Deserialize)]
pub struct CreateOrderRequest {
    pub items: Vec<OrderItemRequest>,
    /// Higher = processed earlier (default 0).
    #[serde(default)]
    pub priority: i64,
}

#[derive(Debug, Deserialize)]
pub struct OrderItemRequest {
    pub isrc: String,
}

// ── Per-ISRC metadata (BPM / key authority) ──────────────────────────────────

/// One metadata push for an ISRC (hub / ingest). Non-null fields win on merge.
#[derive(Debug, Clone, Deserialize)]
pub struct MetadataItem {
    pub isrc: String,
    #[serde(default)]
    pub bpm: Option<f64>,
    /// Accepts `key` or `musicalKey` from callers.
    #[serde(default, alias = "musicalKey")]
    pub key: Option<String>,
    #[serde(default)]
    pub camelot: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

/// `POST /metadata` body.
#[derive(Debug, Deserialize)]
pub struct MetadataPushRequest {
    pub items: Vec<MetadataItem>,
}

/// Stored per-ISRC metadata (`GET /metadata`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TrackMetadata {
    pub isrc: String,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub camelot: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub source: Option<String>,
    pub updated_at: i64,
}
