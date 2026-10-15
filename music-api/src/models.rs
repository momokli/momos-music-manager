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

/// Lifecycle of an artist order (whole-discography download).
pub mod artist_state {
    /// Created; not yet accepted by deemix (or deemix was not ready).
    pub const OPEN: &str = "open";
    /// Submitted to deemix, download in progress.
    pub const RUNNING: &str = "running";
    /// deemix finished and the produced files were ingested into the vault.
    pub const DONE: &str = "done";
    /// deemix rejected it, or the download/ingest failed.
    pub const FAILED: &str = "failed";
}

/// A track row keyed by `isrc` (the primary key of the whole system). For
/// non-Deezer sources the key is `<source>:<external_id>` (e.g.
/// `soundcloud:1837565340`, `youtube:dQw4w9WgXcQ`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub isrc: String,
    /// `deezer` | `soundcloud` | `youtube` | …
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

/// An artist order: download a whole Deezer artist's discography via deemix.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ArtistOrder {
    pub id: String,
    /// The artist as supplied by the caller (id or URL).
    pub artist: String,
    /// The canonical Deezer artist URL handed to deemix.
    pub artist_url: String,
    pub status: String,
    /// Higher = processed earlier.
    pub priority: i64,
    /// The deemix queue handle to poll while `running`.
    pub deemix_uuid: Option<String>,
    /// How many files were ingested into the vault.
    pub added: i64,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// `POST /artist-orders` body.
#[derive(Debug, Deserialize)]
pub struct CreateArtistOrderRequest {
    /// Deezer artist id (`12345`) or URL (`https://www.deezer.com/artist/12345`).
    pub artist: String,
    /// Higher = processed earlier (default 0).
    #[serde(default)]
    pub priority: i64,
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
    /// Non-Deezer source, e.g. `soundcloud` or `youtube`.
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
    /// Output-only alias of `musicalKey` (the analyzer's field name for the same
    /// value), echoed for consumers that speak the analyzer contract. Not a
    /// column; filled by [`TrackMetadata::with_key_alias`].
    #[sqlx(default)]
    pub key: Option<String>,
}

impl TrackMetadata {
    /// Populate the `key` output alias from `musical_key` (same value).
    pub fn with_key_alias(mut self) -> Self {
        self.key = self.musical_key.clone();
        self
    }
}
