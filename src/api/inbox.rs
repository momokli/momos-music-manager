//! Tag roundtrip inbox API.
//!
//! The inbox lists files whose stored comment does not match the generated
//! target comment — compared via roundtrip (`parse → generate → compare`),
//! so formatting differences (tag order, quoting, case) never create false
//! positives. Reuses the existing needs-comment target computation.
//!
//! Endpoints:
//!   GET /api/inbox           → { files: [InboxFileItem], total }
//!   GET /api/inbox/count     → { count } (nav badge)

use axum::{
    Json, Router,
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, internal_error};
use crate::db::{get_inbox_count, get_inbox_files};

// ── Request types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InboxQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

// ── Response types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InboxListResponse {
    files: Vec<crate::db::InboxFileItem>,
    total: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InboxCountResponse {
    count: i64,
}

// ── Handlers ──────────────────────────────────────────────────────────────

/// GET /api/inbox — files whose stored comment ≠ generated target comment.
async fn inbox_list_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<InboxQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);
    let offset = query.offset.unwrap_or(0).max(0);

    let files = match get_inbox_files(&state.db, limit, offset).await {
        Ok(f) => f,
        Err(e) => return internal_error(e).into_response(),
    };

    // Total across all pages (for "showing X of Y").
    let total = match get_inbox_count(&state.db).await {
        Ok(c) => c as usize,
        Err(e) => return internal_error(e).into_response(),
    };

    Json(ApiResponse {
        data: InboxListResponse { files, total },
    })
    .into_response()
}

/// GET /api/inbox/count — number of files needing a comment update.
async fn inbox_count_handler(State(state): State<Arc<AppState>>) -> Response {
    match get_inbox_count(&state.db).await {
        Ok(count) => Json(ApiResponse {
            data: InboxCountResponse { count },
        })
        .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/inbox", get(inbox_list_handler))
        .route("/api/inbox/count", get(inbox_count_handler))
}
