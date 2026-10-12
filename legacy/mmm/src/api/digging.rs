use axum::{
    Json, Router,
    extract::{Query, State},
    response::IntoResponse,
    routing::{get, post},
};
use std::sync::Arc;

use crate::api::types::{ApiResponse, ErrorResponse, internal_error};
use crate::digging::{
    DiggingSearchQuery, DiggingSuggestRequest, DiggingTracksQuery, LadderSuggestRequest,
    get_ladder_suggestions, get_multi_seed_suggestions, search_digging_tracks, search_tracks_and_files,
};
use crate::AppState;

// ── Handlers ──────────────────────────────────────────────────────────────

async fn digging_suggest_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DiggingSuggestRequest>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if request.seed_file_ids.is_none() && request.seed_tag.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Either seed_tag or seed_file_ids must be provided".to_string(),
            }),
        )
            .into_response();
    }

    match get_multi_seed_suggestions(&state.db, &request).await {
        Ok(response) => Json(ApiResponse { data: response }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn digging_search_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DiggingSearchQuery>,
) -> impl IntoResponse {
    // Only return early if ALL filters are empty (no text search + no tags + no BPM filter)
    let has_any_filter = !query.q.trim().is_empty()
        || query
            .tags
            .as_deref()
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false)
        || query.bpm_min.is_some()
        || query.bpm_max.is_some()
        || query.energy_min.is_some()
        || query.energy_max.is_some();

    if !has_any_filter {
        return Json(ApiResponse {
            data: serde_json::json!({"tags": [], "files": []}),
        })
        .into_response();
    }
    match search_tracks_and_files(&state.db, &query).await {
        Ok(response) => Json(ApiResponse { data: response }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn digging_tracks_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DiggingTracksQuery>,
) -> impl IntoResponse {
    match search_digging_tracks(&state.db, &query).await {
        Ok(response) => Json(ApiResponse { data: response }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn digging_ladder_suggest_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<LadderSuggestRequest>,
) -> impl IntoResponse {
    match get_ladder_suggestions(&state.db, &request).await {
        Ok(response) => Json(ApiResponse { data: response }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/digging/suggest", post(digging_suggest_handler))
        .route("/api/digging/search", get(digging_search_handler))
        .route("/api/digging/tracks", get(digging_tracks_handler))
        .route(
            "/api/digging/ladder/suggest",
            post(digging_ladder_suggest_handler),
        )
}
