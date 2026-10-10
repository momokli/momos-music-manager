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
    pub priority: i64,
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
}

#[derive(Debug, Deserialize)]
pub struct OrderItemRequest {
    pub isrc: String,
}

// ── URL-based orders (YouTube / SoundCloud) ──────────────────────────────────

/// A track row keyed by its source URL (no ISRC).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UrlTrack {
    pub id: i64,
    pub url: String,
    pub provider: String,
    pub provider_id: Option<String>,
    pub playlist_id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub state: String,
    pub source_format: Option<String>,
    pub path_flac: Option<String>,
    pub path_320: Option<String>,
    pub path_128: Option<String>,
    pub error: Option<String>,
    pub priority: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl UrlTrack {
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

/// A URL order: a batch of URLs placed by a consumer.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct UrlOrder {
    pub id: String,
    pub status: String,
    pub priority: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One URL inside an order, joined with its current track state.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UrlOrderItem {
    pub url: String,
    pub provider: Option<String>,
    pub provider_id: Option<String>,
    pub state: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub formats: Vec<String>,
    pub error: Option<String>,
}

/// `POST /url-orders` body.
#[derive(Debug, Deserialize)]
pub struct CreateUrlOrderRequest {
    pub items: Vec<UrlOrderItemRequest>,
}

#[derive(Debug, Deserialize)]
pub struct UrlOrderItemRequest {
    pub url: String,
}

/// Metadata for a single track, provider-agnostic.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataTrack {
    pub provider: String,
    pub provider_id: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub url: String,
    pub isrc: Option<String>,
    pub artwork: Option<String>,
}

/// Metadata for a playlist, provider-agnostic.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataPlaylist {
    pub provider: String,
    pub provider_id: String,
    pub name: Option<String>,
    pub track_count: usize,
    pub url: String,
    pub tracks: Vec<MetadataTrack>,
}
