//! Daily tagging queue — generates a playlist from tag-filtered, BPM-constrained
//! tracks and optionally pushes it to Spotify.

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post};
use serde::Deserialize;
use sqlx::QueryBuilder;
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;
use crate::api::types::{ApiResponse, internal_error};

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyGenerateRequest {
    tags: Vec<String>,
    #[serde(default)]
    bpm_min: Option<f64>,
    #[serde(default)]
    bpm_max: Option<f64>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    exclude_fully_tagged: Option<bool>,
}

// ── Router ───────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/daily/generate", post(daily_generate_handler))
}

// ── Handlers ──────────────────────────────────────────────────────────────

async fn daily_generate_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DailyGenerateRequest>,
) -> impl IntoResponse {
    // Validate
    if request.tags.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "At least one tag is required"
            })),
        )
            .into_response();
    }

    let limit = request.limit.unwrap_or(20).clamp(1, 50);
    let bpm_min = request.bpm_min.unwrap_or(0.0);
    let bpm_max = request.bpm_max.unwrap_or(999.0);
    let exclude_fully_tagged = request.exclude_fully_tagged.unwrap_or(true);

    // ── Resolve tag names to track IDs ──────────────────────────────────
    let mut qb = QueryBuilder::new(
        "SELECT DISTINCT st.id FROM service_tracks st \
         JOIN track_resolved_tags trt ON trt.track_id = st.id \
         WHERE st.service = 'spotify'",
    );

    // Tag name filter
    qb.push(" AND trt.tag_name IN (");
    let mut separated = qb.separated(", ");
    for tag in &request.tags {
        separated.push_bind(tag);
    }
    separated.push_unseparated(")");

    // BPM filter (via linked local files)
    if bpm_min > 0.0 || bpm_max < 999.0 {
        qb.push(
            " AND st.id IN (\
             SELECT vftl.track_id FROM v_file_track_link vftl \
             JOIN files f ON f.id = vftl.file_id \
             WHERE f.bpm IS NOT NULL AND f.bpm >= ",
        );
        qb.push_bind(bpm_min);
        qb.push(" AND f.bpm <= ");
        qb.push_bind(bpm_max);
        qb.push(")");
    }

    // Exclude fully-tagged tracks (P+M+V)
    if exclude_fully_tagged {
        qb.push(" AND NOT (");
        qb.push(
            "EXISTS (SELECT 1 FROM track_resolved_tags trt_p \
             WHERE trt_p.track_id = st.id AND LOWER(trt_p.prefix) = 'p')",
        );
        qb.push(" AND ");
        qb.push(
            "EXISTS (SELECT 1 FROM track_resolved_tags trt_m \
             WHERE trt_m.track_id = st.id AND LOWER(trt_m.prefix) = 'm')",
        );
        qb.push(" AND ");
        qb.push(
            "EXISTS (SELECT 1 FROM track_resolved_tags trt_v \
             WHERE trt_v.track_id = st.id AND LOWER(trt_v.prefix) = 'v')",
        );
        qb.push(")");
    }

    // Random sample + limit
    qb.push(" ORDER BY RANDOM() LIMIT ");
    qb.push_bind(limit);

    let track_ids: Vec<i64> = match qb.build_query_scalar().fetch_all(&state.db).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::error!("Failed to query tracks for daily queue: {}", e);
            return internal_error(e).into_response();
        }
    };

    // ── Playlist naming ─────────────────────────────────────────────────
    let tag_summary = if request.tags.len() == 1 {
        request.tags[0].clone()
    } else {
        format!("{}+{}", request.tags[0], request.tags.len() - 1)
    };
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let playlist_name = if bpm_min > 0.0 || bpm_max < 999.0 {
        format!(
            "Daily-{}-{:.0}-{:.0}-{}",
            tag_summary, bpm_min, bpm_max, date
        )
    } else {
        format!("Daily-{}-{}", tag_summary, date)
    };

    // ── Create local playlist ────────────────────────────────────────────
    let playlist_id_str = format!("local-{}", Uuid::new_v4());

    let insert_result = sqlx::query(
        "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at) \
         VALUES ('local', ?, ?, unixepoch(), unixepoch())",
    )
    .bind(&playlist_id_str)
    .bind(&playlist_name)
    .execute(&state.db)
    .await;

    let playlist_db_id = match insert_result {
        Ok(r) => r.last_insert_rowid(),
        Err(e) => {
            tracing::error!("Failed to create daily playlist: {}", e);
            return internal_error(e).into_response();
        }
    };

    // Link tracks to playlist
    for track_id in &track_ids {
        if let Err(e) = sqlx::query(
            "INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, added_at) \
             VALUES (?, ?, unixepoch())",
        )
        .bind(playlist_db_id)
        .bind(track_id)
        .execute(&state.db)
        .await
        {
            tracing::error!(
                "Failed to add track {} to daily playlist {}: {}",
                track_id,
                playlist_db_id,
                e
            );
        }
    }

    // NOTE: Not refreshing materialized tables — the playlist name "Daily-..."
    // doesn't match any tag name, so the refresh would be a 14s no-op.

    // ── Push to Spotify (best-effort) ────────────────────────────────────
    let mut spotify_url: Option<String> = None;
    let spotify_push_status = if !state.config.is_spotify_configured() {
        "not_configured"
    } else if track_ids.is_empty() {
        "no_tracks"
    } else {
        match crate::api::playlists::push_playlist_to_spotify(&state, playlist_db_id, None, true)
            .await
        {
            Ok(result) => {
                spotify_url = Some(result.spotify_url);
                "ok"
            }
            Err((status, err_val)) => {
                tracing::warn!(
                    "Failed to push daily playlist to Spotify (status {}): {:?}",
                    status,
                    err_val
                );
                "failed"
            }
        }
    };

    // ── Response ─────────────────────────────────────────────────────────
    let mut response = serde_json::json!({
        "playlistId": playlist_db_id,
        "playlistName": playlist_name,
        "trackCount": track_ids.len(),
        "spotifyPushStatus": spotify_push_status,
    });
    if let Some(url) = spotify_url {
        response["spotifyUrl"] = serde_json::Value::String(url);
    }

    (StatusCode::OK, Json(ApiResponse { data: response })).into_response()
}
