//! Per-ISRC metadata (Part C): the merged record served by `GET /metadata/{isrc}`.
//!
//! Fed from two sides: the ingest (Part A/B) writes what the file tags carry
//! (ISRC, key, title, …), and the hub pushes BPM/key it holds (Traktor
//! `collection.nml`, MMM data, its own analysis). Merge rule: a non-null field
//! wins over null; `source` records the last writer.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Sqlite};

use crate::AppState;
use crate::store::ImportMeta;

/// A merged metadata record keyed by ISRC.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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

/// `POST /metadata` body.
#[derive(Debug, Deserialize)]
pub struct UpsertRequest {
    pub items: Vec<MetadataItem>,
}

/// One metadata item. `key` is the musical key (stored as `musical_key`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataItem {
    pub isrc: String,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub camelot: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub source: Option<String>,
}

/// Upsert one item; non-null fields win, nulls keep the existing value.
pub async fn upsert(pool: &Pool<Sqlite>, item: &MetadataItem) -> anyhow::Result<()> {
    let isrc = crate::db::normalize_isrc(&item.isrc);
    if isrc.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO track_metadata
           (isrc, bpm, musical_key, camelot, title, artist, album, genre, source, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(isrc) DO UPDATE SET
           bpm         = COALESCE(excluded.bpm, track_metadata.bpm),
           musical_key = COALESCE(excluded.musical_key, track_metadata.musical_key),
           camelot     = COALESCE(excluded.camelot, track_metadata.camelot),
           title       = COALESCE(excluded.title, track_metadata.title),
           artist      = COALESCE(excluded.artist, track_metadata.artist),
           album       = COALESCE(excluded.album, track_metadata.album),
           genre       = COALESCE(excluded.genre, track_metadata.genre),
           source      = COALESCE(excluded.source, track_metadata.source),
           updated_at  = excluded.updated_at",
    )
    .bind(&isrc)
    .bind(item.bpm)
    .bind(&item.key)
    .bind(&item.camelot)
    .bind(&item.title)
    .bind(&item.artist)
    .bind(&item.album)
    .bind(&item.genre)
    .bind(&item.source)
    .bind(crate::db::now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Upsert from an ingested file's tags (Part A/B). Source is `stem-tags`.
pub async fn upsert_from_ingest(
    pool: &Pool<Sqlite>,
    isrc: &str,
    meta: &ImportMeta,
) -> anyhow::Result<()> {
    let item = MetadataItem {
        isrc: isrc.to_string(),
        bpm: meta.bpm,
        key: meta.musical_key.clone(),
        camelot: None,
        title: meta.title.clone(),
        artist: meta.artist.clone(),
        album: meta.album.clone(),
        genre: meta.genre.clone(),
        source: Some("stem-tags".to_string()),
    };
    upsert(pool, &item).await
}

pub async fn get(pool: &Pool<Sqlite>, isrc: &str) -> anyhow::Result<Option<TrackMetadata>> {
    let isrc = crate::db::normalize_isrc(isrc);
    Ok(
        sqlx::query_as::<_, TrackMetadata>("SELECT * FROM track_metadata WHERE isrc = ?1")
            .bind(isrc)
            .fetch_optional(pool)
            .await?,
    )
}

// ── Handlers ───────────────────────────────────────────────────────────────

/// `POST /metadata` — bulk upsert (the hub pushes BPM/key here).
pub async fn post_metadata(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpsertRequest>,
) -> Response {
    let mut upserted = 0usize;
    for item in &req.items {
        match upsert(&state.pool, item).await {
            Ok(()) => upserted += 1,
            Err(e) => tracing::warn!("metadata upsert failed for {}: {e:#}", item.isrc),
        }
    }
    Json(serde_json::json!({ "upserted": upserted })).into_response()
}

/// `GET /metadata/{isrc}` — the merged record.
pub async fn get_metadata(
    State(state): State<Arc<AppState>>,
    UrlPath(isrc): UrlPath<String>,
) -> Response {
    match get(&state.pool, &isrc).await {
        Ok(Some(m)) => Json(m).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "unknown ISRC" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
