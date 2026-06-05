use axum::{
    Json, Router,
    extract::{Query, State},
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse};

// ── Types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TraktorImportRequest {
    /// Optional custom path to collection.nml.
    /// If omitted, auto-detects from ~/Documents/Native Instruments/Traktor
    custom_path: Option<String>,
}

// ── Handlers ──────────────────────────────────────────────────────────────

async fn traktor_import_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TraktorImportRequest>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match crate::tasks::start_traktor_import_task(&state.task_manager, &state.db, body.custom_path)
        .await
    {
        Ok(task_id) => Json(ApiResponse {
            data: serde_json::json!({ "taskId": task_id }),
        })
        .into_response(),
        Err(e) => (
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

/// Check the current status of the Traktor collection.nml file.
/// Returns the detected path and its last modification timestamp.
async fn traktor_status_handler(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<TraktorImportRequest>,
) -> impl IntoResponse {
    let custom_path = query.custom_path;
    let custom_path_ref = custom_path.as_ref().map(std::path::Path::new);

    let (path, modified_at) = match crate::traktor::get_collection_status(custom_path_ref) {
        Ok((p, mtime)) => (
            Some(p.to_string_lossy().to_string()),
            Some(
                mtime
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
            ),
        ),
        Err(_) => (None, None),
    };

    Json(ApiResponse {
        data: serde_json::json!({
            "path": path,
            "modifiedAt": modified_at,
        }),
    })
    .into_response()
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/traktor/import", post(traktor_import_handler))
        .route("/api/traktor/status", get(traktor_status_handler))
}
