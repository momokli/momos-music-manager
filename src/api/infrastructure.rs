use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;
use crate::api::types::{ApiResponse, internal_error};
use crate::db::testing;
use crate::embeddings::compute_tag_similarities;
use crate::tasks::TaskStatus;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingStatusResponse {
    pub model_loaded: bool,
    pub tags_total: usize,
    pub tags_embedded: usize,
    pub model_version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub status: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<usize>,
    pub task_type: Option<String>,
}

// ── Handlers ──────────────────────────────────────────────────────────────

/// GET /api/version — returns the application version from Cargo.toml
async fn version_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// List tasks with pagination and optional status filter.
/// Merges live in-memory tasks with persisted task_history.
async fn tasks_list_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TasksQuery>,
) -> impl IntoResponse {
    let limit = query.page_size.or(query.limit).unwrap_or(50).min(200);
    let offset = query.offset.unwrap_or(0);
    let status_filter = query.status.clone().and_then(|s| match s.as_str() {
        "pending" => Some(TaskStatus::Pending),
        "running" => Some(TaskStatus::Running),
        "completed" => Some(TaskStatus::Completed),
        "failed" => Some(TaskStatus::Failed),
        "cancelled" | "canceled" => Some(TaskStatus::Cancelled),
        _ => None,
    });
    let sort = query.sort.clone();
    let order = query.order.clone();
    let task_type_filter = query.task_type.clone();

    // 1. Get live in-memory tasks (Running/Pending + recently completed)
    let (live_tasks, _live_total) = state
        .task_manager
        .list_tasks_paginated(500, 0, status_filter.clone(), sort.clone(), order.clone())
        .await;

    // Collect IDs of live tasks so we can deduplicate against history
    let live_ids: Vec<String> = live_tasks.iter().map(|t| t.id.clone()).collect();

    // 2. Get persisted history tasks (backfill for auto-pruned ones)
    let history_status = query.status.clone();
    let history_type = task_type_filter.clone();
    let (history_tasks, _hist_total) = crate::tasks::get_task_history(
        &state.db,
        1000,
        0,
        history_status.as_deref(),
        history_type.as_deref(),
    )
    .await
    .unwrap_or_default();

    // 3. Convert history entries to TaskProgress format and deduplicate
    let mut all_tasks: Vec<crate::tasks::TaskProgress> = live_tasks;
    for hist in &history_tasks {
        if let Some(hist_id) = hist.get("id").and_then(|v| v.as_str()) {
            if !live_ids.contains(&hist_id.to_string()) {
                // Try to deserialize — if it fails, skip
                if let Ok(task) = serde_json::from_value::<crate::tasks::TaskProgress>(hist.clone())
                {
                    all_tasks.push(task);
                }
            }
        }
    }

    // 4. Sort by created_at DESC
    all_tasks.sort_by(|a, b| {
        b.created_at_secs
            .partial_cmp(&a.created_at_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 5. Apply status filter (already applied to live; re-apply to merged set)
    if let Some(ref filter) = status_filter.clone() {
        all_tasks.retain(|t| t.status == *filter);
    }

    let total = all_tasks.len();
    let paginated = all_tasks
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect::<Vec<_>>();

    Json(ApiResponse {
        data: serde_json::json!({
            "tasks": paginated,
            "total": total,
            "limit": limit,
            "offset": offset,
        }),
    })
    .into_response()
}

/// Get a single task by ID
async fn task_handler(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    match state.task_manager.get_task(&task_id).await {
        Some(task) => Json(ApiResponse { data: task }).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse {
                data: format!("Task {} not found", task_id),
            }),
        )
            .into_response(),
    }
}

/// Cancel a task by ID
async fn task_cancel_handler(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
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

#[derive(Debug, serde::Deserialize)]
struct TaskHistoryQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
    status: Option<String>,
    #[serde(rename = "taskType")]
    task_type: Option<String>,
}

async fn tasks_history_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TaskHistoryQuery>,
) -> Json<serde_json::Value> {
    let limit = query.limit.unwrap_or(50).min(200);
    let offset = query.offset.unwrap_or(0);
    let status = query.status.as_deref().filter(|s| !s.is_empty());
    let task_type = query.task_type.as_deref().filter(|s| !s.is_empty());

    match crate::tasks::get_task_history(&state.db, limit, offset, status, task_type).await {
        Ok((rows, total)) => Json(serde_json::json!({
            "data": { "tasks": rows, "total": total, "limit": limit, "offset": offset }
        })),
        Err(e) => Json(serde_json::json!({ "error": format!("{:#}", e) })),
    }
}

async fn health_check_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use serde_json::json;

    // Check database connection
    match sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&state.db)
        .await
    {
        Ok(_) => Json(json!({
            "status": "ok",
            "database": "connected"
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "error",
                "database": e.to_string()
            })),
        )
            .into_response(),
    }
}

/// GET /api/dump — Export database as JSON file download
async fn dump_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use serde_json::json;

    match crate::dump::export_dump_json(&state.db).await {
        Ok(bytes) => {
            let filename = format!(
                "momos-dump-{}.json",
                chrono::Utc::now().format("%Y%m%d-%H%M%S")
            );
            let headers = [
                (header::CONTENT_TYPE, "application/json"),
                (
                    header::CONTENT_DISPOSITION,
                    &format!("attachment; filename=\"{}\"", filename),
                ),
            ];
            (StatusCode::OK, headers, bytes).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to export dump: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to export dump: {e}")})),
            )
                .into_response()
        }
    }
}

/// POST /api/restore?confirm=true — Import database from uploaded JSON file
async fn restore_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    use serde_json::json;

    // Safety guard: require ?confirm=true
    let confirmed = params.get("confirm").map(|s| s == "true").unwrap_or(false);
    if !confirmed {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "confirm=true query param is required (this operation wipes all existing data)"
            })),
        )
            .into_response();
    }

    // Extract the uploaded file from multipart
    let mut file_bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart.next_field().await.unwrap_or(None) {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let data = field.bytes().await.unwrap_or_default().to_vec();
            if !data.is_empty() {
                file_bytes = Some(data);
            }
            break;
        }
    }

    let data = match file_bytes {
        Some(d) if !d.is_empty() => d,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "No file uploaded. Send a multipart form with a 'file' field."})),
            )
                .into_response();
        }
    };

    // Write the uploaded data to a temp file
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("momos-restore-{}.json", Uuid::new_v4()));
    let display_path = temp_path.display().to_string();

    if let Err(e) = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&temp_path)?;
        f.write_all(&data)?;
        Ok(())
    })() {
        tracing::error!("Failed to write uploaded file: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to write uploaded file: {e}")})),
        )
            .into_response();
    }

    // Import the dump
    match crate::dump::import_dump(&state.db, &display_path).await {
        Ok(()) => {
            // Clean up temp file
            let _ = std::fs::remove_file(&temp_path);
            Json(json!({
                "success": true,
                "message": "Database restored successfully"
            }))
            .into_response()
        }
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            tracing::error!("Failed to restore dump: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to restore dump: {e}")})),
            )
                .into_response()
        }
    }
}

async fn embeddings_status_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let model_loaded = state.embeddings.lock().await.is_some();

    // Count tags with embeddings
    let tags_embedded: i64 =
        match sqlx::query_scalar::<_, Option<i64>>("SELECT COUNT(*) FROM tag_embeddings")
            .fetch_one(&state.db)
            .await
        {
            Ok(c) => c.unwrap_or(0),
            Err(_) => 0,
        };
    let tags_total: i64 = match sqlx::query_scalar::<_, Option<i64>>("SELECT COUNT(*) FROM tags")
        .fetch_one(&state.db)
        .await
    {
        Ok(c) => c.unwrap_or(0),
        Err(_) => 0,
    };

    Json(ApiResponse {
        data: EmbeddingStatusResponse {
            model_loaded,
            tags_total: tags_total as usize,
            tags_embedded: tags_embedded as usize,
            model_version: "all-MiniLM-L6-v2".to_string(),
        },
    })
    .into_response()
}

/// POST /api/embeddings/recompute
/// Startet eine Hintergrund-Aufgabe zur Neuberechnung aller Embeddings.
/// Gibt sofort eine task_id zurück — Fortschritt über /api/tasks sichtbar.
async fn recompute_embeddings_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let task_id =
        crate::tasks::start_recompute_embeddings_task(&state.task_manager, &state.db).await;

    Json(ApiResponse {
        data: serde_json::json!({
            "task_id": task_id,
            "message": "Embedding recompute started as background task",
        }),
    })
    .into_response()
}

/// POST /api/embeddings/reset-review
/// Setzt reviewed_at = NULL für alle Tags (Alle Tags werden wieder im Wizard angezeigt)
async fn reset_review_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match crate::db::reset_all_reviewed_at(&state.db).await {
        Ok(count) => {
            tracing::info!("Reset reviewed_at for {} tags", count);
            Json(ApiResponse {
                data: serde_json::json!({ "reset": count }),
            })
            .into_response()
        }
        Err(e) => internal_error(format!("Failed to reset reviewed_at: {}", e)).into_response(),
    }
}

/// POST /api/tag-similarities/recompute
/// Compute pairwise cosine similarity for all tag embeddings.
/// This is a fast operation (no ML model needed, just DB math).
async fn recompute_tag_similarities_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match compute_tag_similarities(&state.db).await {
        Ok(count) => Json(ApiResponse {
            data: serde_json::json!({
                "pairs_computed": count,
                "message": format!("Computed {} tag similarity pairs", count)
            }),
        })
        .into_response(),
        Err(e) => {
            internal_error(format!("Failed to compute tag similarities: {}", e)).into_response()
        }
    }
}

/// GET /api/tag-similarities/status
/// Returns how many tags have embeddings vs how many similarity pairs exist.
async fn tag_similarities_status_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let tags_with_embeddings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tag_embeddings")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let similarity_pairs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tag_similarities")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let tags_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    Json(ApiResponse {
        data: serde_json::json!({
            "tagsTotal": tags_total,
            "tagsWithEmbeddings": tags_with_embeddings,
            "similarityPairs": similarity_pairs,
            "ready": tags_with_embeddings > 1 && similarity_pairs > 0,
        }),
    })
    .into_response()
}

// ── Testing Seed Endpoint ─────────────────────────────────────────────────

/// POST /api/testing/seed — Seed known test data for Playwright E2E tests.
/// Accepts `{ "scenario": "basic" | "files_filter" | "digging" | "wav_variants" }`.
/// Returns row counts per table.
async fn testing_seed_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let scenario = body
        .get("scenario")
        .and_then(|v| v.as_str())
        .unwrap_or("basic");

    testing::clear_all_tables(&state.db).await;

    let counts: HashMap<String, usize> = match scenario {
        "basic" => testing::seed_basic_scenario(&state.db).await,
        "files_filter" => testing::seed_files_filter_scenario(&state.db).await,
        "digging" => testing::seed_digging_scenario(&state.db).await,
        "wav_variants" => testing::seed_wav_variant_scenario(&state.db).await,
        "comment_diff" => testing::seed_comment_diff_scenario(&state.db).await,
        "dynamic_bundles" => testing::seed_dynamic_bundles_scenario(&state.db).await,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("Unknown scenario: {}. Valid: basic, files_filter, digging, wav_variants, comment_diff, dynamic_bundles", scenario)
                })),
            )
                .into_response();
        }
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "scenario": scenario,
            "rows": counts,
        })),
    )
        .into_response()
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/version", get(version_handler))
        .route("/api/health", get(health_check_handler))
        .route("/api/dump", get(dump_handler))
        .route(
            "/api/restore",
            post(restore_handler).layer(DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        // Testing seed endpoint for Playwright E2E tests
        .route("/api/testing/seed", post(testing_seed_handler))
        .route("/api/tasks", get(tasks_list_handler))
        .route("/api/tasks/history", get(tasks_history_handler))
        .route(
            "/api/tasks/{id}",
            get(task_handler).delete(task_cancel_handler),
        )
        .route("/api/embeddings/status", get(embeddings_status_handler))
        .route(
            "/api/embeddings/recompute",
            post(recompute_embeddings_handler),
        )
        .route("/api/embeddings/reset-review", post(reset_review_handler))
        .route(
            "/api/tag-similarities/recompute",
            post(recompute_tag_similarities_handler),
        )
        .route(
            "/api/tag-similarities/status",
            get(tag_similarities_status_handler),
        )
        // File-track correction deletion
        .route(
            "/api/file-track-corrections/{id}",
            delete(crate::api::file_track_corrections::correction_delete),
        )
}
