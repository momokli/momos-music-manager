//! File-track correction API: handlers for `/api/files/{id}/track-corrections`,
//! `/api/tracks/{id}/file-corrections`, and `/api/file-track-corrections/{id}` endpoints.
//!
//! Corrections override the automatic file↔track linking in `v_file_track_link`.
//! - `include`: explicitly link a file to a track (wins over automatic ISRC matching)
//! - `exclude`: prevent automatic linking of a file to a track

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, put},
};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, internal_error};

// ── Types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CorrectionInput {
    track_id: i64,
    link_type: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PutCorrectionsRequest {
    corrections: Vec<CorrectionInput>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrackLinkInfo {
    track_id: i64,
    title: String,
    artist: String,
    service: String,
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileLinkInfo {
    file_id: i64,
    file_path: String,
    file_type: String,
    title: Option<String>,
    artist: Option<String>,
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileCorrectionsState {
    file_id: i64,
    automatic_links: Vec<TrackLinkInfo>,
    manual_includes: Vec<TrackLinkInfo>,
    manual_excludes: Vec<TrackLinkInfo>,
    effective_links: Vec<TrackLinkInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrackCorrectionsState {
    track_id: i64,
    automatic_links: Vec<FileLinkInfo>,
    manual_includes: Vec<FileLinkInfo>,
    manual_excludes: Vec<FileLinkInfo>,
    effective_links: Vec<FileLinkInfo>,
}

// ── Router ───────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/files/{id}/track-corrections",
            get(file_track_corrections_get).put(file_track_corrections_put),
        )
        .route(
            "/api/tracks/{id}/file-corrections",
            get(track_file_corrections_get).put(track_file_corrections_put),
        )
        .route(
            "/api/file-track-corrections/{id}",
            delete(correction_delete),
        )
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Compute the automatic links that would exist without any corrections.
async fn compute_automatic_links_file(
    pool: &Pool<Sqlite>,
    file_id: i64,
) -> Result<Vec<TrackLinkInfo>, anyhow::Error> {
    // Verify file exists
    let file_exists: bool = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE id = ?")
        .bind(file_id)
        .fetch_one(pool)
        .await
        .map(|c: i64| c > 0)?;

    if !file_exists {
        anyhow::bail!("File not found");
    }

    let rows = sqlx::query(
        r#"SELECT st.id, st.title, st.artist, st.service
           FROM service_tracks st
           JOIN files f ON (
               st.isrc = f.isrc
               OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
               OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
               OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
               OR (st.service = 'local' AND st.service_id = CAST(f.id AS TEXT))
           )
           WHERE f.id = ?
           ORDER BY st.id"#,
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| TrackLinkInfo {
            track_id: r.get("id"),
            title: r.get("title"),
            artist: r.get("artist"),
            service: r.get("service"),
            reason: "auto".to_string(),
        })
        .collect())
}

/// Compute the automatic links that would exist without any corrections (from track perspective).
async fn compute_automatic_links_track(
    pool: &Pool<Sqlite>,
    track_id: i64,
) -> Result<Vec<FileLinkInfo>, anyhow::Error> {
    // Verify track exists
    let track_exists: bool = sqlx::query_scalar("SELECT COUNT(*) FROM service_tracks WHERE id = ?")
        .bind(track_id)
        .fetch_one(pool)
        .await
        .map(|c: i64| c > 0)?;

    if !track_exists {
        anyhow::bail!("Track not found");
    }

    let rows = sqlx::query(
        r#"SELECT f.id, f.file_path, f.file_type, f.title, f.artist
           FROM files f
           JOIN service_tracks st ON (
               st.isrc = f.isrc
               OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
               OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
               OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
               OR (st.service = 'local' AND st.service_id = CAST(f.id AS TEXT))
           )
           WHERE st.id = ?
           ORDER BY f.id"#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| FileLinkInfo {
            file_id: r.get("id"),
            file_path: r.get("file_path"),
            file_type: r.get("file_type"),
            title: r.get("title"),
            artist: r.get("artist"),
            reason: "auto".to_string(),
        })
        .collect())
}

/// Fetch manual corrections for a file and convert to TrackLinkInfo.
async fn manual_corrections_for_file(
    pool: &Pool<Sqlite>,
    file_id: i64,
    link_type: &str,
) -> Result<Vec<TrackLinkInfo>, anyhow::Error> {
    let rows = sqlx::query(
        r#"SELECT ftc.track_id, ftc.reason,
                  st.title, st.artist, st.service
           FROM file_track_corrections ftc
           JOIN service_tracks st ON st.id = ftc.track_id
           WHERE ftc.file_id = ? AND ftc.link_type = ?
           ORDER BY ftc.track_id"#,
    )
    .bind(file_id)
    .bind(link_type)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| TrackLinkInfo {
            track_id: r.get("track_id"),
            title: r.get("title"),
            artist: r.get("artist"),
            service: r.get("service"),
            reason: r
                .get::<Option<String>, _>("reason")
                .unwrap_or_else(|| link_type.to_string()),
        })
        .collect())
}

/// Fetch manual corrections for a track and convert to FileLinkInfo.
async fn manual_corrections_for_track(
    pool: &Pool<Sqlite>,
    track_id: i64,
    link_type: &str,
) -> Result<Vec<FileLinkInfo>, anyhow::Error> {
    let rows = sqlx::query(
        r#"SELECT ftc.file_id, ftc.reason,
                  f.file_path, f.file_type, f.title, f.artist
           FROM file_track_corrections ftc
           JOIN files f ON f.id = ftc.file_id
           WHERE ftc.track_id = ? AND ftc.link_type = ?
           ORDER BY ftc.file_id"#,
    )
    .bind(track_id)
    .bind(link_type)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| FileLinkInfo {
            file_id: r.get("file_id"),
            file_path: r.get("file_path"),
            file_type: r.get("file_type"),
            title: r.get("title"),
            artist: r.get("artist"),
            reason: r
                .get::<Option<String>, _>("reason")
                .unwrap_or_else(|| link_type.to_string()),
        })
        .collect())
}

/// Compute effective links for a file (what v_file_track_link actually returns).
async fn compute_effective_links_file(
    pool: &Pool<Sqlite>,
    file_id: i64,
) -> Result<Vec<TrackLinkInfo>, anyhow::Error> {
    let rows = sqlx::query(
        r#"SELECT v.track_id, st.title, st.artist, st.service
           FROM v_file_track_link v
           JOIN service_tracks st ON st.id = v.track_id
           WHERE v.file_id = ?
           ORDER BY v.track_id"#,
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| TrackLinkInfo {
            track_id: r.get("track_id"),
            title: r.get("title"),
            artist: r.get("artist"),
            service: r.get("service"),
            reason: "effective".to_string(),
        })
        .collect())
}

/// Compute effective links for a track (what v_file_track_link actually returns).
async fn compute_effective_links_track(
    pool: &Pool<Sqlite>,
    track_id: i64,
) -> Result<Vec<FileLinkInfo>, anyhow::Error> {
    let rows = sqlx::query(
        r#"SELECT v.file_id, f.file_path, f.file_type, f.title, f.artist
           FROM v_file_track_link v
           JOIN files f ON f.id = v.file_id
           WHERE v.track_id = ?
           ORDER BY v.file_id"#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| FileLinkInfo {
            file_id: r.get("file_id"),
            file_path: r.get("file_path"),
            file_type: r.get("file_type"),
            title: r.get("title"),
            artist: r.get("artist"),
            reason: "effective".to_string(),
        })
        .collect())
}

// ── File Endpoints ───────────────────────────────────────────────────────

/// GET /api/files/{id}/track-corrections — return the full correction state for a file.
pub async fn file_track_corrections_get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let file_id = id;

    // Verify file exists
    let file_exists: bool = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE id = ?")
        .bind(file_id)
        .fetch_one(&state.db)
        .await
        .map(|c: i64| c > 0)
        .unwrap_or(false);
    if !file_exists {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("File not found with id: {}", file_id),
            }),
        )
            .into_response();
    }

    let automatic = compute_automatic_links_file(&state.db, file_id)
        .await
        .unwrap_or_default();
    let manual_includes = manual_corrections_for_file(&state.db, file_id, "include")
        .await
        .unwrap_or_default();
    let manual_excludes = manual_corrections_for_file(&state.db, file_id, "exclude")
        .await
        .unwrap_or_default();
    let effective = compute_effective_links_file(&state.db, file_id)
        .await
        .unwrap_or_default();

    let data = FileCorrectionsState {
        file_id,
        automatic_links: automatic,
        manual_includes,
        manual_excludes,
        effective_links: effective,
    };

    (StatusCode::OK, Json(ApiResponse { data })).into_response()
}

/// PUT /api/files/{id}/track-corrections — upsert corrections for a file.
pub async fn file_track_corrections_put(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<PutCorrectionsRequest>,
) -> impl IntoResponse {
    let file_id = id;

    // Validate
    if body.corrections.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "corrections array must not be empty".to_string(),
            }),
        )
            .into_response();
    }

    // Verify file exists
    let file_exists: bool = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE id = ?")
        .bind(file_id)
        .fetch_one(&state.db)
        .await
        .map(|c: i64| c > 0)
        .unwrap_or(false);
    if !file_exists {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("File not found with id: {}", file_id),
            }),
        )
            .into_response();
    }

    for corr in &body.corrections {
        // Validate link_type
        if corr.link_type != "include" && corr.link_type != "exclude" {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "Invalid linkType '{}': must be 'include' or 'exclude'",
                        corr.link_type
                    ),
                }),
            )
                .into_response();
        }

        // Verify track exists
        let track_exists: bool =
            sqlx::query_scalar("SELECT COUNT(*) FROM service_tracks WHERE id = ?")
                .bind(corr.track_id)
                .fetch_one(&state.db)
                .await
                .map(|c: i64| c > 0)
                .unwrap_or(false);
        if !track_exists {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Track not found with id: {}", corr.track_id),
                }),
            )
                .into_response();
        }

        // UPSERT the correction
        if let Err(e) = sqlx::query(
            r#"INSERT INTO file_track_corrections (file_id, track_id, link_type, reason)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(file_id, track_id) DO UPDATE SET
                   link_type = excluded.link_type,
                   reason = excluded.reason"#,
        )
        .bind(file_id)
        .bind(corr.track_id)
        .bind(&corr.link_type)
        .bind(&corr.reason)
        .execute(&state.db)
        .await
        {
            return internal_error(e).into_response();
        }
    }

    // Return updated state (inline instead of delegating to avoid opaque return type)
    let automatic = compute_automatic_links_file(&state.db, file_id)
        .await
        .unwrap_or_default();
    let manual_includes = manual_corrections_for_file(&state.db, file_id, "include")
        .await
        .unwrap_or_default();
    let manual_excludes = manual_corrections_for_file(&state.db, file_id, "exclude")
        .await
        .unwrap_or_default();
    let effective = compute_effective_links_file(&state.db, file_id)
        .await
        .unwrap_or_default();

    let data = FileCorrectionsState {
        file_id,
        automatic_links: automatic,
        manual_includes,
        manual_excludes,
        effective_links: effective,
    };

    (StatusCode::OK, Json(ApiResponse { data })).into_response()
}

// ── Track Endpoints ──────────────────────────────────────────────────────

/// GET /api/tracks/{id}/file-corrections — return the full correction state for a track.
pub async fn track_file_corrections_get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let track_id = id;

    // Verify track exists
    let track_exists: bool = sqlx::query_scalar("SELECT COUNT(*) FROM service_tracks WHERE id = ?")
        .bind(track_id)
        .fetch_one(&state.db)
        .await
        .map(|c: i64| c > 0)
        .unwrap_or(false);
    if !track_exists {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Track not found with id: {}", track_id),
            }),
        )
            .into_response();
    }

    let automatic = compute_automatic_links_track(&state.db, track_id)
        .await
        .unwrap_or_default();
    let manual_includes = manual_corrections_for_track(&state.db, track_id, "include")
        .await
        .unwrap_or_default();
    let manual_excludes = manual_corrections_for_track(&state.db, track_id, "exclude")
        .await
        .unwrap_or_default();
    let effective = compute_effective_links_track(&state.db, track_id)
        .await
        .unwrap_or_default();

    let data = TrackCorrectionsState {
        track_id,
        automatic_links: automatic,
        manual_includes,
        manual_excludes,
        effective_links: effective,
    };

    (StatusCode::OK, Json(ApiResponse { data })).into_response()
}

/// PUT /api/tracks/{id}/file-corrections — upsert corrections for a track.
pub async fn track_file_corrections_put(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<PutCorrectionsRequest>,
) -> impl IntoResponse {
    let track_id = id;

    // Validate
    if body.corrections.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "corrections array must not be empty".to_string(),
            }),
        )
            .into_response();
    }

    // Verify track exists
    let track_exists: bool = sqlx::query_scalar("SELECT COUNT(*) FROM service_tracks WHERE id = ?")
        .bind(track_id)
        .fetch_one(&state.db)
        .await
        .map(|c: i64| c > 0)
        .unwrap_or(false);
    if !track_exists {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Track not found with id: {}", track_id),
            }),
        )
            .into_response();
    }

    for corr in &body.corrections {
        // Validate link_type
        if corr.link_type != "include" && corr.link_type != "exclude" {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "Invalid linkType '{}': must be 'include' or 'exclude'",
                        corr.link_type
                    ),
                }),
            )
                .into_response();
        }

        // Verify file exists
        let file_exists: bool = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE id = ?")
            .bind(corr.track_id) // track_id field actually holds the file_id in this context
            .fetch_one(&state.db)
            .await
            .map(|c: i64| c > 0)
            .unwrap_or(false);

        // Note: the CorrectionInput uses `track_id` as a field name, but in the
        // track context, the user provides file IDs via `trackId`. We treat
        // corr.track_id as the file_id here since that's what the PUT body provides.
        let file_id = corr.track_id;

        if !file_exists {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("File not found with id: {}", file_id),
                }),
            )
                .into_response();
        }

        // UPSERT the correction
        if let Err(e) = sqlx::query(
            r#"INSERT INTO file_track_corrections (file_id, track_id, link_type, reason)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(file_id, track_id) DO UPDATE SET
                   link_type = excluded.link_type,
                   reason = excluded.reason"#,
        )
        .bind(file_id)
        .bind(track_id)
        .bind(&corr.link_type)
        .bind(&corr.reason)
        .execute(&state.db)
        .await
        {
            return internal_error(e).into_response();
        }
    }

    // Return updated state (inline instead of delegating to avoid opaque return type)
    let automatic = compute_automatic_links_track(&state.db, track_id)
        .await
        .unwrap_or_default();
    let manual_includes = manual_corrections_for_track(&state.db, track_id, "include")
        .await
        .unwrap_or_default();
    let manual_excludes = manual_corrections_for_track(&state.db, track_id, "exclude")
        .await
        .unwrap_or_default();
    let effective = compute_effective_links_track(&state.db, track_id)
        .await
        .unwrap_or_default();

    let data = TrackCorrectionsState {
        track_id,
        automatic_links: automatic,
        manual_includes,
        manual_excludes,
        effective_links: effective,
    };

    (StatusCode::OK, Json(ApiResponse { data })).into_response()
}

// ── Delete Endpoint ──────────────────────────────────────────────────────

/// DELETE /api/file-track-corrections/{id} — delete a single correction by primary key.
pub async fn correction_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Check if correction exists
    let exists: bool =
        sqlx::query_scalar("SELECT COUNT(*) FROM file_track_corrections WHERE id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .map(|c: i64| c > 0)
            .unwrap_or(false);

    if !exists {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Correction not found with id: {}", id),
            }),
        )
            .into_response();
    }

    if let Err(e) = sqlx::query("DELETE FROM file_track_corrections WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await
    {
        return internal_error(e).into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}
