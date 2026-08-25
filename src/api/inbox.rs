//! Tag roundtrip inbox API.
//!
//! The inbox lists files whose stored comment does not match the generated
//! target comment — compared via roundtrip (`parse → generate → compare`),
//! so formatting differences (tag order, quoting, case) never create false
//! positives. Reuses the existing needs-comment target computation.
//!
//! Full feature set (see `plans/proposed/tag-roundtrip-inbox.md`):
//!
//! * `GET /api/inbox`            → { files: [InboxFileItem], total }
//!   Each item carries `newTags`: the new (not-yet-canonical) tags of its
//!   diff, each with fuzzy suggestions of similar EXISTING tags
//!   (case-insensitive Levenshtein ≤ 2) and the open mapping (if any).
//! * `GET /api/inbox/count`      → { count } (nav badge)
//! * `GET /api/inbox/mappings`   → { mappings: [TagInboxMapping] } (open staging)
//! * `POST /api/inbox/resolve`   → { mapping } — record the user's decision:
//!   rename (typo fix), merge (into an existing tag), dismiss. Staging only:
//!   nothing is applied until the next comment write.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, internal_error};
use crate::db::{
    get_inbox_count, get_inbox_files, get_open_tag_mappings, upsert_tag_inbox_mapping,
};

// ── Request types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InboxQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveRequest {
    /// The new/typo tag to resolve (as it appears in comments).
    tag: String,
    /// `rename` | `merge` | `dismiss`
    action: String,
    /// Canonical spelling (rename) or existing tag (merge). Required for
    /// rename/merge, ignored for dismiss.
    target_tag: Option<String>,
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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InboxMappingsResponse {
    mappings: Vec<crate::db::TagInboxMapping>,
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn bad_request(message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: message.into(),
        }),
    )
        .into_response()
}

// ── Handlers ──────────────────────────────────────────────────────────────

/// GET /api/inbox — files whose stored comment ≠ generated target comment
/// (after open tag-inbox mappings), each annotated with similar-tag
/// suggestions for its new tags.
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

/// GET /api/inbox/mappings — open tag-inbox staging decisions.
async fn inbox_mappings_handler(State(state): State<Arc<AppState>>) -> Response {
    match get_open_tag_mappings(&state.db).await {
        Ok(mappings) => Json(ApiResponse {
            data: InboxMappingsResponse { mappings },
        })
        .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// POST /api/inbox/resolve — record the user's decision for a new tag.
///
/// Staging semantics: this only writes the `tag_inbox` mapping row. The
/// canonical (mapped) tag is written on the NEXT comment write — nothing is
/// auto-applied here.
async fn inbox_resolve_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ResolveRequest>,
) -> Response {
    let tag = body.tag.trim();
    if tag.is_empty() {
        return bad_request("tag is required");
    }

    match body.action.as_str() {
        "rename" | "merge" => {
            let target = body.target_tag.as_deref().unwrap_or("").trim();
            if target.is_empty() {
                return bad_request("targetTag is required for rename/merge");
            }
            if body.action == "merge" {
                if target.eq_ignore_ascii_case(tag) {
                    return bad_request("cannot merge a tag into itself");
                }
                // Merge target must be an existing canonical tag.
                match crate::db::get_tag_by_name(&state.db, target).await {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        return bad_request(format!(
                            "targetTag '{}' is not an existing tag",
                            target
                        ))
                    }
                    Err(e) => return internal_error(e).into_response(),
                }
            }

            match upsert_tag_inbox_mapping(&state.db, tag, &body.action, target).await {
                Ok(mapping) => Json(ApiResponse { data: mapping }).into_response(),
                Err(e) => internal_error(e).into_response(),
            }
        }
        "dismiss" => {
            match upsert_tag_inbox_mapping(&state.db, tag, "dismiss", tag).await {
                Ok(mapping) => Json(ApiResponse { data: mapping }).into_response(),
                Err(e) => internal_error(e).into_response(),
            }
        }
        other => bad_request(format!(
            "action must be 'rename', 'merge' or 'dismiss', got '{}'",
            other
        )),
    }
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/inbox", get(inbox_list_handler))
        .route("/api/inbox/count", get(inbox_count_handler))
        .route("/api/inbox/mappings", get(inbox_mappings_handler))
        .route("/api/inbox/resolve", post(inbox_resolve_handler))
}
