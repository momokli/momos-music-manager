//! Import of normalized playlist JSON (`spelunke.playlist-import/v1`).
//!
//! The Hub never talks to the source service on this path: an external producer
//! (offline script, reseller adapter, manual export) delivers the playlist as
//! JSON and the Hub only stores it. That keeps tokens, rate limits and ToS
//! questions outside the always-on server.
//!
//! The contract lives in `plans/proposed/apple-music-source.md` §3a — change it
//! there first, then here.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::api::AppState;

/// Wire schema identifier. Bumping it is a breaking change for every producer.
pub const SCHEMA: &str = "spelunke.playlist-import/v1";

/// Services this endpoint accepts. Deliberately an allowlist: a typo must fail
/// loudly instead of creating a parallel namespace in `hub_tracks`.
pub const ALLOWED_SOURCES: [&str; 1] = ["apple_music"];

/// Hard cap per request (SQLite bind limits + 2 MB body default).
const MAX_TRACKS: usize = 2000;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/hub/import/playlist", post(import_playlist))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub schema: String,
    pub source: String,
    pub playlist: PlaylistIn,
    #[serde(default)]
    pub tracks: Vec<TrackIn>,
}

#[derive(Debug, Deserialize)]
pub struct PlaylistIn {
    pub remote_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub storefront: Option<String>,
    #[serde(default)]
    pub fetched_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TrackIn {
    pub remote_track_id: String,
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub artists: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub explicit: Option<bool>,
    #[serde(default)]
    pub image_url: Option<String>,
}

/// Upper-case, strip separators, and accept only a well-formed 12-char ISRC.
/// Anything else becomes `None` — a single broken value must not fail a playlist.
fn normalize_isrc(raw: Option<&str>) -> Option<String> {
    let cleaned: String = raw?
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase();
    if cleaned.len() == 12 {
        Some(cleaned)
    } else {
        None
    }
}

const NOW_SQL: &str = "SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')";

async fn now_iso(pool: &SqlitePool) -> String {
    sqlx::query_scalar::<_, String>(NOW_SQL)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|_| Utc::now().to_rfc3339())
}

/// `POST /api/hub/import/playlist` — store a normalized playlist for the current
/// user. Idempotent: re-importing the same `remote_id` updates in place.
async fn import_playlist(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ImportRequest>,
) -> Response {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return err(StatusCode::UNAUTHORIZED, "login required");
    };

    if req.schema.trim() != SCHEMA {
        return err(
            StatusCode::BAD_REQUEST,
            format!("unsupported schema {SCHEMA} expected, got {:?}", req.schema),
        );
    }
    let source = req.source.trim().to_string();
    if !ALLOWED_SOURCES.contains(&source.as_str()) {
        return err(
            StatusCode::BAD_REQUEST,
            format!(
                "unsupported source {:?} (allowed: {})",
                source,
                ALLOWED_SOURCES.join(", ")
            ),
        );
    }
    let remote_id = req.playlist.remote_id.trim().to_string();
    if remote_id.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "playlist.remote_id must not be empty",
        );
    }
    if req.tracks.len() > MAX_TRACKS {
        return err(
            StatusCode::BAD_REQUEST,
            format!("too many tracks ({} > {MAX_TRACKS})", req.tracks.len()),
        );
    }

    match store(&st.pool, &st.cfg, user_id, &source, &req).await {
        Ok(stats) => (StatusCode::OK, Json(json!({ "data": stats }))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Default)]
struct Stats {
    playlist_id: i64,
    tracks: usize,
    with_isrc: usize,
    without_isrc: usize,
    invalid_isrc: usize,
    duplicates: usize,
    skipped: usize,
    touched: usize,
}

impl Stats {
    fn to_json(&self) -> Value {
        json!({
            "playlistId": self.playlist_id,
            "tracks": self.tracks,
            "withIsrc": self.with_isrc,
            "withoutIsrc": self.without_isrc,
            "invalidIsrc": self.invalid_isrc,
            "duplicates": self.duplicates,
            "skipped": self.skipped,
            "touched": self.touched,
        })
    }
}

async fn store(
    pool: &SqlitePool,
    cfg: &crate::config::Config,
    user_id: i64,
    source: &str,
    req: &ImportRequest,
) -> anyhow::Result<Value> {
    let now = now_iso(pool).await;
    let fetched_at = req
        .playlist
        .fetched_at
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| now.clone());

    let mut stats = Stats::default();

    // Dedupe first-wins: `hub_playlist_tracks` is keyed by (playlist_id, track_id),
    // so a repeated id would otherwise collapse silently with a later position.
    let mut seen: Vec<&str> = Vec::with_capacity(req.tracks.len());
    let mut ordered: Vec<(i64, &TrackIn)> = Vec::with_capacity(req.tracks.len());
    for (idx, t) in req.tracks.iter().enumerate() {
        let id = t.remote_track_id.trim();
        if id.is_empty() {
            stats.skipped += 1;
            continue;
        }
        if seen.contains(&id) {
            stats.duplicates += 1;
            continue;
        }
        seen.push(id);
        ordered.push((t.position.unwrap_or(idx as i64), t));
    }
    ordered.sort_by_key(|(pos, _)| *pos);

    let playlist_id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_playlists
             (user_id, service, playlist_id, name, description, track_count, fetched_at,
              items_available, enabled_for_fetch, is_owned, fetch_status, fetch_error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 0, 1, 'ok', NULL)
         ON CONFLICT(user_id, service, playlist_id) DO UPDATE SET
             name            = COALESCE(excluded.name, hub_playlists.name),
             description     = COALESCE(excluded.description, hub_playlists.description),
             track_count     = excluded.track_count,
             fetched_at      = excluded.fetched_at,
             items_available = 1,
             fetch_status    = 'ok',
             fetch_error     = NULL
         RETURNING id",
    )
    .bind(user_id)
    .bind(source)
    .bind(&req.playlist.remote_id.trim())
    .bind(req.playlist.name.as_deref())
    .bind(req.playlist.description.as_deref())
    .bind(ordered.len() as i64)
    .bind(&fetched_at)
    .fetch_one(pool)
    .await?;
    stats.playlist_id = playlist_id;

    sqlx::query("DELETE FROM hub_playlist_tracks WHERE playlist_id = ?1")
        .bind(playlist_id)
        .execute(pool)
        .await?;

    let mut touch_ids: Vec<i64> = Vec::with_capacity(ordered.len());
    let mut position: i64 = 0;

    for (_, t) in &ordered {
        let raw_isrc = t.isrc.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let isrc = normalize_isrc(raw_isrc);
        if raw_isrc.is_some() && isrc.is_none() {
            // Provided but unusable (wrong length/charsetter) — counted, stored as NULL.
            stats.invalid_isrc += 1;
        }

        let track_id: i64 = sqlx::query_scalar(
            "INSERT INTO hub_tracks
                 (service, service_track_id, isrc, title, artists, album, duration_ms,
                  explicit, image_url, first_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(service, service_track_id) DO UPDATE SET
                 isrc        = COALESCE(excluded.isrc, hub_tracks.isrc),
                 title       = COALESCE(excluded.title, hub_tracks.title),
                 artists     = COALESCE(excluded.artists, hub_tracks.artists),
                 album       = COALESCE(excluded.album, hub_tracks.album),
                 duration_ms = COALESCE(excluded.duration_ms, hub_tracks.duration_ms),
                 explicit    = COALESCE(excluded.explicit, hub_tracks.explicit),
                 image_url   = COALESCE(excluded.image_url, hub_tracks.image_url)
             RETURNING id",
        )
        .bind(source)
        .bind(t.remote_track_id.trim())
        .bind(isrc.as_deref())
        .bind(t.title.as_deref())
        .bind(t.artists.as_deref())
        .bind(t.album.as_deref())
        .bind(t.duration_ms)
        .bind(t.explicit.map(|b| if b { 1_i64 } else { 0_i64 }))
        .bind(t.image_url.as_deref())
        .bind(&now)
        .fetch_one(pool)
        .await?;

        if let Some(url) = req.playlist.url.as_deref() {
            let _ = sqlx::query(
                "INSERT INTO hub_track_external_ids
                     (track_id, service, external_id, url, source, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, 'playlist-import', ?5)
                 ON CONFLICT(track_id, service, external_id) DO NOTHING",
            )
            .bind(track_id)
            .bind(source)
            .bind(t.remote_track_id.trim())
            .bind(url)
            .bind(&now)
            .execute(pool)
            .await;
        }

        sqlx::query(
            "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?1, ?2, ?3, NULL)
             ON CONFLICT(playlist_id, track_id) DO NOTHING",
        )
        .bind(playlist_id)
        .bind(track_id)
        .bind(position)
        .execute(pool)
        .await?;

        position += 1;
        stats.tracks += 1;
        if isrc.is_some() {
            stats.with_isrc += 1;
            touch_ids.push(track_id);
        } else {
            stats.without_isrc += 1;
        }
    }

    // Only tracks with an ISRC are worth touching: `order_touched` filters on
    // `isrc IS NOT NULL` anyway (crawl.rs:116-118).
    if !touch_ids.is_empty() {
        crate::crawl::touch_tracks(pool, cfg, &touch_ids).await?;
        stats.touched = touch_ids.len();
    }

    Ok(stats.to_json())
}

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}
