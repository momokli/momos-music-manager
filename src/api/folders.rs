use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::Pool;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, internal_error};
use crate::db::{
    delete_folder, get_folder_by_id, get_folder_file_count, get_folder_stats,
    get_folders as db_get_folders, update_folder_active, update_folder_backup_config,
    update_folder_with_config,
};

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoldersQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInfo {
    pub id: i64,
    pub path: String,
    pub watch_enabled: bool,
    pub scan_recursive: bool,
    pub fixed_extensions: bool,
    pub file_extensions: String,
    pub max_depth: i32,
    pub file_count: i64,
    pub last_scanned: Option<i64>,
    pub backup_path: Option<String>,
    pub scan_sources: bool,
    pub auto_backup: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddFolderRequest {
    pub path: String,
    pub watch_enabled: bool,
    #[serde(default)]
    pub scan_recursive: bool,
    #[serde(default)]
    pub fixed_extensions: bool,
    #[serde(default = "default_file_extensions")]
    pub file_extensions: String,
    #[serde(default = "default_max_depth")]
    pub max_depth: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFolderRequest {
    pub path: Option<String>,
    pub watch_enabled: Option<bool>,
    pub scan_recursive: Option<bool>,
    pub fixed_extensions: Option<bool>,
    pub file_extensions: Option<String>,
    pub max_depth: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderBackupConfig {
    pub backup_path: Option<String>,
    pub scan_sources: bool,
}

fn default_file_extensions() -> String {
    String::new()
}

fn default_max_depth() -> i32 {
    1
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/folders",
            get(folders_handler).post(add_folder_handler),
        )
        .route("/api/folders/count", get(folders_count_handler))
        .route(
            "/api/folders/{id}",
            get(get_folder_handler)
                .put(update_folder_handler)
                .delete(delete_folder_handler),
        )
        .route("/api/folders/{id}/watch", post(toggle_watch_handler))
        .route("/api/folders/{id}/scan", post(scan_folder_handler))
        .route("/api/folders/{id}/stats", get(folder_stats_handler))
        .route(
            "/api/folders/{id}/auto-backup",
            put(folder_auto_backup_handler),
        )
        .route(
            "/api/folders/{id}/backup",
            put(folder_backup_config_handler),
        )
        .route(
            "/api/folders/{id}/scan-sources",
            post(folder_scan_sources_handler),
        )
}

// ── Helpers ────────────────────────────────────────────────────────────────

async fn get_folders(
    pool: &Pool<sqlx::Sqlite>,
    query: &FoldersQuery,
) -> anyhow::Result<Vec<FolderInfo>> {
    let limit = query.page_size.or(query.limit).unwrap_or(100);
    let offset = query.offset.unwrap_or(0);

    let folders = db_get_folders(pool).await?;

    // Convert Folder to FolderInfo with file counts
    let mut folder_infos = Vec::new();
    for folder in folders {
        let file_count = get_folder_file_count(pool, folder.id).await.unwrap_or(0);
        folder_infos.push(FolderInfo {
            id: folder.id,
            path: folder.folder_path,
            watch_enabled: folder.active,
            scan_recursive: folder.scan_recursive,
            fixed_extensions: folder.fixed_extensions,
            file_extensions: folder.file_extensions,
            max_depth: folder.max_depth,
            file_count,
            last_scanned: folder.last_scanned,
            backup_path: folder.backup_path.clone(),
            scan_sources: folder.scan_sources,
            auto_backup: folder.auto_backup,
        });
    }

    // Apply search filter (client-side)
    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        let lower = search.to_lowercase();
        folder_infos.retain(|f| f.path.to_lowercase().contains(&lower));
    }

    // Apply sort (client-side)
    if let Some(sort) = query.sort.as_deref() {
        let order = query.order.as_deref().unwrap_or("asc");
        match (sort, order) {
            ("path", "asc") => folder_infos.sort_by(|a, b| a.path.cmp(&b.path)),
            ("path", "desc") => folder_infos.sort_by(|a, b| b.path.cmp(&a.path)),
            ("file_count", "asc") => folder_infos.sort_by(|a, b| a.file_count.cmp(&b.file_count)),
            ("file_count", "desc") => folder_infos.sort_by(|a, b| b.file_count.cmp(&a.file_count)),
            ("watch_enabled", "asc") => {
                folder_infos.sort_by(|a, b| a.watch_enabled.cmp(&b.watch_enabled))
            }
            ("watch_enabled", "desc") => {
                folder_infos.sort_by(|a, b| b.watch_enabled.cmp(&a.watch_enabled))
            }
            ("scan_recursive", "asc") => {
                folder_infos.sort_by(|a, b| a.scan_recursive.cmp(&b.scan_recursive))
            }
            ("scan_recursive", "desc") => {
                folder_infos.sort_by(|a, b| b.scan_recursive.cmp(&a.scan_recursive))
            }
            ("last_scanned", "asc") => {
                folder_infos.sort_by(|a, b| a.last_scanned.cmp(&b.last_scanned))
            }
            ("last_scanned", "desc") => {
                folder_infos.sort_by(|a, b| b.last_scanned.cmp(&a.last_scanned))
            }
            ("max_depth", "asc") => folder_infos.sort_by(|a, b| a.max_depth.cmp(&b.max_depth)),
            ("max_depth", "desc") => folder_infos.sort_by(|a, b| b.max_depth.cmp(&a.max_depth)),
            _ => {}
        }
    }

    // Apply pagination (client-side)
    let paged: Vec<FolderInfo> = folder_infos
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    Ok(paged)
}

pub async fn get_folders_count(
    pool: &Pool<sqlx::Sqlite>,
    query: &FoldersQuery,
) -> anyhow::Result<i64> {
    let folders = db_get_folders(pool).await?;

    // Convert to FolderInfo for search filtering
    let mut folder_infos = Vec::new();
    for folder in folders {
        let file_count = get_folder_file_count(pool, folder.id).await.unwrap_or(0);
        folder_infos.push(FolderInfo {
            id: folder.id,
            path: folder.folder_path,
            watch_enabled: folder.active,
            scan_recursive: folder.scan_recursive,
            fixed_extensions: folder.fixed_extensions,
            file_extensions: folder.file_extensions,
            max_depth: folder.max_depth,
            file_count,
            last_scanned: folder.last_scanned,
            backup_path: folder.backup_path.clone(),
            scan_sources: folder.scan_sources,
            auto_backup: folder.auto_backup,
        });
    }

    // Apply search filter (client-side)
    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        let lower = search.to_lowercase();
        folder_infos.retain(|f| f.path.to_lowercase().contains(&lower));
    }

    Ok(folder_infos.len() as i64)
}

// ── Handlers ──────────────────────────────────────────────────────────────

async fn folders_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FoldersQuery>,
) -> impl IntoResponse {
    match get_folders(&state.db, &query).await {
        Ok(folders) => Json(ApiResponse { data: folders }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn folders_count_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FoldersQuery>,
) -> impl IntoResponse {
    match get_folders_count(&state.db, &query).await {
        Ok(count) => Json(ApiResponse { data: count }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn add_folder_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AddFolderRequest>,
) -> impl IntoResponse {
    // Normalize and validate folder path
    let normalized_path = match crate::db::normalize_and_validate_folder_path(&request.path) {
        Ok(path) => path,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
    };

    // Create folder in database with full configuration
    match crate::db::create_folder_with_config(
        &state.db,
        &normalized_path,
        request.watch_enabled,
        request.scan_recursive,
        request.fixed_extensions,
        request.file_extensions,
        request.max_depth,
    )
    .await
    {
        Ok(folder) => {
            let folder_info = FolderInfo {
                id: folder.id,
                path: folder.folder_path,
                watch_enabled: folder.active,
                scan_recursive: folder.scan_recursive,
                fixed_extensions: folder.fixed_extensions,
                file_extensions: folder.file_extensions,
                max_depth: folder.max_depth,
                file_count: 0,
                last_scanned: folder.last_scanned,
                backup_path: folder.backup_path.clone(),
                scan_sources: folder.scan_sources,
                auto_backup: folder.auto_backup,
            };
            Json(ApiResponse { data: folder_info }).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

async fn toggle_watch_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Get current folder to know active status
    match get_folder_by_id(&state.db, id).await {
        Ok(Some(folder)) => {
            // Toggle active status
            let new_active = !folder.active;
            match update_folder_active(&state.db, id, new_active).await {
                Ok(updated_folder) => {
                    let file_count = get_folder_file_count(&state.db, updated_folder.id)
                        .await
                        .unwrap_or(0);
                    let folder_info = FolderInfo {
                        id: updated_folder.id,
                        path: updated_folder.folder_path,
                        watch_enabled: updated_folder.active,
                        scan_recursive: updated_folder.scan_recursive,
                        fixed_extensions: updated_folder.fixed_extensions,
                        file_extensions: updated_folder.file_extensions,
                        max_depth: updated_folder.max_depth,
                        file_count,
                        last_scanned: updated_folder.last_scanned,
                        backup_path: updated_folder.backup_path.clone(),
                        scan_sources: updated_folder.scan_sources,
                        auto_backup: updated_folder.auto_backup,
                    };
                    Json(ApiResponse { data: folder_info }).into_response()
                }
                Err(e) => internal_error(e).into_response(),
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Folder not found with id: {}", id),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn get_folder_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_folder_by_id(&state.db, id).await {
        Ok(Some(folder)) => {
            let file_count = get_folder_file_count(&state.db, folder.id)
                .await
                .unwrap_or(0);
            let folder_info = FolderInfo {
                id: folder.id,
                path: folder.folder_path,
                watch_enabled: folder.active,
                scan_recursive: folder.scan_recursive,
                fixed_extensions: folder.fixed_extensions,
                file_extensions: folder.file_extensions,
                max_depth: folder.max_depth,
                file_count,
                last_scanned: folder.last_scanned,
                backup_path: folder.backup_path.clone(),
                scan_sources: folder.scan_sources,
                auto_backup: folder.auto_backup,
            };
            Json(ApiResponse { data: folder_info }).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Folder not found with id: {}", id),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn update_folder_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<UpdateFolderRequest>,
) -> impl IntoResponse {
    // Validate new path if provided
    let normalized_path = if let Some(path) = &request.path {
        match crate::db::normalize_and_validate_folder_path(path) {
            Ok(path) => Some(path),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
    } else {
        None
    };

    // Convert watch_enabled to active
    let active = request.watch_enabled;

    // Check folder exists
    match crate::db::get_folder_by_id(&state.db, id).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Folder not found with id: {}", id),
                }),
            )
                .into_response();
        }
        Err(e) => return internal_error(e).into_response(),
        _ => {}
    }

    // Update folder in database with full configuration
    match update_folder_with_config(
        &state.db,
        id,
        normalized_path.as_deref(),
        active,
        request.scan_recursive,
        request.fixed_extensions,
        request.file_extensions.as_deref(),
        request.max_depth,
    )
    .await
    {
        Ok(folder) => {
            let file_count = get_folder_file_count(&state.db, folder.id)
                .await
                .unwrap_or(0);
            let folder_info = FolderInfo {
                id: folder.id,
                path: folder.folder_path,
                watch_enabled: folder.active,
                scan_recursive: folder.scan_recursive,
                fixed_extensions: folder.fixed_extensions,
                file_extensions: folder.file_extensions,
                max_depth: folder.max_depth,
                file_count,
                last_scanned: folder.last_scanned,
                backup_path: folder.backup_path.clone(),
                scan_sources: folder.scan_sources,
                auto_backup: folder.auto_backup,
            };
            Json(ApiResponse { data: folder_info }).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

async fn delete_folder_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match crate::db::get_folder_by_id(&state.db, id).await {
        Ok(Some(_)) => match delete_folder(&state.db, id).await {
            Ok(()) => Json(ApiResponse {
                data: format!("Folder {} deleted successfully", id),
            })
            .into_response(),
            Err(e) => internal_error(e).into_response(),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Folder not found with id: {}", id),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn scan_folder_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    // Determine scan mode from query param (default: incremental)
    let scan_mode = match params.get("mode").map(|s| s.as_str()) {
        Some("full") => crate::db::ScanMode::Full,
        _ => crate::db::ScanMode::Incremental { since: None },
    };

    // First check if folder exists
    match get_folder_by_id(&state.db, id).await {
        Ok(Some(_)) => {
            // Folder exists, start a tracked scan task via TaskManager
            let mode_label = if matches!(&scan_mode, crate::db::ScanMode::Full) {
                "full"
            } else {
                "incremental"
            };
            let task_id = match crate::tasks::start_scan_folder_task(
                &state.task_manager,
                &state.db,
                id,
                scan_mode,
            )
            .await
            {
                Ok(id) => id,
                Err(e) => return internal_error(e).into_response(),
            };

            Json(ApiResponse {
                data: serde_json::json!({
                    "taskId": task_id,
                    "folderId": id,
                    "mode": mode_label
                }),
            })
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Folder not found with id: {}", id),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn folder_stats_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_folder_stats(&state.db, id).await {
        Ok(stats) => Json(ApiResponse { data: stats }).into_response(),
        Err(e) => {
            let msg = format!("{}", e);
            if msg.contains("not found") {
                (StatusCode::NOT_FOUND, Json(ErrorResponse { error: msg })).into_response()
            } else {
                internal_error(e).into_response()
            }
        }
    }
}

async fn folder_auto_backup_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    match crate::db::get_folder_by_id(&state.db, id).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Folder not found with id: {}", id),
                }),
            )
                .into_response();
        }
        Err(e) => return internal_error(e).into_response(),
        _ => {}
    }

    let auto_backup = body
        .get("autoBackup")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    match sqlx::query("UPDATE folders SET auto_backup = ?, updated_at = unixepoch() WHERE id = ?")
        .bind(auto_backup)
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(_) => Json(ApiResponse {
            data: serde_json::json!({ "autoBackup": auto_backup }),
        })
        .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn folder_backup_config_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    match crate::db::get_folder_by_id(&state.db, id).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Folder not found with id: {}", id),
                }),
            )
                .into_response();
        }
        Err(e) => return internal_error(e).into_response(),
        _ => {}
    }

    let backup_path = body.get("backupPath").and_then(|v| v.as_str());
    let scan_sources = body.get("scanSources").and_then(|v| v.as_bool());

    match update_folder_backup_config(&state.db, id, backup_path, scan_sources).await {
        Ok(()) => {
            // Fetch updated folder
            match get_folder_by_id(&state.db, id).await {
                Ok(Some(folder)) => Json(ApiResponse {
                    data: FolderBackupConfig {
                        backup_path: folder.backup_path.clone(),
                        scan_sources: folder.scan_sources,
                    },
                })
                .into_response(),
                _ => Json(ApiResponse {
                    data: FolderBackupConfig {
                        backup_path: backup_path.map(|s| s.to_string()),
                        scan_sources: scan_sources.unwrap_or(false),
                    },
                })
                .into_response(),
            }
        }
        Err(e) => internal_error(e).into_response(),
    }
}

async fn folder_scan_sources_handler(
    State(state): State<Arc<AppState>>,
    Path(folder_id): Path<i64>,
) -> impl IntoResponse {
    let folder = match get_folder_by_id(&state.db, folder_id).await {
        Ok(Some(f)) => f,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Folder not found".to_string(),
                }),
            )
                .into_response();
        }
        Err(e) => return internal_error(e).into_response(),
    };

    if !folder.scan_sources {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Folder does not have scan_sources enabled".to_string(),
            }),
        )
            .into_response();
    }

    let task_id =
        crate::tasks::start_scan_wav_sources_task(&state.task_manager, &state.db, folder_id).await;

    if task_id.is_empty() {
        return Json(ApiResponse {
            data: serde_json::json!({
                "taskId": null,
                "message": "Scan WAV sources already in progress for this folder",
            }),
        })
        .into_response();
    }

    Json(ApiResponse {
        data: serde_json::json!({ "taskId": task_id }),
    })
    .into_response()
}
