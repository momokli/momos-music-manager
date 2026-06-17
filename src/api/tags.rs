//! Tag-domain API: handlers for `/api/tags*`, `/api/tag-categories*`,
//! and `/api/tag-energy-levels*` endpoints.
//!
//! Extracted from `legacy.rs` — every handler is a verbatim copy.

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::api::playlists;
use crate::api::types::{ApiResponse, ErrorResponse, apply_sort, internal_error};
use crate::db::{
    ServiceConnections, bulk_categorize_tags, bulk_check_tags, bulk_create_tags, bulk_review_tags,
    bulk_update_tags, categorize_tag as db_categorize_tag, create_tag, create_tag_category,
    delete_tag, delete_tag_category, get_bundle_members, get_bundle_of,
    get_bundle_tags_with_counts, get_curation_queue, get_embeddings_by_category, get_tag_by_id,
    get_tag_by_name, get_tag_categories, get_tag_category_by_id, get_tag_children,
    get_tag_embedding, get_tag_parents, get_tag_review_counts, get_unreviewed_tags,
    refresh_file_resolved_tags, refresh_track_resolved_tags, set_bundle_members, set_tag_backpack,
    set_tag_parents, update_tag, update_tag_category_metadata, upsert_tag_embedding,
};
use crate::digging::{
    TagReorderItem, delete_tag_energy_level, get_tag_energy_levels, reorder_tags_batch,
    set_tag_energy_level,
};
use crate::embeddings::{
    EmbeddingModel, compute_tag_similarities, deserialize_embedding, mean_embedding,
    serialize_embedding, suggest_category,
};
use crate::tasks::start_recompute_embeddings_task;

// ── Types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTagCategoryRequest {
    name: String,
    prefix: String,
    icon: String,
    is_default: Option<bool>,
    sort_order: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateTagCategoryRequest {
    name: Option<String>,
    prefix: Option<String>,
    icon: Option<String>,
    is_default: Option<bool>,
    sort_order: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTagRequest {
    name: String,
    category_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateTagRequest {
    name: Option<String>,
    category_id: Option<i64>,
}

// ─── Laboratory Analysis Types ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NeedsAnalysisQuery {
    format: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NeedsAnalysisFile {
    file_id: i64,
    file_path: String,
    file_type: String,
    local_size: i64,
    title: Option<String>,
    artist: Option<String>,
    needs_bpm: bool,
    needs_key: bool,
    backed_up: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NeedsAnalysisResponse {
    tag_id: i64,
    tag_name: String,
    file_count: usize,
    needs_bpm: usize,
    needs_key: usize,
    needs_both: usize,
    files: Vec<NeedsAnalysisFile>,
}

// ─── Auto-Categorize Types ─────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreviewedTagItem {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreviewedTagsResponse {
    pub total_unreviewed: usize,
    pub total_reviewed: usize,
    pub queue: Vec<UnreviewedTagItem>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySuggestionResponse {
    pub suggested_category_id: i64,
    pub suggested_category_name: String,
    pub confidence: f32,
    pub all_categories: Vec<crate::api::types::TagCategory>,
    pub service_connections: ServiceConnections,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorizeRequest {
    pub category_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkCategorizeRequest {
    pub tag_ids: Vec<i64>,
    pub category_id: i64,
}

// ─── Bulk Import Types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkImportEntry {
    pub name: String,
    pub category_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkImportRequest {
    pub entries: Vec<BulkImportEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkImportResult {
    pub name: String,
    pub status: String,
    pub tag_id: Option<i64>,
    pub category_id: i64,
    pub category_name: String,
    pub current_category_id: Option<i64>,
    pub current_category_name: Option<String>,
}

// ─── Bulk Resolve Types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkResolveEntry {
    pub name: String,
    pub category_id: i64,
    pub action: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkResolveRequest {
    pub entries: Vec<BulkResolveEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkResolveResult {
    pub name: String,
    pub status: String,
    pub tag_id: Option<i64>,
    pub category_id: i64,
    pub category_name: String,
    pub error: Option<String>,
}

// ─── Curation Types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct CurationQueueQuery {
    pub search: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    #[serde(rename = "has_parents")]
    pub has_parents: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurationParentTag {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub category_icon: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurationQueueTag {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub category_icon: String,
    pub file_count: i64,
    pub parent_count: i64,
    pub parents: Vec<CurationParentTag>,
}

// ─── API Tag Types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiTag {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub category_icon: Option<String>,
    pub category_id: Option<i64>,
    pub file_count: i64,
    pub created_at: i64,
    pub backpack: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub category: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
}

// ─── Energy Level Types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetEnergyLevelRequest {
    energy_level: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchReorderRequest {
    tags: Vec<TagReorderItem>,
}

// ── Parent Tag Types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetTagParentsRequest {
    #[serde(rename = "parentTagIds")]
    parent_tag_ids: Vec<i64>,
}

// ── Tag Bundle Types ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetBundleMembersRequest {
    #[serde(rename = "memberTagIds")]
    member_tag_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagBundlesQuery {
    search: Option<String>,
}

// ── Helper functions ──────────────────────────────────────────────────────

/// Get all tags with pagination, search, and category filter.
async fn get_all_tags(pool: &Pool<Sqlite>, query: &TagsQuery) -> Result<Vec<ApiTag>> {
    let limit = query.page_size.or(query.limit).unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    let search_pattern = query.search.as_ref().and_then(|s| {
        if s.is_empty() {
            None
        } else {
            Some(format!("%{}%", s))
        }
    });
    let categories: Option<Vec<String>> = query
        .category
        .as_ref()
        .and_then(|c| if c.is_empty() { None } else { Some(c) })
        .map(|c| c.split(',').map(|s| s.trim().to_string()).collect());

    let mut sql = String::from(
        "SELECT t.id, t.name, t.category_id, t.sort_order, t.created_at, t.reviewed_at,
                tc.name as category, tc.icon as category_icon,
                COALESCE(vfc.file_count, 0) as file_count,
                COALESCE(t.backpack, 0) as backpack
         FROM tags t
         LEFT JOIN tag_categories tc ON t.category_id = tc.id
         LEFT JOIN (
             SELECT fr.tag_id, COUNT(DISTINCT fr.file_id) as file_count
             FROM file_resolved_tags fr
             GROUP BY fr.tag_id
         ) vfc ON vfc.tag_id = t.id
         WHERE 1=1",
    );

    if search_pattern.is_some() {
        sql.push_str(" AND (t.name LIKE ? OR tc.name LIKE ?)");
    }
    if let Some(ref cats) = categories {
        let placeholders: Vec<&str> = cats.iter().map(|_| "?").collect();
        sql.push_str(&format!(" AND tc.name IN ({})", placeholders.join(", ")));
    }

    apply_sort(
        &mut sql,
        query.sort.as_deref(),
        query.order.as_deref(),
        &["t.name", "category", "t.created_at", "file_count"],
        "t.name",
    );

    sql.push_str(" LIMIT ? OFFSET ?");

    let mut q = sqlx::query(&sql);

    if let Some(ref pattern) = search_pattern {
        q = q.bind(pattern).bind(pattern);
    }
    if let Some(ref cats) = categories {
        for cat in cats {
            q = q.bind(cat);
        }
    }

    q = q.bind(limit).bind(offset);
    let rows = q.fetch_all(pool).await?;

    let mut tags = Vec::new();
    for row in rows {
        tags.push(ApiTag {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            category: row
                .try_get::<Option<String>, _>("category")?
                .unwrap_or_default(),
            category_icon: row.try_get("category_icon").ok(),
            category_id: row.try_get("category_id").ok(),
            file_count: row.try_get("file_count")?,
            created_at: row.try_get::<Option<i64>, _>("created_at")?.unwrap_or(0),
            backpack: row.try_get::<bool, _>("backpack").unwrap_or(false),
        });
    }

    Ok(tags)
}

pub async fn get_tags_count(pool: &Pool<Sqlite>, query: &TagsQuery) -> Result<i64> {
    let mut sql = String::from(
        "SELECT COUNT(DISTINCT t.id) FROM tags t
         LEFT JOIN tag_categories tc ON t.category_id = tc.id
         WHERE 1=1",
    );

    let search_pattern = query.search.as_ref().and_then(|s| {
        if s.is_empty() {
            None
        } else {
            Some(format!("%{}%", s))
        }
    });
    let categories: Option<Vec<String>> = query
        .category
        .as_ref()
        .and_then(|c| if c.is_empty() { None } else { Some(c) })
        .map(|c| c.split(',').map(|s| s.trim().to_string()).collect());

    if search_pattern.is_some() {
        sql.push_str(" AND (t.name LIKE ? OR tc.name LIKE ?)");
    }
    if let Some(ref cats) = categories {
        let placeholders: Vec<&str> = cats.iter().map(|_| "?").collect();
        sql.push_str(&format!(" AND tc.name IN ({})", placeholders.join(", ")));
    }

    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    if let Some(ref pattern) = search_pattern {
        q = q.bind(pattern).bind(pattern);
    }
    if let Some(ref cats) = categories {
        for cat in cats {
            q = q.bind(cat as &str);
        }
    }

    q.fetch_one(pool).await.map_err(|e| anyhow::anyhow!("{e}"))
}

/// Get a single tag with category information
async fn get_tag_with_category(
    pool: &Pool<Sqlite>,
    tag_id: i64,
) -> Result<Option<crate::api::types::Tag>> {
    let row = sqlx::query("SELECT * FROM v_tags_with_categories WHERE id = ?")
        .bind(tag_id)
        .fetch_optional(pool)
        .await?;

    if let Some(row) = row {
        Ok(Some(crate::api::types::Tag {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            category: row.try_get("category").ok(),
            category_icon: row.try_get("category_icon").ok(),
            created_at: row.try_get("created_at").ok(),
            backpack: row.try_get("backpack").ok(),
        }))
    } else {
        Ok(None)
    }
}

// ── Wrappers for cross-module route delegation ───────────────────────────

/// GET /api/tags/from-playlists — delegates to playlists module
async fn tags_from_playlists_handler_wrapper(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    playlists::get_playlists_without_tags_handler(State(state)).await
}

/// POST /api/tags/create-from-playlists — delegates to playlists module
async fn tags_create_from_playlists_handler_wrapper(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    playlists::create_tags_from_playlists_handler(State(state)).await
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// GET /api/tags/curation-queue
async fn curation_queue_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CurationQueueQuery>,
) -> impl IntoResponse {
    match get_curation_queue(
        &state.db,
        query.search.as_deref(),
        query.sort.as_deref(),
        query.order.as_deref(),
        query.has_parents.as_deref(),
        query.limit,
    )
    .await
    {
        Ok(tags) => {
            // Parse parents_json into Vec<CurationParentTag> for each tag
            let result: Vec<CurationQueueTag> = tags
                .into_iter()
                .map(|t| {
                    let parents: Vec<CurationParentTag> =
                        serde_json::from_str(&t.parents_json).unwrap_or_default();
                    CurationQueueTag {
                        id: t.id,
                        name: t.name,
                        category: t.category,
                        category_icon: t.category_icon,
                        file_count: t.file_count,
                        parent_count: t.parent_count,
                        parents,
                    }
                })
                .collect();
            Json(ApiResponse { data: result }).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/tags
async fn tags_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TagsQuery>,
) -> impl IntoResponse {
    match get_all_tags(&state.db, &query).await {
        Ok(tags) => Json(ApiResponse { data: tags }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/tags/count
async fn tags_count_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TagsQuery>,
) -> impl IntoResponse {
    match get_tags_count(&state.db, &query).await {
        Ok(count) => Json(ApiResponse { data: count }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/tag-categories
async fn tag_categories_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_tag_categories(&state.db).await {
        Ok(categories) => Json(ApiResponse { data: categories }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// POST /api/tag-categories
async fn create_tag_category_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateTagCategoryRequest>,
) -> impl IntoResponse {
    match create_tag_category(
        &state.db,
        &request.name,
        &request.prefix,
        &request.icon,
        request.is_default.unwrap_or(false),
        request.sort_order.unwrap_or(0) as i64,
    )
    .await
    {
        Ok(category) => Json(ApiResponse { data: category }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// PUT /api/tag-categories/{id} — metadata variant
async fn update_tag_category_metadata_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<UpdateTagCategoryRequest>,
) -> impl IntoResponse {
    match update_tag_category_metadata(
        &state.db,
        id,
        request.name.as_deref(),
        request.prefix.as_deref(),
        request.icon.as_deref(),
        request.is_default,
        request.sort_order.map(|v| v as i64),
    )
    .await
    {
        Ok(category) => Json(ApiResponse { data: category }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// DELETE /api/tag-categories/{id}
async fn delete_tag_category_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match delete_tag_category(&state.db, id).await {
        Ok(_) => Json(ApiResponse { data: () }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/tag-energy-levels
async fn tag_energy_levels_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_tag_energy_levels(&state.db).await {
        Ok(levels) => Json(ApiResponse { data: levels }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// PUT /api/tag-energy-levels/{tag_id}
async fn set_tag_energy_level_handler(
    State(state): State<Arc<AppState>>,
    Path(tag_id): Path<i64>,
    Json(request): Json<SetEnergyLevelRequest>,
) -> impl IntoResponse {
    match set_tag_energy_level(&state.db, tag_id, request.energy_level).await {
        Ok(_) => Json(ApiResponse {
            data: serde_json::json!({ "message": "Energy level updated" }),
        })
        .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// DELETE /api/tag-energy-levels/{tag_id}
async fn delete_tag_energy_level_handler(
    State(state): State<Arc<AppState>>,
    Path(tag_id): Path<i64>,
) -> impl IntoResponse {
    match delete_tag_energy_level(&state.db, tag_id).await {
        Ok(_) => Json(ApiResponse { data: () }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// PUT /api/tag-energy-levels/batch
async fn reorder_tags_batch_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<BatchReorderRequest>,
) -> impl IntoResponse {
    match reorder_tags_batch(&state.db, &request.tags).await {
        Ok(_) => Json(ApiResponse {
            data: serde_json::json!({ "message": "Tags reordered" }),
        })
        .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/tag-categories/{id}
async fn get_tag_category_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_tag_category_by_id(&state.db, id).await {
        Ok(Some(category)) => Json(ApiResponse { data: category }).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Tag category with id {} not found", id),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// PUT /api/tag-categories/{id}
async fn update_tag_category_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<UpdateTagCategoryRequest>,
) -> impl IntoResponse {
    match update_tag_category_metadata(
        &state.db,
        id,
        request.name.as_deref(),
        request.prefix.as_deref(),
        request.icon.as_deref(),
        request.is_default,
        request.sort_order.map(|v| v as i64),
    )
    .await
    {
        Ok(category) => Json(ApiResponse { data: category }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// After a tag is created or renamed, compute its embedding and recompute all similarity pairs.
async fn auto_update_tag_embedding_and_similarities(
    state: &Arc<AppState>,
    tag_id: i64,
    tag_name: &str,
) {
    // Take the lock once — check if model is cached and use it in one shot
    let vec = {
        let mut cache = state.embeddings.lock().await;
        match cache.as_mut().and_then(|m| m.embed_text(tag_name).ok()) {
            Some(v) => v,
            None => {
                // Model not loaded (or embedding failed) — fallback to background task
                drop(cache);
                tracing::info!(
                    "Embedding model not loaded (or failed), dispatching background recompute for tag '{}'",
                    tag_name
                );
                start_recompute_embeddings_task(&state.task_manager, &state.db).await;
                return;
            }
        }
    };

    let blob = serialize_embedding(&vec);
    if let Err(e) = upsert_tag_embedding(&state.db, tag_id, &blob, "all-MiniLM-L6-v2").await {
        tracing::warn!("Failed to upsert embedding for tag '{}': {}", tag_name, e);
        return;
    }

    // Recompute all similarities (cheap — just DB math on stored embeddings)
    match compute_tag_similarities(&state.db).await {
        Ok(count) => {
            tracing::debug!(
                "Auto-recomputed {} similarity pairs after tag '{}' mutation",
                count,
                tag_name
            );
        }
        Err(e) => {
            tracing::warn!("Failed to auto-recompute tag similarities: {}", e);
        }
    }
}

/// POST /api/tags
async fn create_tag_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateTagRequest>,
) -> impl IntoResponse {
    match create_tag(&state.db, &request.name, request.category_id).await {
        Ok(tag) => {
            // Auto-compute embedding and similarity pairs for the new tag
            auto_update_tag_embedding_and_similarities(&state, tag.id, &tag.name).await;

            // Get tag with category info using helper function
            match get_tag_with_category(&state.db, tag.id).await {
                Ok(Some(api_tag)) => Json(ApiResponse { data: api_tag }).into_response(),
                Ok(None) => {
                    // Fallback: create basic response
                    let api_tag = crate::api::types::Tag {
                        id: tag.id,
                        name: tag.name,
                        category: None,
                        category_icon: None,
                        created_at: None,
                        backpack: None,
                    };
                    Json(ApiResponse { data: api_tag }).into_response()
                }
                Err(e) => internal_error(format!("Failed to fetch tag with category info: {}", e))
                    .into_response(),
            }
        }
        Err(e) => internal_error(e).into_response(),
    }
}

/// PUT /api/tags/{id}
async fn update_tag_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<UpdateTagRequest>,
) -> impl IntoResponse {
    match update_tag(&state.db, id, request.name.as_deref(), request.category_id).await {
        Ok(tag) => {
            // If name changed, recompute embedding and similarity pairs
            if request.name.is_some() {
                auto_update_tag_embedding_and_similarities(&state, tag.id, &tag.name).await;
            }

            // Convert to API Tag format with category info
            match get_tag_with_category(&state.db, tag.id).await {
                Ok(Some(api_tag)) => Json(ApiResponse { data: api_tag }).into_response(),
                Ok(None) => {
                    // Fallback: create basic response
                    let api_tag = crate::api::types::Tag {
                        id: tag.id,
                        name: tag.name,
                        category: None,
                        category_icon: None,
                        created_at: None,
                        backpack: None,
                    };
                    Json(ApiResponse { data: api_tag }).into_response()
                }
                Err(e) => internal_error(format!("Failed to fetch tag with category info: {}", e))
                    .into_response(),
            }
        }
        Err(e) => internal_error(e).into_response(),
    }
}

/// DELETE /api/tags/{id}
async fn delete_tag_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_tag_by_id(&state.db, id).await {
        Ok(Some(_)) => match delete_tag(&state.db, id).await {
            Ok(_) => Json(ApiResponse { data: () }).into_response(),
            Err(e) => internal_error(e).into_response(),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Tag not found with id: {}", id),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/tags/{id}
async fn get_tag_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_tag_with_category(&state.db, id).await {
        Ok(Some(tag)) => Json(ApiResponse { data: tag }).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Tag with id {} not found", id),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/tags/{id}/needs-analysis
/// Returns files in this tag that need BPM/key analysis.
async fn tag_needs_analysis_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(query): Query<NeedsAnalysisQuery>,
) -> impl IntoResponse {
    // 1. Check tag exists
    let tag = match get_tag_by_id(&state.db, id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Tag with id {} not found", id),
                }),
            )
                .into_response();
        }
        Err(e) => return internal_error(e).into_response(),
    };

    let limit = query.limit.unwrap_or(50).max(1).min(500);

    // Build the query
    #[derive(sqlx::FromRow)]
    struct FileRow {
        file_id: i64,
        file_path: String,
        file_type: String,
        file_size: i64,
        title: Option<String>,
        artist: Option<String>,
        bpm: Option<f64>,
        musical_key: Option<String>,
        backed_up: i32,
    }

    // Count query (no limit)
    let mut count_sql = String::from(
        "SELECT COUNT(DISTINCT f.id) FROM files f
         JOIN file_resolved_tags frt ON frt.file_id = f.id AND frt.tag_name = ?
         JOIN file_locations fl_local ON fl_local.file_id = f.id AND fl_local.location_type = 'local'
         WHERE (f.bpm IS NULL OR f.musical_key IS NULL)"
    );

    // Data query with limit
    let mut data_sql = String::from(
        "SELECT DISTINCT
            f.id AS file_id,
            f.file_path,
            f.file_type,
            f.file_size,
            f.title,
            f.artist,
            f.bpm,
            f.musical_key,
            CASE WHEN fl_backup.id IS NOT NULL THEN 1 ELSE 0 END AS backed_up
         FROM files f
         JOIN file_resolved_tags frt ON frt.file_id = f.id AND frt.tag_name = ?
         JOIN file_locations fl_local ON fl_local.file_id = f.id AND fl_local.location_type = 'local'
         LEFT JOIN file_locations fl_backup ON fl_backup.file_id = f.id AND fl_backup.location_type = 'backup'
         WHERE (f.bpm IS NULL OR f.musical_key IS NULL)"
    );

    // Optional format filter
    use std::fmt::Write;
    if let Some(ref format) = query.format {
        write!(count_sql, " AND f.file_type = ?").unwrap();
        write!(data_sql, " AND f.file_type = ?").unwrap();
    }

    write!(
        data_sql,
        " ORDER BY COALESCE(f.artist, ''), COALESCE(f.title, '') LIMIT ?"
    )
    .unwrap();

    // Execute count
    let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql).bind(&tag.name);
    if let Some(ref format) = query.format {
        count_query = count_query.bind(format);
    }
    let total_count: i64 = match count_query.fetch_one(&state.db).await {
        Ok(c) => c,
        Err(e) => return internal_error(e).into_response(),
    };

    // Execute data query
    let mut data_query = sqlx::query_as::<_, FileRow>(&data_sql).bind(&tag.name);
    if let Some(ref format) = query.format {
        data_query = data_query.bind(format);
    }
    data_query = data_query.bind(limit);
    let rows: Vec<FileRow> = match data_query.fetch_all(&state.db).await {
        Ok(r) => r,
        Err(e) => return internal_error(e).into_response(),
    };

    // Compute counts from the FULL result set (we need all rows for counts,
    // but the query is LIMITed — however we only have the limited rows here.
    // For accurate counts, we compute from the total_count query and the
    // needs_bpm/needs_key/needs_both from the limited rows.
    // Actually — let's compute needs_bpm/needs_key from the limited rows
    // as a reasonable approximation for the UI.
    let needs_bpm = rows.iter().filter(|r| r.bpm.is_none()).count();
    let needs_key = rows.iter().filter(|r| r.musical_key.is_none()).count();
    let needs_both = rows
        .iter()
        .filter(|r| r.bpm.is_none() && r.musical_key.is_none())
        .count();

    let files: Vec<NeedsAnalysisFile> = rows
        .into_iter()
        .map(|r| NeedsAnalysisFile {
            file_id: r.file_id,
            file_path: r.file_path,
            file_type: r.file_type,
            local_size: r.file_size,
            title: r.title,
            artist: r.artist,
            needs_bpm: r.bpm.is_none(),
            needs_key: r.musical_key.is_none(),
            backed_up: r.backed_up == 1,
        })
        .collect();

    let response = NeedsAnalysisResponse {
        tag_id: id,
        tag_name: tag.name,
        file_count: total_count as usize,
        needs_bpm,
        needs_key,
        needs_both,
        files,
    };

    Json(ApiResponse { data: response }).into_response()
}

/// GET /api/tags/unreviewed
async fn unreviewed_tags_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Get unreviewed tags + counts
    let (reviewed, unreviewed) = match get_tag_review_counts(&state.db).await {
        Ok(counts) => counts,
        Err(e) => {
            return internal_error(format!("Failed to get review counts: {}", e)).into_response();
        }
    };

    let tags = match get_unreviewed_tags(&state.db).await {
        Ok(tags) => tags,
        Err(e) => {
            return internal_error(format!("Failed to get unreviewed tags: {}", e)).into_response();
        }
    };

    let queue: Vec<UnreviewedTagItem> = tags
        .into_iter()
        .map(|t| UnreviewedTagItem {
            id: t.id,
            name: t.name,
        })
        .collect();

    Json(ApiResponse {
        data: UnreviewedTagsResponse {
            total_unreviewed: unreviewed,
            total_reviewed: reviewed,
            queue,
        },
    })
    .into_response()
}

/// GET /api/tags/service-coverage
async fn tags_service_coverage_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Total tag count
    let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tags")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    // Tags with matching Spotify playlists
    let spotify = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(DISTINCT tag_id) FROM v_tag_playlist WHERE service = 'spotify'"#,
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    // Tags with matching SoundCloud playlists
    let soundcloud = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(DISTINCT tag_id) FROM v_tag_playlist WHERE service = 'soundcloud'"#,
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    // Tags with matching YouTube playlists
    let youtube = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(DISTINCT tag_id) FROM v_tag_playlist WHERE service = 'youtube'"#,
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    Json(ApiResponse {
        data: serde_json::json!({
            "total": total,
            "spotify": spotify,
            "soundcloud": soundcloud,
            "youtube": youtube,
        }),
    })
    .into_response()
}

/// PUT /api/tags/{id}/categorize
async fn categorize_tag_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<CategorizeRequest>,
) -> impl IntoResponse {
    // 1. Hole alten Tag (für alte category_id)
    let _old_tag = match get_tag_by_id(&state.db, id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Tag {} not found", id),
                }),
            )
                .into_response();
        }
        Err(e) => {
            return internal_error(e).into_response();
        }
    };

    // 2. Update category_id + reviewed_at
    match db_categorize_tag(&state.db, id, request.category_id).await {
        Ok(tag) => {
            // 3. Embedding-Cache aktualisieren (falls Modell geladen)
            let cache = state.embeddings.lock().await;
            if let Some(ref model) = *cache {
                // Hole oder berechne Embedding für den Tag
                let embedding_blob = match get_tag_embedding(&state.db, tag.id).await {
                    Ok(Some(blob)) => Some(blob),
                    _ => {
                        // Embedding berechnen und speichern
                        match model.embed_text(&tag.name) {
                            Ok(vec) => {
                                let blob = serialize_embedding(&vec);
                                let _ = upsert_tag_embedding(
                                    &state.db,
                                    tag.id,
                                    &blob,
                                    "all-MiniLM-L6-v2",
                                )
                                .await;
                                Some(blob)
                            }
                            Err(_) => None,
                        }
                    }
                };

                if let Some(blob) = embedding_blob
                    && let Ok(_vec) = deserialize_embedding(&blob)
                {
                    // Aktualisiere Cache (in-Memory Category Means)
                    tracing::debug!(
                        "Updated embedding for tag '{}' -> category {}",
                        tag.name,
                        request.category_id
                    );
                }

                // Invalidate category means cache so the next suggest recomputes
                *state.category_means.lock().await = None;
            }

            // API-Tag mit Category-Info zurückgeben
            match get_tag_with_category(&state.db, tag.id).await {
                Ok(Some(api_tag)) => Json(ApiResponse { data: api_tag }).into_response(),
                Ok(None) => Json(ApiResponse {
                    data: crate::api::types::Tag {
                        id: tag.id,
                        name: tag.name,
                        category: None,
                        category_icon: None,
                        created_at: None,
                        backpack: None,
                    },
                })
                .into_response(),
                Err(e) => internal_error(format!("Failed to fetch tag after categorize: {}", e))
                    .into_response(),
            }
        }
        Err(e) => internal_error(e).into_response(),
    }
}

/// POST /api/tags/bulk-categorize
async fn bulk_categorize_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<BulkCategorizeRequest>,
) -> impl IntoResponse {
    if request.tag_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "tagIds must not be empty".to_string(),
            }),
        )
            .into_response();
    }
    match bulk_categorize_tags(&state.db, &request.tag_ids, request.category_id).await {
        Ok(count) => Json(ApiResponse {
            data: serde_json::json!({ "updated": count }),
        })
        .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/tags/{id}/suggest
async fn suggest_category_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // 1. Tag aus DB holen
    let tag = match get_tag_by_id(&state.db, id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Tag {} not found", id),
                }),
            )
                .into_response();
        }
        Err(e) => {
            return internal_error(e).into_response();
        }
    };

    // 2. Embedding-Modell laden (lazy)
    {
        let mut cache = state.embeddings.lock().await;
        if cache.is_none() {
            match EmbeddingModel::new() {
                Ok(model) => {
                    *cache = Some(model);
                }
                Err(e) => {
                    return internal_error(format!("Failed to load embedding model: {}", e))
                        .into_response();
                }
            }
        }
    }

    // 3. Tag-Embedding holen oder berechnen
    let tag_embedding = match get_tag_embedding(&state.db, tag.id).await {
        Ok(Some(blob)) => match deserialize_embedding(&blob) {
            Ok(vec) => vec,
            Err(_) => {
                // Neu berechnen
                let cache = state.embeddings.lock().await;
                let model = match cache.as_ref() {
                    Some(m) => m,
                    None => {
                        return internal_error("Embedding model not loaded").into_response();
                    }
                };
                match model.embed_text(&tag.name) {
                    Ok(vec) => {
                        let blob = serialize_embedding(&vec);
                        let _ = upsert_tag_embedding(&state.db, tag.id, &blob, "all-MiniLM-L6-v2")
                            .await;
                        vec
                    }
                    Err(e) => {
                        return internal_error(format!("Failed to compute embedding: {}", e))
                            .into_response();
                    }
                }
            }
        },
        _ => {
            // Neu berechnen
            let cache = state.embeddings.lock().await;
            let model = match cache.as_ref() {
                Some(m) => m,
                None => {
                    return internal_error("Embedding model not loaded").into_response();
                }
            };
            match model.embed_text(&tag.name) {
                Ok(vec) => {
                    let blob = serialize_embedding(&vec);
                    let _ =
                        upsert_tag_embedding(&state.db, tag.id, &blob, "all-MiniLM-L6-v2").await;
                    vec
                }
                Err(e) => {
                    return internal_error(format!("Failed to compute embedding: {}", e))
                        .into_response();
                }
            }
        }
    };

    // 4. Alle Kategorien holen (für die Buttons + AI-Suggestion)
    let categories = match get_tag_categories(&state.db).await {
        Ok(cats) => cats,
        Err(e) => {
            return internal_error(format!("Failed to get categories: {}", e)).into_response();
        }
    };

    // Phase is technical/prefilled — filter it out from the UI and AI suggestions
    let phase_id = categories.iter().find(|c| c.prefix == "P").map(|c| c.id);

    let api_categories: Vec<crate::api::types::TagCategory> = categories
        .iter()
        .filter(|c| Some(c.id) != phase_id)
        .map(|c| crate::api::types::TagCategory {
            id: c.id,
            name: c.name.clone(),
            prefix: Some(c.prefix.clone()),
            icon: c.icon.clone(),
            is_default: c.is_default,
            sort_order: c.sort_order,
            created_at: Some(c.created_at),
        })
        .collect();

    // 5. Category-Embeddings berechnen (skip Setlist + Phase)
    let skip_ids: Vec<i64> = categories
        .iter()
        .filter(|c| c.is_default || Some(c.id) == phase_id)
        .map(|c| c.id)
        .collect();

    let category_embeddings = {
        let mut cache = state.category_means.lock().await;
        if let Some(ref cached) = *cache {
            cached.clone()
        } else {
            let mut means = std::collections::HashMap::new();
            for cat in &categories {
                if skip_ids.contains(&cat.id) {
                    continue;
                }
                let rows = match get_embeddings_by_category(&state.db, cat.id).await {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                if rows.is_empty() {
                    continue;
                }
                let mut vectors = Vec::new();
                for (_tid, blob) in &rows {
                    if let Ok(vec) = deserialize_embedding(blob) {
                        vectors.push(vec);
                    }
                }
                if vectors.is_empty() {
                    continue;
                }
                let mean = mean_embedding(&vectors);
                means.insert(cat.id, (cat.name.clone(), mean));
            }
            *cache = Some(means.clone());
            means
        }
    };

    // 6. Similarity berechnen
    let suggestion = suggest_category(&tag_embedding, &category_embeddings, -1);

    let (sug_id, sug_name, confidence) = match suggestion {
        Some(s) => (s.category_id, s.category_name, s.confidence),
        None => {
            // Fallback: erste nicht-default, nicht-Phase Kategorie
            let fallback = categories
                .iter()
                .find(|c| !c.is_default && Some(c.id) != phase_id);
            match fallback {
                Some(c) => (c.id, c.name.clone(), 0.0),
                None => (-1, "None".to_string(), 0.0),
            }
        }
    };

    // 7. Service connections abfragen (Spotify/SoundCloud/YouTube)
    let services = crate::db::get_tag_service_connections(&state.db, &tag.name)
        .await
        .unwrap_or(ServiceConnections {
            spotify: false,
            soundcloud: false,
            youtube: false,
        });

    Json(ApiResponse {
        data: CategorySuggestionResponse {
            suggested_category_id: sug_id,
            suggested_category_name: sug_name,
            confidence,
            all_categories: api_categories,
            service_connections: services,
        },
    })
    .into_response()
}

/// POST /api/tags/bulk-import
async fn bulk_import_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<BulkImportRequest>,
) -> impl IntoResponse {
    let names: Vec<String> = request.entries.iter().map(|e| e.name.clone()).collect();
    let category_map: HashMap<i64, String> = {
        let cats = match get_tag_categories(&state.db).await {
            Ok(c) => c,
            Err(e) => {
                return internal_error(e).into_response();
            }
        };
        cats.into_iter().map(|c| (c.id, c.name)).collect()
    };

    let checked = match bulk_check_tags(&state.db, &names).await {
        Ok(c) => c,
        Err(e) => {
            return internal_error(e).into_response();
        }
    };

    // Build a lookup: name -> (name, category_id) from request
    let request_map: HashMap<&str, i64> = request
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e.category_id))
        .collect();

    let mut results = Vec::new();
    for (name, current_cat_id, current_cat_name) in checked {
        let target_cat_id = request_map.get(name.as_str()).copied().unwrap_or(-1);
        let target_cat_name = category_map
            .get(&target_cat_id)
            .cloned()
            .unwrap_or_else(|| "Unknown".to_string());

        let status = match (current_cat_id, &current_cat_name) {
            (Some(cid), Some(_)) if cid == target_cat_id => "matched",
            (Some(_), _) => "conflict",
            (None, _) => "not_found",
        };

        // Get the tag ID if it exists
        let existing_tag_id = if current_cat_id.is_some() {
            match crate::db::get_tag_by_name(&state.db, &name).await {
                Ok(Some(t)) => Some(t.id),
                _ => None,
            }
        } else {
            None
        };

        results.push(BulkImportResult {
            name,
            status: status.to_string(),
            tag_id: existing_tag_id,
            category_id: target_cat_id,
            category_name: target_cat_name,
            current_category_id: current_cat_id,
            current_category_name: current_cat_name,
        });
    }

    Json(ApiResponse { data: results }).into_response()
}

/// POST /api/tags/bulk-resolve
async fn bulk_resolve_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<BulkResolveRequest>,
) -> impl IntoResponse {
    let category_map: HashMap<i64, String> = {
        let cats = match get_tag_categories(&state.db).await {
            Ok(c) => c,
            Err(e) => {
                return internal_error(e).into_response();
            }
        };
        cats.into_iter().map(|c| (c.id, c.name)).collect()
    };

    let mut results = Vec::new();
    let mut any_created = false;
    for entry in &request.entries {
        let cat_name = category_map
            .get(&entry.category_id)
            .cloned()
            .unwrap_or_else(|| "Unknown".to_string());

        match entry.action.as_str() {
            "create" => {
                match bulk_create_tags(&state.db, &[(entry.name.clone(), entry.category_id)]).await
                {
                    Ok(tags) => {
                        for t in tags {
                            results.push(BulkResolveResult {
                                name: entry.name.clone(),
                                status: "created".to_string(),
                                tag_id: Some(t.id),
                                category_id: entry.category_id,
                                category_name: cat_name.clone(),
                                error: None,
                            });
                            any_created = true;
                        }
                    }
                    Err(e) => {
                        results.push(BulkResolveResult {
                            name: entry.name.clone(),
                            status: "error".to_string(),
                            tag_id: None,
                            category_id: entry.category_id,
                            category_name: cat_name.clone(),
                            error: Some(e.to_string()),
                        });
                    }
                }
            }
            "move" => {
                match bulk_update_tags(&state.db, &[(entry.name.clone(), entry.category_id)]).await
                {
                    Ok(tags) => {
                        for t in tags {
                            results.push(BulkResolveResult {
                                name: entry.name.clone(),
                                status: "moved".to_string(),
                                tag_id: Some(t.id),
                                category_id: entry.category_id,
                                category_name: cat_name.clone(),
                                error: None,
                            });
                        }
                    }
                    Err(e) => {
                        results.push(BulkResolveResult {
                            name: entry.name.clone(),
                            status: "error".to_string(),
                            tag_id: None,
                            category_id: entry.category_id,
                            category_name: cat_name.clone(),
                            error: Some(e.to_string()),
                        });
                    }
                }
            }
            "review" => {
                match bulk_review_tags(&state.db, std::slice::from_ref(&entry.name)).await {
                    Ok(_count) => {
                        // Get tag id
                        let tag_id = match get_tag_by_name(&state.db, &entry.name).await {
                            Ok(Some(t)) => Some(t.id),
                            _ => None,
                        };
                        results.push(BulkResolveResult {
                            name: entry.name.clone(),
                            status: "reviewed".to_string(),
                            tag_id,
                            category_id: entry.category_id,
                            category_name: cat_name.clone(),
                            error: None,
                        });
                    }
                    Err(e) => {
                        results.push(BulkResolveResult {
                            name: entry.name.clone(),
                            status: "error".to_string(),
                            tag_id: None,
                            category_id: entry.category_id,
                            category_name: cat_name.clone(),
                            error: Some(e.to_string()),
                        });
                    }
                }
            }
            _ => {
                results.push(BulkResolveResult {
                    name: entry.name.clone(),
                    status: "error".to_string(),
                    tag_id: None,
                    category_id: entry.category_id,
                    category_name: cat_name,
                    error: Some(format!("Unknown action: {}", entry.action)),
                });
            }
        }
    }

    // Auto-recompute embeddings and similarities if any tags were created
    if any_created {
        start_recompute_embeddings_task(&state.task_manager, &state.db).await;
        // Refresh materialized file_resolved_tags since tag→playlist links may have changed
        let _ = refresh_file_resolved_tags(&state.db).await;
        let _ = refresh_track_resolved_tags(&state.db).await;
    }

    Json(ApiResponse { data: results }).into_response()
}

/// GET /api/tags/{id}/parents
async fn tag_parents_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_tag_parents(&state.db, id).await {
        Ok(parents) => {
            // Convert Tag to API Tag with category info
            let mut api_tags: Vec<crate::api::types::Tag> = Vec::new();
            for parent in parents {
                if let Ok(Some(api_tag)) = get_tag_with_category(&state.db, parent.id).await {
                    api_tags.push(api_tag);
                }
            }
            Json(ApiResponse { data: api_tags }).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

/// PUT /api/tags/{id}/parents
async fn tag_parents_set_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<SetTagParentsRequest>,
) -> impl IntoResponse {
    match set_tag_parents(&state.db, id, &request.parent_tag_ids).await {
        Ok(parents) => {
            // Refresh materialized tables since parent resolution changed
            let _ = refresh_file_resolved_tags(&state.db).await;
            let _ = refresh_track_resolved_tags(&state.db).await;

            let mut api_tags: Vec<crate::api::types::Tag> = Vec::new();
            for parent in parents {
                if let Ok(Some(api_tag)) = get_tag_with_category(&state.db, parent.id).await {
                    api_tags.push(api_tag);
                }
            }
            Json(ApiResponse { data: api_tags }).into_response()
        }
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("Only Setlist tags")
                || err_msg.contains("own parent")
                || err_msg.contains("not found")
            {
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse { error: err_msg }),
                )
                    .into_response()
            } else {
                internal_error(e).into_response()
            }
        }
    }
}

/// GET /api/tags/{id}/children
async fn tag_children_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_tag_children(&state.db, id).await {
        Ok(children) => {
            let mut api_tags: Vec<crate::api::types::Tag> = Vec::new();
            for child in children {
                if let Ok(Some(api_tag)) = get_tag_with_category(&state.db, child.id).await {
                    api_tags.push(api_tag);
                }
            }
            Json(ApiResponse { data: api_tags }).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

/// PUT /api/tags/{id}/backpack
async fn tag_backpack_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let backpack = body
        .get("backpack")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    match set_tag_backpack(&state.db, id, backpack).await {
        Ok(()) => {
            // When toggling TO backpack, trigger a background sync task
            if backpack {
                let task_id =
                    crate::tasks::start_backpack_sync_task(&state.task_manager, &state.db).await;
                if task_id.is_empty() {
                    return Json(ApiResponse {
                        data: serde_json::json!({
                            "backpack": true,
                            "taskId": null,
                            "message": "Backpack sync already in progress",
                        }),
                    })
                    .into_response();
                }
                Json(ApiResponse {
                    data: serde_json::json!({
                        "backpack": true,
                        "taskId": task_id,
                    }),
                })
                .into_response()
            } else {
                Json(ApiResponse {
                    data: serde_json::json!({"backpack": false}),
                })
                .into_response()
            }
        }
        Err(e) => internal_error(e).into_response(),
    }
}

// ── Tag Bundle Handlers ──────────────────────────────────────────────────

/// GET /api/tags/bundles — list all bundle tags with member count
async fn tag_bundles_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TagBundlesQuery>,
) -> impl IntoResponse {
    match get_bundle_tags_with_counts(&state.db, params.search.as_deref()).await {
        Ok(bundles) => {
            // Enrich with category info
            let mut result = Vec::new();
            for (tag, member_count) in bundles {
                let api_tag = get_tag_with_category(&state.db, tag.id)
                    .await
                    .ok()
                    .flatten();
                let category_name = api_tag.as_ref().and_then(|t| t.category.clone());
                let category_id = Some(tag.category_id);
                result.push(serde_json::json!({
                    "id": tag.id,
                    "name": tag.name,
                    "categoryId": category_id,
                    "categoryName": category_name,
                    "memberCount": member_count,
                    "backpack": tag.backpack,
                }));
            }
            Json(ApiResponse { data: result }).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/tags/{id}/bundle-members
async fn tag_bundle_members_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_bundle_members(&state.db, id).await {
        Ok(members) => {
            let mut api_tags: Vec<crate::api::types::Tag> = Vec::new();
            for member in members {
                if let Ok(Some(api_tag)) = get_tag_with_category(&state.db, member.id).await {
                    api_tags.push(api_tag);
                }
            }
            Json(ApiResponse { data: api_tags }).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

/// PUT /api/tags/{id}/bundle-members
async fn tag_bundle_members_set_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<SetBundleMembersRequest>,
) -> impl IntoResponse {
    match set_bundle_members(&state.db, id, &request.member_tag_ids).await {
        Ok(members) => {
            // Refresh materialized tables since bundle resolution changed
            let _ = refresh_file_resolved_tags(&state.db).await;
            let _ = refresh_track_resolved_tags(&state.db).await;

            let mut api_tags: Vec<crate::api::types::Tag> = Vec::new();
            for member in members {
                if let Ok(Some(api_tag)) = get_tag_with_category(&state.db, member.id).await {
                    api_tags.push(api_tag);
                }
            }
            Json(ApiResponse { data: api_tags }).into_response()
        }
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("Circular bundle")
                || err_msg.contains("cannot be a member of itself")
                || err_msg.contains("not found")
            {
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse { error: err_msg }),
                )
                    .into_response()
            } else {
                internal_error(e).into_response()
            }
        }
    }
}

/// GET /api/tags/{id}/bundle-of
async fn tag_bundle_of_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_bundle_of(&state.db, id).await {
        Ok(bundles) => {
            let mut api_tags: Vec<crate::api::types::Tag> = Vec::new();
            for bundle in bundles {
                if let Ok(Some(api_tag)) = get_tag_with_category(&state.db, bundle.id).await {
                    api_tags.push(api_tag);
                }
            }
            Json(ApiResponse { data: api_tags }).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

// ── Router ─────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/tags", get(tags_handler).post(create_tag_handler))
        .route("/api/tags/count", get(tags_count_handler))
        .route("/api/tags/curation-queue", get(curation_queue_handler))
        .route(
            "/api/tags/service-coverage",
            get(tags_service_coverage_handler),
        )
        .route(
            "/api/tags/from-playlists",
            get(tags_from_playlists_handler_wrapper),
        )
        .route(
            "/api/tags/create-from-playlists",
            post(tags_create_from_playlists_handler_wrapper),
        )
        .route("/api/tags/unreviewed", get(unreviewed_tags_handler))
        .route("/api/tags/bulk-categorize", post(bulk_categorize_handler))
        .route("/api/tags/bulk-import", post(bulk_import_handler))
        .route("/api/tags/bulk-resolve", post(bulk_resolve_handler))
        .route(
            "/api/tags/{id}",
            get(get_tag_handler)
                .put(update_tag_handler)
                .delete(delete_tag_handler),
        )
        .route("/api/tags/{id}/categorize", put(categorize_tag_handler))
        .route("/api/tags/{id}/suggest", get(suggest_category_handler))
        .route(
            "/api/tags/{id}/parents",
            get(tag_parents_handler).put(tag_parents_set_handler),
        )
        .route("/api/tags/{id}/children", get(tag_children_handler))
        .route("/api/tags/{id}/backpack", put(tag_backpack_handler))
        .route(
            "/api/tags/{id}/needs-analysis",
            get(tag_needs_analysis_handler),
        )
        .route("/api/tags/bundles", get(tag_bundles_handler))
        .route(
            "/api/tags/{id}/bundle-members",
            get(tag_bundle_members_handler).put(tag_bundle_members_set_handler),
        )
        .route("/api/tags/{id}/bundle-of", get(tag_bundle_of_handler))
        .route(
            "/api/tag-categories",
            get(tag_categories_handler).post(create_tag_category_handler),
        )
        .route(
            "/api/tag-categories/{id}",
            get(get_tag_category_handler)
                .put(update_tag_category_handler)
                .delete(delete_tag_category_handler),
        )
        .route("/api/tag-energy-levels", get(tag_energy_levels_handler))
        .route(
            "/api/tag-energy-levels/batch",
            put(reorder_tags_batch_handler),
        )
        .route(
            "/api/tag-energy-levels/{tag_id}",
            put(set_tag_energy_level_handler).delete(delete_tag_energy_level_handler),
        )
}
