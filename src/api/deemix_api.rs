use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::Deserialize;
use sqlx::Pool;
use sqlx::Sqlite;
use std::sync::Arc;

use crate::AppState;
use crate::api::types::ApiResponse;
use crate::deemix::{
    DeemixAuthRequest, DeemixClient, DeemixCombinedQueueItem, DeemixEnqueueRequest,
};

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeemixQueueQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub status: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
}

// ── Handlers ──────────────────────────────────────────────────────────────

/// POST /api/services/deemix/auth
///
/// Validates ARL against a deemix server, then stores the config.
/// Body: { "arl": "...", "host": "http://localhost:6595" }
async fn deemix_auth_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DeemixAuthRequest>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    let host = request.host.trim_end_matches('/').to_string();
    if host.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Host is required".to_string(),
            }),
        )
            .into_response();
    }
    if request.arl.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "ARL is required".to_string(),
            }),
        )
            .into_response();
    }

    // Build a temporary client to test the ARL
    let client = DeemixClient::new(&host, state.db.clone());
    match client.login_arl(&request.arl).await {
        Ok(_) => {
            // Store ARL as access_token and host in metadata_json
            let metadata = serde_json::json!({"host": host});
            let now = chrono::Utc::now().timestamp();

            // Use SELECT + INSERT/UPDATE instead of ON CONFLICT because the
            // live DB may not have a UNIQUE constraint on service_config.service.
            let existing: Option<i64> =
                sqlx::query_scalar("SELECT id FROM service_config WHERE service = 'deemix'")
                    .fetch_optional(&state.db)
                    .await
                    .unwrap_or(None);

            let result = if existing.is_some() {
                sqlx::query(
                    "UPDATE service_config SET access_token = ?, metadata_json = ?, is_connected = 1, last_checked = ?, updated_at = ? WHERE service = 'deemix'",
                )
                .bind(&request.arl)
                .bind(metadata.to_string())
                .bind(now)
                .bind(now)
                .execute(&state.db)
                .await
            } else {
                sqlx::query(
                    "INSERT INTO service_config (service, access_token, metadata_json, is_connected, last_checked, updated_at, created_at) VALUES ('deemix', ?, ?, 1, ?, ?, ?)",
                )
                .bind(&request.arl)
                .bind(metadata.to_string())
                .bind(now)
                .bind(now)
                .bind(now)
                .execute(&state.db)
                .await
            };

            match result {
                Ok(_) => {
                    tracing::info!("Deemix configured and connected");
                    Json(ApiResponse {
                        data: serde_json::json!({"status": "connected"}),
                    })
                    .into_response()
                }
                Err(e) => {
                    tracing::error!("Failed to store deemix config: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to store config: {}", e),
                        }),
                    )
                        .into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Deemix auth failed: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: format!("Authentication failed: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// GET /api/services/deemix/queue
///
/// Returns combined list of deemix queue items + local deemix_downloads entries.
async fn deemix_queue_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DeemixQueueQuery>,
) -> impl IntoResponse {
    // Fetch local downloads from deemix_downloads table
    let local_downloads = sqlx::query_as::<
        _,
        (
            i64,
            String,
            Option<String>,
            String,
            i64,
            i64,
            Option<String>,
            Option<i64>,
            Option<i64>,
        ),
    >(
        r#"
        SELECT id, spotify_playlist_url, playlist_name, status,
               track_count_total, track_count_downloaded, error_message,
               created_at, updated_at
        FROM deemix_downloads
        ORDER BY updated_at DESC
        "#,
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    // Try to fetch remote queue from deemix server (if configured)
    let remote_items = match load_deemix_client_from_db(&state.db).await {
        Some(client) => client.get_queue().await.unwrap_or_default(),
        None => std::collections::HashMap::new(),
    };

    // Backfill local deemix_downloads table with remote queue items not yet in DB
    let now = chrono::Utc::now().timestamp();
    for item in remote_items.values() {
        let url = format!("https://open.spotify.com/playlist/{}", item.id);
        let status = match item.status.as_str() {
            "completed" | "withErrors" => "completed",
            "queued" => "queued",
            "downloading" => "downloading",
            _ => "queued",
        };
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO deemix_downloads (spotify_playlist_url, playlist_name, status, track_count_total, track_count_downloaded, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&url)
        .bind(&item.title)
        .bind(status)
        .bind(item.size)
        .bind(item.downloaded)
        .bind(now)
        .bind(now)
        .execute(&state.db)
        .await;
    }

    // Build combined result
    let mut combined: Vec<DeemixCombinedQueueItem> = Vec::new();

    for (id, url, name, status, total, downloaded, error, created, updated) in local_downloads {
        combined.push(DeemixCombinedQueueItem {
            id: Some(id),
            uuid: None,
            spotify_playlist_url: Some(url),
            playlist_name: name,
            status,
            track_count_total: total,
            track_count_downloaded: downloaded,
            error_message: error,
            created_at: created,
            updated_at: updated,
            title: None,
            artist: None,
            progress: 0,
        });
    }

    // Merge remote queue items (they may have richer status info)
    for (uuid, item) in remote_items {
        let status = match item.status.as_str() {
            "completed" => "completed",
            "withErrors" => "completed",
            "queued" => "queued",
            "downloading" => "downloading",
            _ => "queued",
        };

        // Check if we already have this in local list by URL
        let url = format!("https://open.spotify.com/playlist/{}", item.id);
        let existing = combined
            .iter_mut()
            .find(|c| c.spotify_playlist_url.as_deref() == Some(&url));

        if let Some(existing) = existing {
            existing.uuid = Some(uuid);
            existing.status = status.to_string();
            existing.track_count_total = item.size;
            existing.track_count_downloaded = item.downloaded;
            existing.progress = item.progress;
            existing.title = Some(item.title);
            existing.artist = Some(item.artist);
        } else {
            combined.push(DeemixCombinedQueueItem {
                id: None,
                uuid: Some(uuid),
                spotify_playlist_url: Some(url),
                playlist_name: Some(item.title.clone()),
                status: status.to_string(),
                track_count_total: item.size,
                track_count_downloaded: item.downloaded,
                error_message: None,
                created_at: None,
                updated_at: None,
                title: Some(item.title),
                artist: Some(item.artist),
                progress: item.progress,
            });
        }
    }

    // Apply status filter (client-side since combined list merges local + remote)
    let mut filtered: Vec<DeemixCombinedQueueItem> = combined;
    if let Some(ref status_filter) = query.status
        && !status_filter.is_empty()
        && status_filter != "all"
    {
        filtered.retain(|item| item.status == *status_filter);
    }

    // Apply search filter (client-side)
    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        let lower = search.to_lowercase();
        filtered.retain(|item| {
            item.title
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(&lower)
                || item
                    .artist
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&lower)
                || item
                    .playlist_name
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&lower)
                || item
                    .spotify_playlist_url
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&lower)
        });
    }

    // Apply sort (client-side)
    if let Some(sort) = query.sort.as_deref() {
        let order = query.order.as_deref().unwrap_or("asc");
        match (sort, order) {
            ("title", "asc") => filtered.sort_by(|a, b| {
                a.title
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.title.as_deref().unwrap_or(""))
            }),
            ("title", "desc") => filtered.sort_by(|a, b| {
                b.title
                    .as_deref()
                    .unwrap_or("")
                    .cmp(a.title.as_deref().unwrap_or(""))
            }),
            ("artist", "asc") => filtered.sort_by(|a, b| {
                a.artist
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.artist.as_deref().unwrap_or(""))
            }),
            ("artist", "desc") => filtered.sort_by(|a, b| {
                b.artist
                    .as_deref()
                    .unwrap_or("")
                    .cmp(a.artist.as_deref().unwrap_or(""))
            }),
            ("playlist_name", "asc") => filtered.sort_by(|a, b| {
                a.playlist_name
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.playlist_name.as_deref().unwrap_or(""))
            }),
            ("playlist_name", "desc") => filtered.sort_by(|a, b| {
                b.playlist_name
                    .as_deref()
                    .unwrap_or("")
                    .cmp(a.playlist_name.as_deref().unwrap_or(""))
            }),
            ("status", "asc") => filtered.sort_by(|a, b| a.status.cmp(&b.status)),
            ("status", "desc") => filtered.sort_by(|a, b| b.status.cmp(&a.status)),
            ("progress", "asc") => filtered.sort_by(|a, b| a.progress.cmp(&b.progress)),
            ("progress", "desc") => filtered.sort_by(|a, b| b.progress.cmp(&a.progress)),
            ("created_at", "asc") => {
                filtered.sort_by(|a, b| a.created_at.unwrap_or(0).cmp(&b.created_at.unwrap_or(0)))
            }
            ("created_at", "desc") => {
                filtered.sort_by(|a, b| b.created_at.unwrap_or(0).cmp(&a.created_at.unwrap_or(0)))
            }
            ("updated_at", "asc") => {
                filtered.sort_by(|a, b| a.updated_at.unwrap_or(0).cmp(&b.updated_at.unwrap_or(0)))
            }
            ("updated_at", "desc") => {
                filtered.sort_by(|a, b| b.updated_at.unwrap_or(0).cmp(&a.updated_at.unwrap_or(0)))
            }
            ("track_count_total", "asc") => {
                filtered.sort_by(|a, b| a.track_count_total.cmp(&b.track_count_total))
            }
            ("track_count_total", "desc") => {
                filtered.sort_by(|a, b| b.track_count_total.cmp(&a.track_count_total))
            }
            ("track_count_downloaded", "asc") => {
                filtered.sort_by(|a, b| a.track_count_downloaded.cmp(&b.track_count_downloaded))
            }
            ("track_count_downloaded", "desc") => {
                filtered.sort_by(|a, b| b.track_count_downloaded.cmp(&a.track_count_downloaded))
            }
            _ => {}
        }
    }

    // Apply pagination (client-side)
    let total = filtered.len() as i64;
    let page_limit = query.page_size.or(query.limit).unwrap_or(100).min(1000) as usize;
    let page_offset = query.offset.unwrap_or(0) as usize;
    let paged: Vec<DeemixCombinedQueueItem> = filtered
        .into_iter()
        .skip(page_offset)
        .take(page_limit)
        .collect();

    Json(ApiResponse {
        data: serde_json::json!({
            "items": paged,
            "total": total,
        }),
    })
    .into_response()
}

/// POST /api/services/deemix/queue
///
/// Add a Spotify playlist URL to the deemix download queue.
/// Body: { "url": "https://open.spotify.com/playlist/..." }
async fn deemix_enqueue_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DeemixEnqueueRequest>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if request.url.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "URL is required".to_string(),
            }),
        )
            .into_response();
    }

    // Insert into local deemix_downloads table
    let now = chrono::Utc::now().timestamp();
    let insert_result = sqlx::query(
        r#"
        INSERT INTO deemix_downloads (spotify_playlist_url, status, created_at, updated_at)
        VALUES (?, 'queued', ?, ?)
        ON CONFLICT(spotify_playlist_url) DO UPDATE SET
            status = 'queued',
            error_message = NULL,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(&request.url)
    .bind(now)
    .bind(now)
    .execute(&state.db)
    .await;

    if let Err(e) = insert_result {
        tracing::error!("Failed to insert deemix download: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse {
                data: format!("Failed to queue download: {}", e),
            }),
        )
            .into_response();
    }

    // Forward to deemix server
    if let Some(client) = load_deemix_client_from_db(&state.db).await
        && let Err(e) = client.add_to_queue(&request.url).await
    {
        tracing::error!("Failed to forward URL to deemix server: {}", e);
        return (
            StatusCode::BAD_GATEWAY,
            Json(ApiResponse {
                data: format!("Deemix server rejected the request: {}", e),
            }),
        )
            .into_response();
    }

    Json(ApiResponse {
        data: "Playlist added to download queue",
    })
    .into_response()
}

/// POST /api/services/deemix/queue/{id}/retry
///
/// Retry a failed download.
async fn deemix_retry_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Get the URL from the local download
    let url: Option<String> =
        sqlx::query_scalar("SELECT spotify_playlist_url FROM deemix_downloads WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

    let url = match url {
        Some(u) => u,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse {
                    data: format!("Download queue item {} not found", id),
                }),
            )
                .into_response();
        }
    };

    // Reset status to queued locally
    let now = chrono::Utc::now().timestamp();
    let _ = sqlx::query(
        "UPDATE deemix_downloads SET status = 'queued', error_message = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(now)
    .bind(id)
    .execute(&state.db)
    .await;

    // Forward to deemix server (best-effort)
    if let Some(client) = load_deemix_client_from_db(&state.db).await {
        // We need to find the UUID — first get the queue to find it
        match client.get_queue().await {
            Ok(queue) => {
                for (uuid, item) in &queue {
                    let item_url = format!("https://open.spotify.com/playlist/{}", item.id);
                    if item_url == url {
                        if let Err(e) = client.retry_download(uuid).await {
                            tracing::warn!("Failed to retry download on deemix: {}", e);
                        }
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get deemix queue for retry: {}", e);
            }
        }
    }

    Json(ApiResponse {
        data: "Download queued for retry",
    })
    .into_response()
}

/// DELETE /api/services/deemix/queue/{id}
///
/// Remove a queue item from the local database.
async fn deemix_delete_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match sqlx::query("DELETE FROM deemix_downloads WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(result) => {
            if result.rows_affected() == 0 {
                (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse {
                        data: format!("Download queue item {} not found", id),
                    }),
                )
                    .into_response()
            } else {
                Json(ApiResponse {
                    data: "Download queue item removed",
                })
                .into_response()
            }
        }
        Err(e) => {
            tracing::error!("Failed to delete deemix download: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to delete: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// Helper: load a DeemixClient from the database config.
pub(super) async fn load_deemix_client_from_db(db: &Pool<Sqlite>) -> Option<DeemixClient> {
    DeemixClient::from_db(db.clone()).await
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/services/deemix/auth", post(deemix_auth_handler))
        .route(
            "/api/services/deemix/queue",
            get(deemix_queue_handler).post(deemix_enqueue_handler),
        )
        .route(
            "/api/services/deemix/queue/{id}/retry",
            post(deemix_retry_handler),
        )
        .route(
            "/api/services/deemix/queue/{id}",
            delete(deemix_delete_handler),
        )
}
