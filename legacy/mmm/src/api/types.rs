//! Shared API types used across multiple domain modules.
//! Domain-specific types live in their respective module files.

use axum::{Router, http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

// ── Response Wrapper ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiResponse<T> {
    pub data: T,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    pub error: String,
}

/// Helper that returns a 500 Internal Server Error JSON response from any Display error.
pub fn internal_error<E: std::fmt::Display>(e: E) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: e.to_string(),
        }),
    )
}

/// Append ORDER BY clause with whitelist validation.
/// Only allows known column names — safe from SQL injection.
pub fn apply_sort(
    sql: &mut String,
    sort: Option<&str>,
    order: Option<&str>,
    whitelist: &[&str],
    default: &str,
) {
    let sort_col = sort.filter(|s| whitelist.contains(s)).unwrap_or(default);
    let ord = match order {
        Some("desc") => "DESC",
        _ => "ASC",
    };
    sql.push_str(format!(" ORDER BY {} {}", sort_col, ord).as_str());
}

// ── Shared Entity Types ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub category: Option<String>,
    pub category_icon: Option<String>,
    pub created_at: Option<i64>,
    pub backpack: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCategory {
    pub id: i64,
    pub name: String,
    pub prefix: Option<String>,
    pub icon: String,
    pub is_default: bool,
    pub sort_order: i32,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub source_type: String,
    pub identifier: String,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub tags: Vec<Tag>,
    pub isrc: Option<String>,
    pub duration_ms: Option<i64>,
    pub rating: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarityMatch {
    pub candidate: Track,
    pub bpm_diff: f64,
    pub key_relationship: String,
    pub shared_tags: Vec<String>,
    pub similarity_score: f64,
}

// ── WebSocket / Callback Types ─────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub error: Option<String>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WebSocketEvent {
    NowPlaying {
        track: Track,
        position_ms: i64,
        is_playing: bool,
    },
    PlaybackState {
        is_playing: bool,
    },
    TokenExpired,
    ConnectionStatus {
        connected: bool,
        service: String,
    },
}

// ── Router builder trait / helper ─────────────────────────────────────

/// A domain module that contributes API routes.
/// Each domain module exposes `pub(super) fn router() -> Router<Arc<AppState>>`.
pub type DomainRouter = Router<Arc<AppState>>;
