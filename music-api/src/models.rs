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

/// A track row keyed by `isrc` (the primary key of the whole system). For
/// non-Deezer sources the key is `<source>:<external_id>` (e.g.
/// `soundcloud:1837565340`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub isrc: String,
    /// `deezer` | `soundcloud` | …
    #[serde(default)]
    pub source: String,
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
    /// Acquisition source of this track (from its key, fallback to the column).
    pub fn source(&self) -> &str {
        if self.source.is_empty() {
            track_source(&self.isrc)
        } else {
            &self.source
        }
    }

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
    /// Deezer/ISRC path (kept optional so non-Deezer sources can omit it).
    #[serde(default)]
    pub isrc: Option<String>,
    /// Non-Deezer source, e.g. `soundcloud`.
    #[serde(default)]
    pub source: Option<String>,
    /// External id within that source (alias `externalId`).
    #[serde(default, alias = "externalId")]
    pub external_id: Option<String>,
}

impl OrderItemRequest {
    /// The universal track key: a bare ISRC for Deezer, `<source>:<id>` else.
    pub fn key(&self) -> Option<String> {
        match (self.source.as_deref(), self.external_id.as_deref()) {
            (Some(s), Some(id)) if !s.trim().is_empty() && !s.eq_ignore_ascii_case("deezer") => {
                let id = id.trim();
                (!id.is_empty()).then(|| format!("{}:{id}", s.trim().to_lowercase()))
            }
            _ => self
                .isrc
                .as_deref()
                .map(crate::db::normalize_key)
                .filter(|s| !s.is_empty()),
        }
    }
}

/// Acquisition source encoded in a track key (`deezer` when there is no
/// non-Deezer prefix).
pub fn track_source(key: &str) -> &str {
    match key.split_once(':') {
        Some((s, _)) if !s.is_empty() && !s.eq_ignore_ascii_case("deezer") => s,
        _ => "deezer",
    }
}

/// The external id part of a track key (the whole key for a bare ISRC).
pub fn track_external_id(key: &str) -> &str {
    match key.split_once(':') {
        Some((_, id)) if !id.is_empty() => id,
        _ => key,
    }
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
