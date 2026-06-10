//! Dynamic bundle API: handlers for `/api/dynamic-bundles*` endpoints.
//!
//! Dynamic bundles define filter criteria (tags, BPM, PMV, file types) that
//! the system evaluates to compute which files belong. The bundle creates a
//! Setlist-category tag, so it participates in backpack sync, tag filtering,
//! and all existing tag workflows.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, internal_error};
use crate::db::{
    DynamicBundle, create_dynamic_bundle, delete_dynamic_bundle, get_dynamic_bundle,
    get_dynamic_bundle_file_count, get_dynamic_bundles, get_tag_by_id, refresh_file_resolved_tags,
    update_dynamic_bundle,
};

// ── Types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateDynamicBundleRequest {
    name: String,
    #[serde(default)]
    base_tags: Option<Vec<String>>,
    #[serde(default)]
    include_all_tracks: bool,
    bpm_min: Option<f64>,
    bpm_max: Option<f64>,
    #[serde(default)]
    pmv_categories: Option<Vec<String>>,
    #[serde(default)]
    keys: Option<Vec<String>>,
    rating_min: Option<i64>,
    play_count_min: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateDynamicBundleRequest {
    name: Option<String>,
    #[serde(default)]
    base_tags: Option<Option<Vec<String>>>,
    include_all_tracks: Option<bool>,
    bpm_min: Option<Option<f64>>,
    bpm_max: Option<Option<f64>>,
    #[serde(default)]
    pmv_categories: Option<Option<Vec<String>>>,
    #[serde(default)]
    keys: Option<Option<Vec<String>>>,
    rating_min: Option<Option<i64>>,
    play_count_min: Option<Option<i64>>,
}

/// Track preview row for the frontend preview table.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackPreview {
    track_id: i64,
    title: String,
    artist: String,
    bpm: Option<f64>,
    musical_key: Option<String>,
    file_path: Option<String>,
    file_type: Option<String>,
}

/// Enriched bundle response sent to the frontend.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DynamicBundleResponse {
    #[serde(flatten)]
    bundle: DynamicBundle,
    tag_name: String,
    tag_backpack: bool,
    matching_file_count: i64,
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Enrich a DynamicBundle with tag info and matching file count.
async fn enrich_bundle(state: &Arc<AppState>, bundle: DynamicBundle) -> DynamicBundleResponse {
    let tag = get_tag_by_id(&state.db, bundle.tag_id).await.ok().flatten();

    let matching_file_count = get_dynamic_bundle_file_count(&state.db, &bundle)
        .await
        .unwrap_or(0);

    DynamicBundleResponse {
        tag_name: tag.as_ref().map(|t| t.name.clone()).unwrap_or_default(),
        tag_backpack: tag.map(|t| t.backpack).unwrap_or(false),
        matching_file_count,
        bundle,
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────

/// GET /api/dynamic-bundles — list all dynamic bundles.
async fn list_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let bundles = match get_dynamic_bundles(&state.db).await {
        Ok(b) => b,
        Err(e) => return internal_error(format!("Failed to list bundles: {}", e)).into_response(),
    };

    let mut enriched = Vec::with_capacity(bundles.len());
    for bundle in bundles {
        enriched.push(enrich_bundle(&state, bundle).await);
    }

    Json(ApiResponse { data: enriched }).into_response()
}

/// POST /api/dynamic-bundles — create a new dynamic bundle.
async fn create_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateDynamicBundleRequest>,
) -> impl IntoResponse {
    // Validate: name must be non-empty
    let name = request.name.trim().to_string();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "name is required and must be non-empty".to_string(),
            }),
        )
            .into_response();
    }

    // create_dynamic_bundle handles tag creation internally
    let bundle = match create_dynamic_bundle(
        &state.db,
        &name,
        request.base_tags,
        request.include_all_tracks,
        request.bpm_min,
        request.bpm_max,
        request.pmv_categories,
        None, // file_types: handled by backpack format priority
        true, // exclude_wav_sources: backpack handles this
        request.keys,
        request.rating_min,
        request.play_count_min,
    )
    .await
    {
        Ok(b) => b,
        Err(e) => {
            return internal_error(format!("Failed to create bundle: {}", e)).into_response();
        }
    };

    // Re-resolve file_resolved_tags to populate matching files
    if let Err(e) = refresh_file_resolved_tags(&state.db).await {
        tracing::warn!("Failed to refresh file_resolved_tags: {}", e);
    }

    let enriched = enrich_bundle(&state, bundle).await;
    (StatusCode::CREATED, Json(ApiResponse { data: enriched })).into_response()
}

/// GET /api/dynamic-bundles/{id} — get a single dynamic bundle.
async fn get_handler(State(state): State<Arc<AppState>>, Path(id): Path<i64>) -> impl IntoResponse {
    let bundle = match get_dynamic_bundle(&state.db, id).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Dynamic bundle not found with id: {}", id),
                }),
            )
                .into_response();
        }
        Err(e) => {
            return internal_error(format!("Failed to get bundle: {}", e)).into_response();
        }
    };

    let enriched = enrich_bundle(&state, bundle).await;
    Json(ApiResponse { data: enriched }).into_response()
}

/// PUT /api/dynamic-bundles/{id} — update a dynamic bundle.
async fn update_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<UpdateDynamicBundleRequest>,
) -> impl IntoResponse {
    // Check bundle exists
    let existing = match get_dynamic_bundle(&state.db, id).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Dynamic bundle not found with id: {}", id),
                }),
            )
                .into_response();
        }
        Err(e) => {
            return internal_error(format!("Failed to get bundle: {}", e)).into_response();
        }
    };

    // Update the bundle
    let bundle = match update_dynamic_bundle(
        &state.db,
        id,
        request.name.as_deref(),
        request.base_tags,
        request.include_all_tracks,
        request.bpm_min,
        request.bpm_max,
        request.pmv_categories,
        None, // file_types: handled by backpack
        None, // exclude_wav_sources: handled by backpack
        request.keys,
        request.rating_min,
        request.play_count_min,
    )
    .await
    {
        Ok(b) => b,
        Err(e) => return internal_error(format!("Failed to update bundle: {}", e)).into_response(),
    };

    // Re-resolve file_resolved_tags
    if let Err(e) = refresh_file_resolved_tags(&state.db).await {
        tracing::warn!("Failed to refresh file_resolved_tags: {}", e);
    }

    let enriched = enrich_bundle(&state, bundle).await;
    Json(ApiResponse { data: enriched }).into_response()
}

/// DELETE /api/dynamic-bundles/{id} — delete a dynamic bundle.
async fn delete_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Check bundle exists
    match get_dynamic_bundle(&state.db, id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Dynamic bundle not found with id: {}", id),
                }),
            )
                .into_response();
        }
        Err(e) => {
            return internal_error(format!("Failed to get bundle: {}", e)).into_response();
        }
    };

    // Delete the bundle (tag is CASCADE'd)
    match delete_dynamic_bundle(&state.db, id).await {
        Ok(_) => {}
        Err(e) => {
            return internal_error(format!("Failed to delete bundle: {}", e)).into_response();
        }
    };

    // Re-resolve to clean up stale entries in file_resolved_tags
    if let Err(e) = refresh_file_resolved_tags(&state.db).await {
        tracing::warn!("Failed to refresh file_resolved_tags: {}", e);
    }

    StatusCode::NO_CONTENT.into_response()
}

/// POST /api/dynamic-bundles/{id}/resolve — force re-resolution of matching files.
async fn resolve_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Check bundle exists
    let bundle = match get_dynamic_bundle(&state.db, id).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Dynamic bundle not found with id: {}", id),
                }),
            )
                .into_response();
        }
        Err(e) => {
            return internal_error(format!("Failed to get bundle: {}", e)).into_response();
        }
    };

    // Re-resolve
    if let Err(e) = refresh_file_resolved_tags(&state.db).await {
        return internal_error(format!("Failed to refresh file_resolved_tags: {}", e))
            .into_response();
    }

    let count = get_dynamic_bundle_file_count(&state.db, &bundle)
        .await
        .unwrap_or(0);

    Json(ApiResponse {
        data: serde_json::json!({
            "id": id,
            "matchingFileCount": count,
        }),
    })
    .into_response()
}

/// GET /api/dynamic-bundles/{id}/preview — preview matching tracks (no side effects).
async fn preview_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let bundle = match get_dynamic_bundle(&state.db, id).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Dynamic bundle not found with id: {}", id),
                }),
            )
                .into_response();
        }
        Err(e) => {
            return internal_error(format!("Failed to get bundle: {}", e)).into_response();
        }
    };

    // Resolve matching file IDs without mutating state
    let file_ids =
        match crate::db::dynamic_bundles::resolve_dynamic_bundle(&state.db, &bundle).await {
            Ok(ids) => ids,
            Err(e) => {
                return internal_error(format!("Failed to resolve bundle: {}", e)).into_response();
            }
        };

    if file_ids.is_empty() {
        return Json(ApiResponse {
            data: serde_json::json!({
                "tracks": [],
                "matchingFileCount": 0,
            }),
        })
        .into_response();
    }

    // Query track preview data for the first 20 matching files
    let limit = 20i64;
    let take = std::cmp::min(file_ids.len() as i64, limit) as usize;
    let preview_ids = &file_ids[..take];
    let placeholders: Vec<String> = preview_ids.iter().map(|_| "?".to_string()).collect();

    let sql = format!(
        r#"
        SELECT DISTINCT vft.track_id, st.title, st.artist,
               f.bpm, f.musical_key, f.file_path, f.file_type
        FROM v_file_track_link vft
        JOIN service_tracks st ON st.id = vft.track_id
        LEFT JOIN files f ON f.id = vft.file_id
        WHERE vft.file_id IN ({})
        ORDER BY vft.track_id
        LIMIT ?
        "#,
        placeholders.join(",")
    );

    let mut q = sqlx::query_as::<
        _,
        (
            i64,
            String,
            String,
            Option<f64>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(&sql);
    for fid in preview_ids {
        q = q.bind(*fid);
    }
    q = q.bind(limit);

    let rows = match q.fetch_all(&state.db).await {
        Ok(r) => r,
        Err(e) => return internal_error(format!("Failed to query preview: {}", e)).into_response(),
    };

    let tracks: Vec<TrackPreview> = rows
        .into_iter()
        .map(|(tid, title, artist, bpm, key, path, ftype)| TrackPreview {
            track_id: tid,
            title,
            artist,
            bpm,
            musical_key: key,
            file_path: path,
            file_type: ftype,
        })
        .collect();

    Json(ApiResponse {
        data: serde_json::json!({
            "tracks": tracks,
            "matchingFileCount": file_ids.len(),
        }),
    })
    .into_response()
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/dynamic-bundles",
            get(list_handler).post(create_handler),
        )
        .route(
            "/api/dynamic-bundles/{id}",
            get(get_handler).put(update_handler).delete(delete_handler),
        )
        .route("/api/dynamic-bundles/{id}/resolve", post(resolve_handler))
        .route("/api/dynamic-bundles/{id}/preview", get(preview_handler))
}
