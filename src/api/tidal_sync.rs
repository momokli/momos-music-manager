//! Tidal sync API routes.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
};
use std::sync::Arc;

use crate::AppState;
use crate::api::types::ApiResponse;
use crate::tasks::{SyncType, Task, TaskStatus};

pub(super) async fn tidal_sync_handler(
    state: Arc<AppState>,
    _service: String,
) -> impl IntoResponse {
    tidal_sync_full_handler(State(state)).await
}

async fn tidal_sync_full_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if !state.config.is_tidal_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<String> {
                data: "Tidal not configured".to_string(),
            }),
        )
            .into_response();
    }
    start_tidal_task(state, SyncType::Full).await
}

async fn tidal_sync_playlists_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if !state.config.is_tidal_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<String> {
                data: "Tidal not configured".to_string(),
            }),
        )
            .into_response();
    }
    start_tidal_task(state, SyncType::Playlists).await
}

async fn tidal_sync_new_playlists_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    tidal_sync_playlists_handler(State(state)).await
}

async fn tidal_sync_batch_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    tidal_sync_full_handler(State(state)).await
}

async fn tidal_refresh_playlist_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if !state.config.is_tidal_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<String> {
                data: "Tidal not configured".to_string(),
            }),
        )
            .into_response();
    }
    start_tidal_task(state, SyncType::TracksForPlaylist(id)).await
}

async fn start_tidal_task(
    state: Arc<AppState>,
    sync_type: SyncType,
) -> axum::http::Response<axum::body::Body> {
    let task = Task::new_sync("tidal".to_string(), sync_type.clone());
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();

    match state.task_manager.start_task_unique(task).await {
        Ok(_) => {}
        Err(e) => {
            return (
                StatusCode::CONFLICT,
                Json(ApiResponse {
                    data: format!("Failed to start sync: {}", e),
                }),
            )
                .into_response();
        }
    }

    tracing::info!("Starting Tidal sync (task: {})", task_id);

    let tm = state.task_manager.clone();
    let config_clone = state.config.clone();
    let db = state.db.clone();

    let st = sync_type;
    tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.add_log(&worker_task_id, "Starting Tidal sync...".to_string())
            .await;

        match crate::tidal::client::TidalClient::from_stored_token(db.clone(), &config_clone).await
        {
            Ok(client) => {
                let progress_st = st.clone();
                let worker = crate::tidal::sync_worker::TidalSyncWorker::new(
                    db,
                    client,
                    worker_task_id.clone(),
                    st,
                    tokio_util::sync::CancellationToken::new(),
                    std::sync::Arc::new(tokio::sync::RwLock::new(crate::tasks::SyncProgress::new(
                        progress_st,
                    ))),
                );

                match worker.run().await {
                    Ok(result) => {
                        tm.update_task_status(&worker_task_id, TaskStatus::Completed)
                            .await;
                        tm.add_log(
                            &worker_task_id,
                            format!(
                                "Tidal sync completed: {} playlists, {} tracks",
                                result.playlist_count, result.track_count
                            ),
                        )
                        .await;
                    }
                    Err(e) => {
                        tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                            .await;
                        tm.add_log(&worker_task_id, format!("Tidal sync failed: {}", e))
                            .await;
                    }
                }
            }
            Err(e) => {
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                tm.add_log(
                    &worker_task_id,
                    format!("Failed to create Tidal client: {}", e),
                )
                .await;
            }
        }
    });

    (
        StatusCode::OK,
        Json(ApiResponse {
            data: serde_json::json!({"taskId": task_id, "status": "running"}),
        }),
    )
        .into_response()
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .without_v07_checks()
        .route("/api/services/tidal/sync", post(tidal_sync_full_handler))
        .route(
            "/api/services/tidal/sync/playlists",
            post(tidal_sync_playlists_handler),
        )
        .route(
            "/api/services/tidal/sync/new-playlists",
            post(tidal_sync_new_playlists_handler),
        )
        .route(
            "/api/services/tidal/sync/playlists/batch",
            post(tidal_sync_batch_handler),
        )
        .route(
            "/api/services/tidal/refresh-playlist/{id}",
            post(tidal_refresh_playlist_handler),
        )
}
