use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, internal_error};
use crate::db::{
    get_file_by_id, get_file_locations, get_prune_candidates, get_storage_status,
    set_file_location,
};

use crate::tasks::start_prune_files_task;

// ── Request/Response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PruneRequest {
    #[serde(default)]
    file_ids: Vec<i64>,
}

// ── Handlers ───────────────────────────────────────────────────────────────

async fn storage_settings_get_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(ApiResponse {
        data: serde_json::json!({}),
    })
    .into_response()
}

async fn storage_settings_put_handler(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    Json(ApiResponse {
        data: serde_json::json!({}),
    })
    .into_response()
}

async fn storage_status_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_storage_status(&state.db).await {
        Ok(status) => Json(ApiResponse { data: status }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn backpack_size_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match crate::db::files::get_backpack_size_stats(&state.db).await {
        Ok(stats) => Json(ApiResponse { data: stats }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn prune_preview_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_prune_candidates(&state.db).await {
        Ok(candidates) => Json(ApiResponse { data: candidates }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn prune_execute_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PruneRequest>,
) -> impl IntoResponse {
    if body.file_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "No file IDs provided".to_string(),
            }),
        )
            .into_response();
    }

    let task_id = start_prune_files_task(&state.task_manager, &state.db, body.file_ids).await;

    Json(ApiResponse {
        data: serde_json::json!({ "taskId": task_id }),
    })
    .into_response()
}

// ── Format Priority ──────────────────────────────────────────────────────────

/// Known audio format strings used for validation.
fn known_audio_formats() -> Vec<&'static str> {
    crate::audio_extensions::ALL_EXTENSIONS
        .iter()
        .map(|e| e.as_str())
        .collect()
}

/// GET /api/storage/settings/format-priority
/// Returns the current format priority list. Falls back to defaults if not set.
async fn format_priority_get_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let priorities = crate::db::files::load_format_priorities(&state.db).await;
    Json(ApiResponse {
        data: serde_json::json!({"priorities": priorities}),
    })
    .into_response()
}

/// PUT /api/storage/settings/format-priority
/// Sets a custom format priority list. Validates non-empty + known formats.
async fn format_priority_put_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let priorities = match body["priorities"].as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "priorities must be a non-empty array".to_string(),
                }),
            )
                .into_response();
        }
    };

    // Validate each format is known
    let known = known_audio_formats();
    for val in priorities {
        let f = val.as_str().unwrap_or("");
        if !known.contains(&f) {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("unknown format: {}", f),
                }),
            )
                .into_response();
        }
    }

    // Convert to JSON string array and store on deemix service_config row
    let json_str = serde_json::to_string(&priorities).unwrap_or_default();
    let now = chrono::Utc::now().timestamp();

    let _ = sqlx::query(
        r#"
        INSERT INTO service_config (service, metadata_json, is_connected, remote_playlists_count, remote_tracks_count, created_at, updated_at)
        VALUES ('deemix', ?, 0, 0, 0, ?, ?)
        ON CONFLICT(service) DO UPDATE SET
            metadata_json = excluded.metadata_json,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(&json_str)
    .bind(now)
    .bind(now)
    .execute(&state.db)
    .await;

    Json(ApiResponse {
        data: serde_json::json!({"priorities": priorities}),
    })
    .into_response()
}

// ── Backpack file-sync switch ────────────────────────────────────────────────

/// GET /api/storage/settings/backpack-sync
/// Whether the Backpack *file* sync (pull missing files + format cleanup) may run.
async fn backpack_sync_get_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let enabled = crate::backpack::backpack_sync_enabled(&state.db).await;
    Json(ApiResponse {
        data: serde_json::json!({ "enabled": enabled }),
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
struct BackpackSyncRequest {
    enabled: bool,
}

/// PUT /api/storage/settings/backpack-sync
/// Persists the Backpack file-sync switch. Turning it off pauses every
/// automatic pull (startup and tag toggles) as well as the manual Sync All.
async fn backpack_sync_put_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BackpackSyncRequest>,
) -> impl IntoResponse {
    match crate::backpack::set_backpack_sync_enabled(&state.db, body.enabled).await {
        Ok(()) => Json(ApiResponse {
            data: serde_json::json!({ "enabled": body.enabled }),
        })
        .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// POST /api/storage/sync-backpack
/// Pulls missing files from backup for all backpack tags.
/// For each track in a backpack tag, ensures the best format exists locally.
async fn sync_backpack_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // The switch turns the Backpack file sync (NAS pulls + format cleanup) off
    // entirely, including this manual trigger.
    if !crate::backpack::backpack_sync_enabled(&state.db).await {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse {
                data: serde_json::json!({ "error": "Backpack file sync is disabled" }),
            }),
        )
            .into_response();
    }

    // Restore is only possible from the object store (NAS retired).
    if !state.config.store.is_configured() {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse {
                data: serde_json::json!({ "error": "Object store is not configured" }),
            }),
        )
            .into_response();
    }

    let task_id =
        crate::tasks::start_backpack_sync_task(&state.task_manager, &state.db, &state.config.store)
            .await;
    if task_id.is_empty() {
        return Json(ApiResponse {
            data: serde_json::json!({
                "taskId": null,
                "message": "Backpack sync already in progress",
            }),
        })
        .into_response();
    }
    Json(ApiResponse {
        data: serde_json::json!({ "taskId": task_id }),
    })
    .into_response()
}
/// POST /api/storage/sync-store
/// Canonicalise + upload local files to the remote object store, then verify
/// the store's records (SHA-256 facts).
async fn sync_store_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if !state.config.store.is_configured() {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse {
                data: serde_json::json!({ "error": "Object store is not configured" }),
            }),
        )
            .into_response();
    }

    let task_id =
        crate::tasks::start_store_sync_task(&state.task_manager, &state.db, &state.config).await;
    if task_id.is_empty() {
        return Json(ApiResponse {
            data: serde_json::json!({
                "taskId": null,
                "message": "Store sync already in progress",
            }),
        })
        .into_response();
    }
    Json(ApiResponse {
        data: serde_json::json!({ "taskId": task_id }),
    })
    .into_response()
}

// ── Purge Orphans ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PurgeOrphansRequest {
    #[serde(default)]
    confirm: Option<bool>,
}

/// POST /api/storage/purge-orphans
async fn purge_orphans_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PurgeOrphansRequest>,
) -> impl IntoResponse {
    if !body.confirm.unwrap_or(false) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: serde_json::json!({"error": "Must set confirm=true to purge orphaned files"}),
            }),
        )
            .into_response();
    }

    match crate::db::files::purge_orphaned_files(&state.db).await {
        Ok(count) => Json(ApiResponse {
            data: serde_json::json!({"purged": count}),
        })
        .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}


/// POST /api/files/{id}/pull-from-backup
/// Copies a file from backup (NAS) to local disk.
async fn file_pull_from_backup_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // 1. Get the file record
    let file = match get_file_by_id(&state.db, id).await {
        Ok(Some(f)) => f,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "File not found".to_string(),
                }),
            )
                .into_response();
        }
        Err(e) => return internal_error(e).into_response(),
    };

    // 2. Check it has a backup location
    let locations = match get_file_locations(&state.db, id).await {
        Ok(l) => l,
        Err(e) => return internal_error(e).into_response(),
    };

    let backup_location = locations.iter().find(|l| l.location_type == "backup");
    let Some(backup_loc) = backup_location else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "File has no backup location".to_string(),
            }),
        )
            .into_response();
    };

    // 3. Determine the local path and whether it already exists
    let local_path = std::path::Path::new(&file.file_path);
    if local_path.exists() {
        return (
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: "File already exists locally".to_string(),
            }),
        )
            .into_response();
    }

    // 4. Only a store object can be restored — the NAS (rsync/SSH) is retired.
    let Some(hash) = crate::store::store_hash(&backup_loc.path) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Backup location is not a store object (NAS retired)".to_string(),
            }),
        )
            .into_response();
    };

    // 5. Restore from the object store, rewriting the comment from the DB.
    if !state.config.store.is_configured() {
        return (
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: "Object store is not configured".to_string(),
            }),
        )
            .into_response();
    }

    let client = crate::store::StoreClient::new(
        state.config.store.base_url.as_deref().unwrap_or_default(),
        state.config.store.token.as_deref().unwrap_or_default(),
    );
    let local_dest = std::path::Path::new(&file.file_path);
    match crate::store::restore_object(&client, &state.db, id, hash, local_dest).await {
        Ok(size) => {
            // 6. Record the restored local copy.
            let _ = set_file_location(&state.db, id, "local", &file.file_path, size).await;
            let _ = sqlx::query("UPDATE files SET last_verified_local = unixepoch() WHERE id = ?")
                .bind(id)
                .execute(&state.db)
                .await;

            Json(ApiResponse {
                data: serde_json::json!({
                    "fileId": id,
                    "localPath": file.file_path,
                    "status": "downloaded"
                }),
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Store restore failed: {e}"),
            }),
        )
            .into_response(),
    }
}

/// GET /api/files/{id}/backup-status
async fn file_backup_status_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let locations = match get_file_locations(&state.db, id).await {
        Ok(l) => l,
        Err(e) => return internal_error(e).into_response(),
    };

    let backed_up = locations.iter().any(|l| l.location_type == "backup");

    Json(ApiResponse {
        data: serde_json::json!({
            "backedUp": backed_up,
            "locations": locations
        }),
    })
    .into_response()
}

// ── Router ─────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/storage/status", get(storage_status_handler))
        .route(
            "/api/storage/settings",
            get(storage_settings_get_handler).put(storage_settings_put_handler),
        )
        .route("/api/storage/prune-preview", post(prune_preview_handler))
        .route("/api/storage/prune", post(prune_execute_handler))
        .route("/api/storage/sync-backpack", post(sync_backpack_handler))
        .route("/api/storage/sync-store", post(sync_store_handler))
        .route("/api/storage/backpack-size", get(backpack_size_handler))
        .route(
            "/api/storage/settings/format-priority",
            get(format_priority_get_handler).put(format_priority_put_handler),
        )
        .route(
            "/api/storage/settings/backpack-sync",
            get(backpack_sync_get_handler).put(backpack_sync_put_handler),
        )
                .route("/api/storage/purge-orphans", post(purge_orphans_handler))
        .route(
            "/api/files/{id}/backup-status",
            get(file_backup_status_handler),
        )
        .route(
            "/api/files/{id}/pull-from-backup",
            post(file_pull_from_backup_handler),
        )
}
