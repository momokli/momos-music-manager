use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, internal_error};
use crate::backup::BackupEngine;
use crate::db::{
    get_file_by_id, get_file_locations, get_folder_by_id, get_prune_candidates, get_storage_status,
    set_file_location,
};

use crate::tasks::start_prune_files_task;

// ── Request/Response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PruneRequest {
    #[serde(default)]
    file_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupTestResponse {
    ok: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupExploreResponse {
    dirs: Vec<String>,
    writable: bool,
    error: Option<String>,
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

async fn storage_backup_handler(
    State(state): State<Arc<AppState>>,
    Path(folder_id): Path<i64>,
) -> impl IntoResponse {
    // Validate folder exists
    let folder = match get_folder_by_id(&state.db, folder_id).await {
        Ok(Some(f)) => f,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse {
                    data: serde_json::json!({"error": "Folder not found"}),
                }),
            )
                .into_response();
        }
        Err(e) => return internal_error(e).into_response(),
    };

    if folder.backup_path.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: serde_json::json!({"error": "Folder has no backup_path configured"}),
            }),
        )
            .into_response();
    }

    let task_id =
        crate::tasks::start_backup_folder_task(&state.task_manager, &state.db, folder_id).await;

    Json(ApiResponse {
        data: serde_json::json!({ "taskId": task_id }),
    })
    .into_response()
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

async fn backup_wavs_handler(
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

    if folder.backup_path.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Folder has no backup_path configured".to_string(),
            }),
        )
            .into_response();
    }

    let task_id =
        crate::tasks::start_backup_wavs_task(&state.task_manager, &state.db, folder_id).await;

    Json(ApiResponse {
        data: serde_json::json!({ "taskId": task_id }),
    })
    .into_response()
}

/// POST /api/storage/discover-backup/{folder_id}
/// Triggers a background task to scan NAS backup and discover backup-only files.
async fn storage_discover_backup_handler(
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

    if folder.backup_path.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Folder has no backup_path configured".to_string(),
            }),
        )
            .into_response();
    }

    let task_id =
        crate::tasks::start_backup_discovery_task(&state.task_manager, &state.db, folder_id).await;

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

/// Resolve the SSH host from a file's backup path by matching against folder configs.
/// Delegates to `crate::db::resolve_backup_host`.
async fn resolve_backup_host(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    backup_path: &str,
) -> anyhow::Result<(String, String)> {
    crate::db::resolve_backup_host(pool, backup_path).await
}

/// POST /api/storage/sync-backpack
/// Pulls missing files from backup for all backpack tags.
/// For each track in a backpack tag, ensures the best format exists locally.
async fn sync_backpack_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // 1. Get candidates
    let candidates = match crate::db::get_backpack_pull_candidates(&state.db).await {
        Ok(c) => c,
        Err(e) => return internal_error(e).into_response(),
    };

    if candidates.is_empty() {
        // No pull candidates, but still run cleanup (redundant files from previous syncs)
        let (deleted, freed_bytes) =
            match crate::db::cleanup_redundant_backpack_files(&state.db).await {
                Ok(result) => result,
                Err(e) => {
                    tracing::warn!("Backpack cleanup failed: {}", e);
                    (0, 0)
                }
            };
        return Json(ApiResponse {
            data: serde_json::json!({
                "pulled": 0,
                "failed": 0,
                "deleted": deleted,
                "freedBytes": freed_bytes,
                "candidates": []
            }),
        })
        .into_response();
    }

    // 2. Pull each candidate from backup
    let mut pulled = 0usize;
    let mut failed = 0usize;
    let mut results: Vec<serde_json::Value> = Vec::new();

    for candidate in &candidates {
        // Resolve SSH host from the folder config.
        // candidate.backup_path is like "/volume1/media/stems/file.stem.m4a"
        // We need to find which folder this file belongs to and extract the host
        // from the folder's backup_path (e.g., "backup:/volume1/media/stems").
        let (ssh_host, remote_path) =
            match resolve_backup_host(&state.db, &candidate.backup_path).await {
                Ok((host, path)) => (host, path),
                Err(e) => {
                    failed += 1;
                    results.push(serde_json::json!({
                        "fileId": candidate.file_id,
                        "status": "error",
                        "error": format!("{}", e)
                    }));
                    continue;
                }
            };

        let engine = BackupEngine::new(ssh_host.to_string());
        let local_path = std::path::Path::new(&candidate.local_path);
        let remote_path = remote_path.to_string();

        match engine.pull_file(&remote_path, local_path).await {
            Ok((true, file_size)) => {
                pulled += 1;
                // Update file_locations: add 'local' entry
                let _ = set_file_location(
                    &state.db,
                    candidate.file_id,
                    "local",
                    &candidate.local_path,
                    file_size,
                )
                .await;
                // Update last_verified_local
                let _ =
                    sqlx::query("UPDATE files SET last_verified_local = unixepoch() WHERE id = ?")
                        .bind(candidate.file_id)
                        .execute(&state.db)
                        .await;

                results.push(serde_json::json!({
                    "fileId": candidate.file_id,
                    "status": "pulled",
                    "fileType": candidate.file_type,
                    "title": candidate.title,
                    "artist": candidate.artist,
                    "localPath": candidate.local_path
                }));
            }
            Ok((false, _)) => {
                failed += 1;
                results.push(serde_json::json!({
                    "fileId": candidate.file_id,
                    "status": "failed",
                    "error": "rsync reported failure"
                }));
            }
            Err(e) => {
                failed += 1;
                results.push(serde_json::json!({
                    "fileId": candidate.file_id,
                    "status": "error",
                    "error": format!("{}", e)
                }));
            }
        }
    }

    // 3. Clean up redundant lower-priority local files (e.g. FLAC when stem is now local)
    let (deleted, freed_bytes) = match crate::db::cleanup_redundant_backpack_files(&state.db).await
    {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!("Backpack cleanup failed: {}", e);
            (0, 0)
        }
    };

    Json(ApiResponse {
        data: serde_json::json!({
            "pulled": pulled,
            "failed": failed,
            "deleted": deleted,
            "freedBytes": freed_bytes,
            "candidates": results
        }),
    })
    .into_response()
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

    // 4. Parse backup path to get SSH host and remote path
    let (ssh_host, remote_path) = match backup_loc.path.split_once(':') {
        Some((host, path)) => (host.to_string(), path.to_string()),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Invalid backup path format".to_string(),
                }),
            )
                .into_response();
        }
    };

    // 5. Ensure local parent directory exists
    if let Some(parent) = local_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).unwrap_or_default();
        }
    }

    // 6. Rsync from backup to local
    let dest = format!("{}:{}", ssh_host, remote_path);
    let output = tokio::process::Command::new("rsync")
        .arg("-a")
        .arg("--rsh=ssh")
        .arg(&dest)
        .arg(local_path.to_string_lossy().as_ref())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            // 7. Update file_locations: add 'local' entry
            if let Ok(metadata) = std::fs::metadata(local_path) {
                let file_size = metadata.len() as i64;
                let _ = set_file_location(&state.db, id, "local", &file.file_path, file_size).await;
                // 8. Update last_verified_local
                let _ =
                    sqlx::query("UPDATE files SET last_verified_local = unixepoch() WHERE id = ?")
                        .bind(id)
                        .execute(&state.db)
                        .await;
            }

            Json(ApiResponse {
                data: serde_json::json!({
                    "fileId": id,
                    "localPath": file.file_path,
                    "status": "downloaded"
                }),
            })
            .into_response()
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Rsync failed: {}", stderr),
                }),
            )
                .into_response()
        }
        Err(e) => internal_error(e).into_response(),
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

/// GET /api/backup/test?host=...
async fn backup_test_handler(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let host = match params.get("host") {
        Some(h) if !h.is_empty() => h,
        _ => {
            return Json(ApiResponse {
                data: BackupTestResponse {
                    ok: false,
                    error: Some("Missing 'host' query parameter".to_string()),
                },
            })
            .into_response();
        }
    };

    let engine = BackupEngine::new(host.clone());
    match engine.test_host().await {
        Ok(true) => Json(ApiResponse {
            data: BackupTestResponse {
                ok: true,
                error: None,
            },
        })
        .into_response(),
        Ok(false) => Json(ApiResponse {
            data: BackupTestResponse {
                ok: false,
                error: Some(
                    "Connection failed — host unreachable or SSH key not accepted".to_string(),
                ),
            },
        })
        .into_response(),
        Err(e) => Json(ApiResponse {
            data: BackupTestResponse {
                ok: false,
                error: Some(format!("SSH error: {}", e)),
            },
        })
        .into_response(),
    }
}

/// GET /api/backup/explore?host=...&path=...
async fn backup_explore_handler(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let host = match params.get("host") {
        Some(h) if !h.is_empty() => h,
        _ => {
            return Json(ApiResponse {
                data: BackupExploreResponse {
                    dirs: vec![],
                    writable: false,
                    error: Some("Missing 'host' query parameter".to_string()),
                },
            })
            .into_response();
        }
    };

    let path = params.get("path").map(|s| s.as_str()).unwrap_or("/");
    let engine = BackupEngine::new(host.clone());

    match engine.explore_dir(path).await {
        Ok((dirs, writable)) => Json(ApiResponse {
            data: BackupExploreResponse {
                dirs,
                writable,
                error: None,
            },
        })
        .into_response(),
        Err(e) => Json(ApiResponse {
            data: BackupExploreResponse {
                dirs: vec![],
                writable: false,
                error: Some(format!("Explore failed: {}", e)),
            },
        })
        .into_response(),
    }
}

// ── Router ─────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/storage/status", get(storage_status_handler))
        .route(
            "/api/storage/settings",
            get(storage_settings_get_handler).put(storage_settings_put_handler),
        )
        .route(
            "/api/storage/backup/{folder_id}",
            post(storage_backup_handler),
        )
        .route("/api/storage/prune-preview", post(prune_preview_handler))
        .route("/api/storage/prune", post(prune_execute_handler))
        .route(
            "/api/storage/backup-wavs/{folder_id}",
            post(backup_wavs_handler),
        )
        .route(
            "/api/storage/discover-backup/{folder_id}",
            post(storage_discover_backup_handler),
        )
        .route("/api/storage/sync-backpack", post(sync_backpack_handler))
        .route("/api/storage/backpack-size", get(backpack_size_handler))
        .route(
            "/api/storage/settings/format-priority",
            get(format_priority_get_handler).put(format_priority_put_handler),
        )
        .route("/api/backup/test", get(backup_test_handler))
        .route("/api/backup/explore", get(backup_explore_handler))
        .route(
            "/api/files/{id}/backup-status",
            get(file_backup_status_handler),
        )
        .route(
            "/api/files/{id}/pull-from-backup",
            post(file_pull_from_backup_handler),
        )
}
