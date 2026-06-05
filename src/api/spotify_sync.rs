use axum::{
    Json, Router,
    extract::{Path, State},
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, internal_error};
use crate::tasks::SyncType;

// ── Handlers ──────────────────────────────────────────────────────────────

/// Direct handler for POST /api/services/spotify/sync.
/// Extracts State from axum and delegates to the internal handler.
async fn spotify_sync_direct_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    spotify_sync_handler(state, "spotify".to_string()).await
}

/// Internal handler — called directly by services.rs dispatcher.
pub(super) async fn spotify_sync_handler(
    state: Arc<AppState>,
    service: String,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Check if Spotify is configured in .env
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        ).into_response();
    }

    // Get service config to check if authenticated
    let service_config = match crate::db::get_service_config(&state.db, &service).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: format!("Service {} not configured", service),
                }),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!("Failed to get service config: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get service config: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Check if tokens are available
    if service_config.access_token.is_none() || service_config.refresh_token.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!(
                    "Tokens not configured for {}. Please authenticate first.",
                    service
                ),
            }),
        )
            .into_response();
    }

    // Start full sync using TaskManager
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::Full,
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse { data: task_id }).into_response(),
        Err(e) => {
            tracing::error!("Failed to start Spotify sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start sync: {}", e),
                }),
            )
                .into_response()
        }
    }
}

// Spotify sync task management endpoints

/// Get sync task status
async fn spotify_sync_task_handler(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match state.task_manager.get_sync_progress(&task_id).await {
        Some(progress) => Json(ApiResponse { data: progress }).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse {
                data: format!("Task {} not found", task_id),
            }),
        )
            .into_response(),
    }
}

/// Cancel a sync task
async fn spotify_sync_cancel_handler(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match state.task_manager.cancel_task(&task_id).await {
        Ok(()) => Json(ApiResponse {
            data: format!("Task {} cancelled", task_id),
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse {
                data: format!("Failed to cancel task: {}", e),
            }),
        )
            .into_response(),
    }
}

/// Sync only playlists (metadata)
async fn spotify_sync_playlists_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Check if Spotify is configured in .env
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        ).into_response();
    }

    // Start playlists-only sync using TaskManager
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::Playlists,
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse { data: task_id }).into_response(),
        Err(e) => {
            tracing::error!("Failed to start Spotify playlists sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start sync: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// Start a new-playlist sync: fetch playlist list from Spotify, diff against DB,
/// only sync metadata + tracks for playlists that don't yet exist.
async fn spotify_sync_new_playlists_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Check if Spotify is configured in .env
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file"
                    .to_string(),
            }),
        )
            .into_response();
    }

    // Start new-playlists sync using TaskManager
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::NewPlaylists,
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse { data: task_id }).into_response(),
        Err(e) => {
            tracing::error!("Failed to start new-playlist sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start sync: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// Sync tracks for all playlists
async fn spotify_sync_tracks_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Check if Spotify is configured in .env
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        ).into_response();
    }

    // Start tracks-all sync using TaskManager
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::TracksAll,
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse { data: task_id }).into_response(),
        Err(e) => {
            tracing::error!("Failed to start Spotify tracks sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start sync: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// Sync tracks for specific playlist
async fn spotify_sync_playlist_tracks_handler(
    State(state): State<Arc<AppState>>,
    Path(playlist_id): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Check if Spotify is configured in .env
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        ).into_response();
    }

    // Start tracks-for-playlist sync using TaskManager
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::TracksForPlaylist(playlist_id),
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse { data: task_id }).into_response(),
        Err(e) => {
            tracing::error!("Failed to start Spotify playlist tracks sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start sync: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// Refresh a single playlist's remote track count from Spotify metadata.
/// Fast: only 1 API call, no track streaming. Returns old and new counts.
async fn spotify_refresh_playlist_handler(
    State(state): State<Arc<AppState>>,
    Path(playlist_id): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured",
            }),
        )
            .into_response();
    }

    let client = match crate::spotify::client::SpotifyClient::from_stored_tokens(
        state.db.clone(),
        &state.config,
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            return internal_error(format!("Failed to create Spotify client: {}", e))
                .into_response();
        }
    };

    // Get the old remote count
    let old_remote: Option<i64> =
        sqlx::query_scalar("SELECT remote_track_count FROM service_playlists WHERE service = 'spotify' AND playlist_id = ?")
            .bind(&playlist_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    // Fetch playlist metadata from Spotify (1 API call, no track streaming)
    let playlist = match client.get_playlist(&playlist_id).await {
        Ok(p) => p,
        Err(e) => {
            return internal_error(format!("Failed to fetch playlist: {}", e)).into_response();
        }
    };

    let new_total = playlist.tracks.total as i64;
    let playlist_name = playlist.name.clone();
    let local_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM service_playlist_tracks spt JOIN service_playlists sp ON sp.id = spt.playlist_id WHERE sp.service = 'spotify' AND sp.playlist_id = ?",
    )
    .bind(&playlist_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    // Update remote_track_count
    let mut conn = match state.db.acquire().await {
        Ok(c) => c,
        Err(e) => {
            return internal_error(format!("DB connection error: {}", e)).into_response();
        }
    };
    if let Err(e) =
        crate::db::update_playlist_fetch_tracking(&mut conn, "spotify", &playlist_id, new_total)
            .await
    {
        return internal_error(format!("Failed to update: {}", e)).into_response();
    }
    drop(conn);

    let changed = old_remote != Some(new_total);

    Json(ApiResponse {
        data: serde_json::json!({
            "playlistId": playlist_id,
            "name": playlist_name,
            "oldRemoteCount": old_remote,
            "newRemoteCount": new_total,
            "localCount": local_count,
            "changed": changed,
        }),
    })
    .into_response()
}

/// Batch sync: fetch tracks for multiple playlists matching a criterion.
/// `mode`: "stale" (local != remote) or "recent" (not fetched in 15+ min).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSyncRequest {
    pub mode: String,
}

async fn spotify_sync_playlists_batch_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BatchSyncRequest>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file"
                    .to_string(),
            }),
        )
            .into_response();
    }

    // Query for matching Spotify playlists based on mode
    let playlist_ids: Vec<String> = match body.mode.as_str() {
        "stale" => {
            // Playlists where local < remote_unique (missing tracks).
            // Uses remote_unique_count instead of remote_track_count to avoid
            // false positives from episodes/duplicates that don't map to tracks.
            match sqlx::query_scalar::<_, String>(
                r#"
                SELECT sp.playlist_id
                FROM service_playlists sp
                WHERE sp.service = 'spotify'
                  AND (SELECT COUNT(*) FROM service_playlist_tracks spt WHERE spt.playlist_id = sp.id) < sp.remote_unique_count
                "#,
            )
            .fetch_all(&state.db)
            .await
            {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!("Failed to query stale playlists: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to query stale playlists: {}", e),
                        }),
                    )
                        .into_response();
                }
            }
        }
        "recent" => {
            // Playlists not fetched within the last 15 minutes (900 seconds)
            match sqlx::query_scalar::<_, String>(
                r#"
                SELECT sp.playlist_id
                FROM service_playlists sp
                WHERE sp.service = 'spotify'
                  AND (
                      sp.last_fetched_at IS NULL
                      OR sp.last_fetched_at < unixepoch() - 900
                  )
                "#,
            )
            .fetch_all(&state.db)
            .await
            {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!("Failed to query recent playlists: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to query recent playlists: {}", e),
                        }),
                    )
                        .into_response();
                }
            }
        }
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: format!("Invalid mode '{}'. Must be 'stale' or 'recent'.", other),
                }),
            )
                .into_response();
        }
    };

    if playlist_ids.is_empty() {
        return Json(ApiResponse {
            data: serde_json::json!({
                "taskId": null,
                "playlistCount": 0,
                "message": "No matching playlists found"
            }),
        })
        .into_response();
    }

    // Spawn a single batch task for all matching playlists
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::TracksForPlaylistList(playlist_ids.clone()),
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse {
            data: serde_json::json!({
                "taskId": task_id,
                "playlistCount": playlist_ids.len(),
                "message": format!("Started batch sync for {} playlist(s)", playlist_ids.len())
            }),
        })
        .into_response(),
        Err(e) => {
            tracing::error!("Failed to start batch sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start batch sync: {}", e),
                }),
            )
                .into_response()
        }
    }
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/services/spotify/sync",
            post(spotify_sync_direct_handler),
        )
        .route(
            "/api/services/spotify/sync/{task_id}",
            get(spotify_sync_task_handler).delete(spotify_sync_cancel_handler),
        )
        .route(
            "/api/services/spotify/sync/playlists",
            post(spotify_sync_playlists_handler),
        )
        .route(
            "/api/services/spotify/sync/new-playlists",
            post(spotify_sync_new_playlists_handler),
        )
        .route(
            "/api/services/spotify/sync/tracks",
            post(spotify_sync_tracks_handler),
        )
        .route(
            "/api/services/spotify/sync/playlists/{playlist_id}/tracks",
            post(spotify_sync_playlist_tracks_handler),
        )
        .route(
            "/api/services/spotify/refresh-playlist/{playlist_id}",
            post(spotify_refresh_playlist_handler),
        )
        .route(
            "/api/services/spotify/sync/playlists/batch",
            post(spotify_sync_playlists_batch_handler),
        )
}
