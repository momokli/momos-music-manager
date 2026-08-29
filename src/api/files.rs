//! File-domain API: handlers for `/api/files*` endpoints.
//!
//! Extracted from `legacy.rs` — every handler is a verbatim copy.

use anyhow::Result;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::process::Command as TokioCommand;

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, apply_sort, internal_error};
use crate::comment::generate_target_comment;
use crate::db::{
    File, compute_target_comment, compute_target_comments_batch, find_tag_similar_tracks,
    get_file_detail, get_file_variants, get_key_comparison,
};
use crate::external_tools::resolve_tool;
use crate::tasks::start_write_comment_task;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkSyncRequest {
    linked_only: Option<bool>,
    tags: Option<Vec<String>>,
    non_default_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiFile {
    pub id: i64,
    pub file_path: String,
    pub file_hash: String,
    pub file_type: String,
    pub file_size: i64,
    pub last_modified: i64,
    pub isrc: Option<String>,
    pub last_scanned: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<i32>,
    pub total_tracks: Option<i32>,
    pub disc_number: Option<i32>,
    pub total_discs: Option<i32>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub composer: Option<String>,
    pub comment: Option<String>,
    pub duration_ms: Option<i64>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i32>,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub rating: Option<i32>,
    pub play_count: Option<i32>,
    pub last_played: Option<i64>,
    pub spotify_id: Option<String>,
    pub soundcloud_id: Option<String>,
    pub youtube_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub matched_services: Vec<String>,
    pub comment_target: String,
    pub comment_needs_update: bool,
    pub backed_up: bool,
    pub is_local: bool,
    pub has_stem: bool,
    pub safe_to_delete: bool,
}

impl From<File> for ApiFile {
    fn from(file: File) -> Self {
        ApiFile {
            id: file.id,
            file_path: file.file_path,
            file_hash: file.file_hash,
            file_type: file.file_type,
            file_size: file.file_size,
            last_modified: file.last_modified,
            isrc: file.isrc,
            last_scanned: file.last_scanned,
            title: file.title.unwrap_or_default(),
            artist: file.artist.unwrap_or_default(),
            album: file.album,
            album_artist: file.album_artist,
            track_number: file.track_number,
            total_tracks: file.total_tracks,
            disc_number: file.disc_number,
            total_discs: file.total_discs,
            genre: file.genre,
            year: file.year,
            composer: file.composer,
            comment: file.comment,
            duration_ms: file.duration_ms,
            bitrate: file.bitrate,
            sample_rate: file.sample_rate,
            channels: file.channels,
            bpm: file.bpm,
            musical_key: file.musical_key,
            rating: Some(file.rating),
            play_count: Some(file.play_count),
            last_played: file.last_played,
            spotify_id: file.spotify_id,
            soundcloud_id: file.soundcloud_id,
            youtube_id: file.youtube_id,
            created_at: file.created_at,
            updated_at: file.updated_at,
            matched_services: vec![],
            comment_target: String::new(),
            comment_needs_update: false,
            backed_up: false,
            is_local: false,
            has_stem: false,
            safe_to_delete: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub rating_min: Option<i32>,
    pub play_count_min: Option<i32>,
    pub key: Option<String>,
    pub tags: Option<String>,
    pub search: Option<String>,
    pub linked_only: Option<bool>,
    pub unlinked: Option<bool>,
    pub non_default_only: Option<bool>,
    pub selected_services: Option<String>,
    pub pmv_categories: Option<String>,
    pub pmv_aggregate: Option<String>,
    pub file_types: Option<String>,
    pub comment_statuses: Option<String>,
    pub backed_up: Option<bool>,
    pub is_local: Option<bool>,
    pub safe_to_delete: Option<bool>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesBulkRequest {
    pub file_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FilesBulkCommentCountResponse {
    pub total_files: usize,
    pub files_needing_update: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FilesBulkWriteCommentsResponse {
    pub task_id: String,
    pub file_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileDebugCommentResponse {
    pub file_id: i64,
    pub title: String,
    pub artist: String,
    pub tag_rows: Vec<DebugTagRow>,
    pub pmv: DebugPmv,
    pub generated_comment: String,
    pub current_comment: Option<String>,
    pub playlists: Vec<DebugPlaylist>,
    pub matched_tags: Vec<DebugMatchedTag>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DebugTagRow {
    pub tag_name: String,
    pub prefix: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DebugPmv {
    pub phase: bool,
    pub mood: bool,
    pub vibe: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DebugPlaylist {
    pub name: String,
    pub service: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DebugMatchedTag {
    pub tag_id: i64,
    pub tag_name: String,
    pub category_name: String,
    pub has_parents: bool,
}

/// Filter params for "select all" operations — same filters as FilesQuery
/// but without pagination/sort. Sent as JSON body via POST.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesFilterAll {
    pub search: Option<String>,
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub key: Option<String>,
    pub tags: Option<String>,
    pub linked_only: Option<bool>,
    pub unlinked: Option<bool>,
    pub non_default_only: Option<bool>,
    pub selected_services: Option<String>,
    pub pmv_categories: Option<String>,
    pub pmv_aggregate: Option<String>,
    pub file_types: Option<String>,
    pub comment_statuses: Option<String>,
    pub backed_up: Option<bool>,
    pub is_local: Option<bool>,
    pub safe_to_delete: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NeedsUpdateCountQuery {
    pub linked_only: Option<bool>,
    pub tags: Option<String>,
    pub non_default_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyComparisonQuery {
    /// Filter by tag name (optional — returns all linked files if omitted)
    tag: Option<String>,
    /// Max results (default 500)
    limit: Option<i64>,
}

// ── Handlers ──────────────────────────────────────────────────────────────

async fn files_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FilesQuery>,
) -> impl IntoResponse {
    match get_files(&state.db, &query).await {
        Ok(files) => Json(ApiResponse { data: files }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn files_count_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FilesQuery>,
) -> impl IntoResponse {
    match get_files_count(&state.db, &query).await {
        Ok(count) => Json(ApiResponse { data: count }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/files/latest
/// Returns the 5 most recently added files (by created_at)
async fn files_latest_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let files = sqlx::query_as::<_, File>("SELECT * FROM files ORDER BY created_at DESC LIMIT 5")
        .fetch_all(&state.db)
        .await;

    match files {
        Ok(files) => {
            let api_files: Vec<ApiFile> = files
                .into_iter()
                .map(|f| {
                    // Minimal conversion without comment computation
                    let mut af = ApiFile::from(f);
                    af.matched_services = vec![];
                    af.comment_target = String::new();
                    af.comment_needs_update = false;
                    af
                })
                .collect();
            Json(ApiResponse { data: api_files }).into_response()
        }
        Err(e) => internal_error(format!("Failed to fetch latest files: {}", e)).into_response(),
    }
}

/// GET /api/files/needs-update-count
/// Returns the count of files whose comment differs from the computed target comment.
/// Accepts optional filter params (linkedOnly, tags, nonDefaultOnly) to scope the count.
async fn files_needs_update_count_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<NeedsUpdateCountQuery>,
) -> impl IntoResponse {
    // Build dynamic SQL with the same filter pattern as get_files/bulk_sync_handler
    let mut sql = String::from("SELECT * FROM files WHERE 1=1");
    let mut tag_params: Vec<String> = Vec::new();

    if query.linked_only.unwrap_or(false) {
        sql.push_str(" AND EXISTS (SELECT 1 FROM v_file_track_link v WHERE v.file_id = files.id)");
    }

    if let Some(ref tags_str) = query.tags
        && !tags_str.is_empty()
    {
        let lowered: Vec<String> = tags_str
            .split(',')
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if !lowered.is_empty() {
            let placeholders: Vec<String> = lowered.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(TRIM(frt.tag_name)) IN ({}))",
                    placeholders.join(",")
                ));
            tag_params = lowered;
        }
    }

    if query.non_default_only.unwrap_or(false) {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND frt.is_default = FALSE)",
        );
    }

    sql.push_str(" ORDER BY id");

    let mut q = sqlx::query_as::<_, File>(&sql);
    for p in &tag_params {
        q = q.bind(p);
    }

    match q.fetch_all(&state.db).await {
        Ok(files) => {
            let mut count = 0i64;
            for file in &files {
                match compute_target_comment(&state.db, file.id).await {
                    Ok(target_comment) => {
                        let current_comment = file.comment.as_deref().unwrap_or("");
                        if current_comment != target_comment {
                            count += 1;
                        }
                    }
                    Err(_) => continue,
                }
            }
            Json(ApiResponse { data: count }).into_response()
        }
        Err(e) => {
            internal_error(format!("Failed to count files needing update: {}", e)).into_response()
        }
    }
}

/// POST /api/files/needs-comment-count
/// Takes a list of file IDs and returns how many have comments that need updating.
async fn files_needs_comment_count_by_ids_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FilesBulkRequest>,
) -> impl IntoResponse {
    let total_files = body.file_ids.len();
    if total_files == 0 {
        return Json(ApiResponse {
            data: FilesBulkCommentCountResponse {
                total_files: 0,
                files_needing_update: 0,
            },
        })
        .into_response();
    }

    // Fetch files by ID
    let placeholders: Vec<String> = body.file_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT * FROM files WHERE id IN ({})",
        placeholders.join(",")
    );
    let mut q = sqlx::query_as::<_, crate::db::File>(&sql);
    for id in &body.file_ids {
        q = q.bind(id);
    }

    let files = match q.fetch_all(&state.db).await {
        Ok(f) => f,
        Err(e) => {
            return internal_error(format!("Failed to fetch files: {}", e)).into_response();
        }
    };

    let mut files_needing_update = 0usize;
    for file in &files {
        match compute_target_comment(&state.db, file.id).await {
            Ok(target) => {
                if file.comment.as_deref() != Some(&target) {
                    files_needing_update += 1;
                }
            }
            Err(e) => {
                tracing::warn!("Could not compute target for file {}: {}", file.id, e);
                files_needing_update += 1;
            }
        }
    }

    Json(ApiResponse {
        data: FilesBulkCommentCountResponse {
            total_files,
            files_needing_update,
        },
    })
    .into_response()
}

/// POST /api/files/write-comments-by-ids
/// Takes a list of file IDs, computes which need comment updates,
/// and queues a write-comment task for those files.
async fn files_write_comments_by_ids_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FilesBulkRequest>,
) -> impl IntoResponse {
    if body.file_ids.is_empty() {
        return Json(ApiResponse {
            data: FilesBulkWriteCommentsResponse {
                task_id: String::new(),
                file_count: 0,
            },
        })
        .into_response();
    }

    // Fetch files by ID
    let placeholders: Vec<String> = body.file_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT * FROM files WHERE id IN ({})",
        placeholders.join(",")
    );
    let mut q = sqlx::query_as::<_, crate::db::File>(&sql);
    for id in &body.file_ids {
        q = q.bind(id);
    }

    let files = match q.fetch_all(&state.db).await {
        Ok(f) => f,
        Err(e) => {
            return internal_error(format!("Failed to fetch files: {}", e)).into_response();
        }
    };

    // Filter to only files that need an update
    let mut needs_update: Vec<i64> = Vec::new();
    for file in &files {
        match compute_target_comment(&state.db, file.id).await {
            Ok(target) => {
                if file.comment.as_deref() != Some(&target) {
                    needs_update.push(file.id);
                }
            }
            Err(e) => {
                tracing::warn!("Could not compute target for file {}: {}", file.id, e);
                needs_update.push(file.id);
            }
        }
    }

    if needs_update.is_empty() {
        return Json(ApiResponse {
            data: FilesBulkWriteCommentsResponse {
                task_id: String::new(),
                file_count: 0,
            },
        })
        .into_response();
    }

    let file_count = needs_update.len();
    let task_id = start_write_comment_task(&state.task_manager, &state.db, needs_update).await;

    Json(ApiResponse {
        data: FilesBulkWriteCommentsResponse {
            task_id,
            file_count,
        },
    })
    .into_response()
}

/// Build the WHERE clause for file filters. Returns (sql_fragment, param_values).
/// Shared by the "select all" handlers to avoid duplicating filter logic.
fn build_files_filter_sql(filter: &FilesFilterAll) -> String {
    let mut sql = String::from("SELECT * FROM files WHERE 1=1");

    if let Some(ref search) = filter.search
        && !search.is_empty()
    {
        sql.push_str(" AND (title LIKE ? OR artist LIKE ? OR file_path LIKE ? OR genre LIKE ? OR album LIKE ? OR isrc LIKE ? OR comment LIKE ? OR EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND frt.tag_name LIKE ?))");
    }

    if filter.bpm_min.is_some() {
        sql.push_str(" AND bpm >= ?");
    }

    if filter.bpm_max.is_some() {
        sql.push_str(" AND bpm <= ?");
    }

    if let Some(ref key_str) = filter.key {
        let keys: Vec<&str> = key_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !keys.is_empty() {
            let placeholders: Vec<String> = keys.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND musical_key IN ({})", placeholders.join(",")));
        }
    }

    if filter.linked_only.unwrap_or(false) {
        sql.push_str(" AND EXISTS (SELECT 1 FROM v_file_track_link v WHERE v.file_id = files.id)");
    }

    if filter.unlinked.unwrap_or(false) {
        sql.push_str(
            " AND NOT EXISTS (SELECT 1 FROM v_file_track_link v WHERE v.file_id = files.id)",
        );
    }

    if filter.non_default_only.unwrap_or(false) {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND frt.is_default = FALSE)",
        );
    }

    // Service filter
    if let Some(ref services_str) = filter.selected_services {
        let services: Vec<&str> = services_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !services.is_empty() {
            let placeholders: Vec<String> = services.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM v_file_track_link vf JOIN service_tracks st ON st.id = vf.track_id WHERE vf.file_id = files.id AND st.service IN ({}))",
                placeholders.join(",")
            ));
        }
    }

    // PMV filter — check file_resolved_tags.prefix (actual tag categories)
    if let Some(ref pmv_cats) = filter.pmv_categories {
        let cats: Vec<String> = pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !cats.is_empty() {
            let placeholders: Vec<String> = cats.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) IN ({}))",
                placeholders.join(",")
            ));
        }
    } else if let Some(ref pmv_agg) = filter.pmv_aggregate {
        match pmv_agg.as_str() {
            "full" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) = 'p')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) = 'm')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) = 'v')");
            }
            "partial" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) IN ('p','m','v'))");
            }
            "none" => {
                sql.push_str(" AND NOT EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) IN ('p','m','v'))");
            }
            _ => {}
        }
    }

    // File type filter
    if let Some(ref ft_str) = filter.file_types {
        let types: Vec<&str> = ft_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !types.is_empty() {
            let placeholders: Vec<String> = types.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND file_type IN ({})", placeholders.join(",")));
        }
    }

    // Tag filter: files that have any of the selected tags
    if let Some(ref tags_str) = filter.tags
        && !tags_str.is_empty()
    {
        let lowered: Vec<String> = tags_str
            .split(',')
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if !lowered.is_empty() {
            let placeholders: Vec<String> = lowered.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(TRIM(frt.tag_name)) IN ({}))",
                placeholders.join(",")
            ));
        }
    }

    // Backup filter
    if let Some(true) = filter.backed_up {
        sql.push_str(" AND EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'backup')");
    } else if let Some(false) = filter.backed_up {
        sql.push_str(" AND NOT EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'backup')");
    }

    // Local presence filter
    if let Some(true) = filter.is_local {
        sql.push_str(" AND EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'local')");
    } else if let Some(false) = filter.is_local {
        sql.push_str(" AND NOT EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'local')");
    }

    if let Some(true) = filter.safe_to_delete {
        sql.push_str(" AND EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'backup')");
        sql.push_str(" AND files.file_type != 'stem.m4a'");
        sql.push_str(" AND EXISTS (SELECT 1 FROM files f2 WHERE f2.isrc = files.isrc AND f2.isrc IS NOT NULL AND f2.file_type = 'stem.m4a')");
    }

    sql
}

/// POST /api/files/needs-comment-count-all
/// Accepts filter params and returns how many matching files need comment updates.
/// Used by the "Select all N files" feature.
async fn files_needs_comment_count_all_handler(
    State(state): State<Arc<AppState>>,
    Json(filter): Json<FilesFilterAll>,
) -> impl IntoResponse {
    let sql = build_files_filter_sql(&filter);

    let mut q = sqlx::query_as::<_, crate::db::File>(&sql);

    if let Some(ref search) = filter.search
        && !search.is_empty()
    {
        let pattern = format!("%{}%", search);
        for _ in 0..8 {
            q = q.bind(pattern.clone());
        }
    }

    if let Some(bpm_min) = filter.bpm_min {
        q = q.bind(bpm_min);
    }

    if let Some(bpm_max) = filter.bpm_max {
        q = q.bind(bpm_max);
    }

    if let Some(ref key_str) = filter.key {
        for k in key_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(k);
        }
    }

    if let Some(ref services_str) = filter.selected_services {
        for s in services_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(s);
        }
    }

    if let Some(ref ft_str) = filter.file_types {
        for t in ft_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(t);
        }
    }

    // Bind tag filter params
    if let Some(ref tags_str) = filter.tags
        && !tags_str.is_empty()
    {
        for t in tags_str
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(t);
        }
    }

    // Bind PMV filter params
    if let Some(ref pmv_cats) = filter.pmv_categories {
        for c in pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(c);
        }
    }

    let files = match q.fetch_all(&state.db).await {
        Ok(f) => f,
        Err(e) => {
            return internal_error(format!("Failed to fetch files: {}", e)).into_response();
        }
    };

    let mut files_needing_update = 0usize;
    for file in &files {
        match compute_target_comment(&state.db, file.id).await {
            Ok(target) => {
                if file.comment.as_deref() != Some(&target) {
                    files_needing_update += 1;
                }
            }
            Err(e) => {
                tracing::warn!("Could not compute target for file {}: {}", file.id, e);
                files_needing_update += 1;
            }
        }
    }

    Json(ApiResponse {
        data: FilesBulkCommentCountResponse {
            total_files: files.len(),
            files_needing_update,
        },
    })
    .into_response()
}

/// POST /api/files/write-comments-all
async fn files_write_comments_all_handler(
    State(state): State<Arc<AppState>>,
    Json(filter): Json<FilesFilterAll>,
) -> impl IntoResponse {
    let sql = build_files_filter_sql(&filter);

    let mut q = sqlx::query_as::<_, crate::db::File>(&sql);

    if let Some(ref search) = filter.search
        && !search.is_empty()
    {
        let pattern = format!("%{}%", search);
        for _ in 0..8 {
            q = q.bind(pattern.clone());
        }
    }

    if let Some(bpm_min) = filter.bpm_min {
        q = q.bind(bpm_min);
    }

    if let Some(bpm_max) = filter.bpm_max {
        q = q.bind(bpm_max);
    }

    if let Some(ref key_str) = filter.key {
        for k in key_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(k);
        }
    }

    if let Some(ref services_str) = filter.selected_services {
        for s in services_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(s);
        }
    }

    if let Some(ref ft_str) = filter.file_types {
        for t in ft_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(t);
        }
    }

    // Bind tag filter params
    if let Some(ref tags_str) = filter.tags
        && !tags_str.is_empty()
    {
        for t in tags_str
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(t);
        }
    }

    // Bind PMV filter params
    if let Some(ref pmv_cats) = filter.pmv_categories {
        for c in pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(c);
        }
    }

    let files = match q.fetch_all(&state.db).await {
        Ok(f) => f,
        Err(e) => {
            return internal_error(format!("Failed to fetch files: {}", e)).into_response();
        }
    };

    // Filter to only files that need an update
    let mut needs_update: Vec<i64> = Vec::new();
    for file in &files {
        match compute_target_comment(&state.db, file.id).await {
            Ok(target) => {
                if file.comment.as_deref() != Some(&target) {
                    needs_update.push(file.id);
                }
            }
            Err(e) => {
                tracing::warn!("Could not compute target for file {}: {}", file.id, e);
                needs_update.push(file.id);
            }
        }
    }

    if needs_update.is_empty() {
        return Json(ApiResponse {
            data: FilesBulkWriteCommentsResponse {
                task_id: String::new(),
                file_count: 0,
            },
        })
        .into_response();
    }

    let file_count = needs_update.len();
    let task_id = start_write_comment_task(&state.task_manager, &state.db, needs_update).await;

    Json(ApiResponse {
        data: FilesBulkWriteCommentsResponse {
            task_id,
            file_count,
        },
    })
    .into_response()
}

async fn files_service_links_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Total file count
    let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM files")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    // Files linked to Spotify: via v_file_track_link (ISRC + direct service_id matching)
    let spotify = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(DISTINCT v.file_id) FROM v_file_track_link v
           JOIN service_tracks st ON st.id = v.track_id AND st.service = 'spotify'"#,
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    // Files linked to SoundCloud: via v_file_track_link
    let soundcloud = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(DISTINCT v.file_id) FROM v_file_track_link v
           JOIN service_tracks st ON st.id = v.track_id AND st.service = 'soundcloud'"#,
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    // Files linked to YouTube: via v_file_track_link
    let youtube = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(DISTINCT v.file_id) FROM v_file_track_link v
           JOIN service_tracks st ON st.id = v.track_id AND st.service = 'youtube'"#,
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    // Unlinked: no direct service ID AND no isrc matches any service track
    let unlinked = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM files f
           WHERE NOT EXISTS (
             SELECT 1 FROM v_file_track_link v WHERE v.file_id = f.id
           )"#,
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
            "unlinked": unlinked,
        }),
    })
    .into_response()
}

async fn file_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_file_by_id(&state.db, id).await {
        Ok(file) => Json(ApiResponse { data: file }).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

/// GET /api/files/{id}/detail — Rich detail view with Traktor metadata,
/// linked Spotify track, audio features, tags, and playlists.
async fn file_detail_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_file_detail(&state.db, id).await {
        Ok(Some(detail)) => Json(ApiResponse { data: detail }).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "File not found".to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

/// GET /api/files/{id}/variants — Returns all file variants for a track:
/// same ISRC files + WAV source files belonging to the same stem.
async fn file_variants_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_file_variants(&state.db, id).await {
        Ok(Some(variants)) => Json(ApiResponse { data: variants }).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "File not found".to_string(),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn sync_comment_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let task_id = start_write_comment_task(&state.task_manager, &state.db, vec![id]).await;
    Json(ApiResponse {
        data: serde_json::json!({ "taskId": task_id }),
    })
    .into_response()
}

async fn bulk_sync_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BulkSyncRequest>,
) -> impl IntoResponse {
    // Build dynamic SQL to filter files based on request parameters
    let mut sql = String::from("SELECT * FROM files WHERE 1=1");
    let mut tag_params: Vec<String> = Vec::new();

    if body.linked_only.unwrap_or(false) {
        sql.push_str(" AND EXISTS (SELECT 1 FROM v_file_track_link v WHERE v.file_id = files.id)");
    }

    if let Some(ref tags) = body.tags
        && !tags.is_empty()
    {
        let lowered: Vec<String> = tags
            .iter()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if !lowered.is_empty() {
            let placeholders: Vec<String> = lowered.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(TRIM(frt.tag_name)) IN ({}))",
                    placeholders.join(",")
                ));
            tag_params = lowered;
        }
    }

    if body.non_default_only.unwrap_or(false) {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND frt.is_default = FALSE)",
        );
    }

    let mut q = sqlx::query_as::<_, crate::db::File>(&sql);
    for p in &tag_params {
        q = q.bind(p);
    }

    let file_ids = match q.fetch_all(&state.db).await {
        Ok(all_files) => {
            let mut ids = Vec::new();
            for file in &all_files {
                match compute_target_comment(&state.db, file.id).await {
                    Ok(target) => {
                        if file.comment.as_deref() != Some(&target) {
                            ids.push(file.id);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Could not compute target for file {}: {}", file.id, e);
                        ids.push(file.id);
                    }
                }
            }
            ids
        }
        Err(e) => {
            return internal_error(format!("Failed to fetch files: {}", e)).into_response();
        }
    };

    if file_ids.is_empty() {
        return Json(ApiResponse {
            data: serde_json::json!({ "taskId": null, "message": "All comments are up to date" }),
        })
        .into_response();
    }

    let task_id = start_write_comment_task(&state.task_manager, &state.db, file_ids).await;
    Json(ApiResponse {
        data: serde_json::json!({ "taskId": task_id }),
    })
    .into_response()
}

/// GET /api/files/key-comparison?tag=Collapse-capital&limit=100
///
/// Returns side-by-side comparison with match/mismatch summary.
async fn key_comparison_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<KeyComparisonQuery>,
) -> impl IntoResponse {
    match get_key_comparison(&state.db, q.tag.as_deref(), q.limit).await {
        Ok((rows, summary)) => {
            let response = serde_json::json!({
                "data": {
                    "files": rows,
                    "summary": summary
                }
            });
            Json(response).into_response()
        }
        Err(e) => {
            tracing::error!("Key comparison failed: {e:?}");
            internal_error(e).into_response()
        }
    }
}

async fn get_files(pool: &Pool<Sqlite>, query: &FilesQuery) -> Result<Vec<ApiFile>> {
    let limit = query.page_size.or(query.limit).unwrap_or(100);
    let offset = query.offset.unwrap_or(0);

    // Build dynamic SQL with WHERE clauses for filtering
    let mut sql = String::from("SELECT * FROM files WHERE 1=1");

    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        sql.push_str(" AND (title LIKE ? OR artist LIKE ? OR file_path LIKE ? OR genre LIKE ? OR album LIKE ? OR isrc LIKE ? OR comment LIKE ? OR EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND frt.tag_name LIKE ?))");
    }

    if query.bpm_min.is_some() {
        sql.push_str(" AND bpm >= ?");
    }

    if query.bpm_max.is_some() {
        sql.push_str(" AND bpm <= ?");
    }

    if let Some(rating_min) = query.rating_min {
        sql.push_str(" AND rating >= ?");
    }

    if let Some(play_count_min) = query.play_count_min {
        sql.push_str(" AND play_count >= ?");
    }

    if let Some(ref key_str) = query.key {
        let keys: Vec<&str> = key_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !keys.is_empty() {
            let placeholders: Vec<String> = keys.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND musical_key IN ({})", placeholders.join(",")));
        }
    }

    // For linkedOnly (direct service IDs OR ISRC matches any service track)
    if query.linked_only.unwrap_or(false) {
        sql.push_str(" AND EXISTS (SELECT 1 FROM v_file_track_link v WHERE v.file_id = files.id)");
    }

    // For unlinked (no direct service IDs AND no ISRC match)
    if query.unlinked.unwrap_or(false) {
        sql.push_str(
            " AND NOT EXISTS (SELECT 1 FROM v_file_track_link v WHERE v.file_id = files.id)",
        );
    }

    // For nonDefaultOnly (files with at least one tag from a non-default category)
    if query.non_default_only.unwrap_or(false) {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND frt.is_default = FALSE)",
        );
    }

    // Tag filter: files that have any of the selected tags
    // Store tag names separately for binding (Vec<String> instead of &str references).
    let mut tag_param_values: Vec<String> = Vec::new();
    if let Some(ref tags_str) = query.tags
        && !tags_str.is_empty()
    {
        let lowered: Vec<String> = tags_str
            .split(',')
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if !lowered.is_empty() {
            let placeholders: Vec<String> = lowered.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(TRIM(frt.tag_name)) IN ({}))",
                placeholders.join(",")
            ));
            tag_param_values = lowered;
        }
    }

    // Service filter: files linked to a service track with matching service
    if let Some(ref services_str) = query.selected_services {
        let services: Vec<&str> = services_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !services.is_empty() {
            let placeholders: Vec<String> = services.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM v_file_track_link vf JOIN service_tracks st ON st.id = vf.track_id WHERE vf.file_id = files.id AND st.service IN ({}))",
                placeholders.join(",")
            ));
        }
    }

    // PMV filter — check file_resolved_tags.prefix (actual tag categories, not comment string)
    if let Some(ref pmv_cats) = query.pmv_categories {
        let cats: Vec<String> = pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !cats.is_empty() {
            let placeholders: Vec<String> = cats.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) IN ({}))",
                placeholders.join(",")
            ));
        }
    } else if let Some(ref pmv_agg) = query.pmv_aggregate {
        match pmv_agg.as_str() {
            "full" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) = 'p')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) = 'm')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) = 'v')");
            }
            "partial" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) IN ('p','m','v'))");
            }
            "none" => {
                sql.push_str(" AND NOT EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) IN ('p','m','v'))");
            }
            _ => {}
        }
    }

    // File type filter
    if let Some(ref ft_str) = query.file_types {
        let types: Vec<&str> = ft_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !types.is_empty() {
            let placeholders: Vec<String> = types.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND file_type IN ({})", placeholders.join(",")));
        }
    }

    // Backup filter
    if let Some(true) = query.backed_up {
        sql.push_str(" AND EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'backup')");
    } else if let Some(false) = query.backed_up {
        sql.push_str(" AND NOT EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'backup')");
    }

    // Local presence filter
    if let Some(true) = query.is_local {
        sql.push_str(" AND EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'local')");
    } else if let Some(false) = query.is_local {
        sql.push_str(" AND NOT EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'local')");
    }

    if let Some(true) = query.safe_to_delete {
        sql.push_str(" AND EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'backup')");
        sql.push_str(" AND files.file_type != 'stem.m4a'");
        sql.push_str(" AND EXISTS (SELECT 1 FROM files f2 WHERE f2.isrc = files.isrc AND f2.isrc IS NOT NULL AND f2.file_type = 'stem.m4a')");
    }

    apply_sort(
        &mut sql,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[
            "title",
            "artist",
            "bpm",
            "musical_key",
            "rating",
            "isrc",
            "play_count",
            "last_played",
            "created_at",
            "duration_ms",
            "file_type",
        ],
        "id",
    );

    // When comment_statuses filter is active, we must apply it in Rust BEFORE pagination.
    // So we fetch ALL rows without LIMIT/OFFSET, compute needs_update, filter, then slice.
    let has_comment_filter = query.comment_statuses.is_some();
    if !has_comment_filter {
        sql.push_str(" LIMIT ? OFFSET ?");
    }

    // Build query with bind parameters
    let mut q = sqlx::query_as::<_, File>(&sql);

    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        let pattern = format!("%{}%", search);
        // Bind once per LIKE clause (title, artist, file_path, genre, album, isrc, comment, tag name)
        for _ in 0..8 {
            q = q.bind(pattern.clone());
        }
    }

    if let Some(bpm_min) = query.bpm_min {
        q = q.bind(bpm_min);
    }

    if let Some(bpm_max) = query.bpm_max {
        q = q.bind(bpm_max);
    }

    if let Some(rating_min) = query.rating_min {
        q = q.bind(rating_min);
    }

    if let Some(play_count_min) = query.play_count_min {
        q = q.bind(play_count_min);
    }

    if let Some(ref key_str) = query.key {
        for k in key_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(k);
        }
    }

    // Bind params for service filter
    if let Some(ref services_str) = query.selected_services {
        for s in services_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(s);
        }
    }

    // Bind params for tag filter
    for tag in &tag_param_values {
        q = q.bind(tag.as_str());
    }

    // Bind params for PMV filter
    if let Some(ref pmv_cats) = query.pmv_categories {
        for c in pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(c);
        }
    }

    // Bind params for file type filter
    if let Some(ref ft_str) = query.file_types {
        for t in ft_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(t);
        }
    }

    if !has_comment_filter {
        q = q.bind(limit).bind(offset);
    }

    let files: Vec<File>;
    // Cache for pre-computed target comments when comment_statuses is active
    // to avoid re-computing in the downstream loop
    let mut target_comments: HashMap<i64, String> = HashMap::new();
    if has_comment_filter {
        // Fetch ALL matching files (no LIMIT/OFFSET) to apply comment status filter before pagination
        let all_files = q.fetch_all(pool).await?;

        if all_files.is_empty() {
            return Ok(vec![]);
        }

        // Compute comment_needs_update for all files and cache target comments
        let file_ids: Vec<i64> = all_files.iter().map(|f| f.id).collect();
        let batch_targets = match compute_target_comments_batch(pool, &file_ids).await {
            Ok(map) => map,
            Err(e) => {
                tracing::warn!("Failed to compute target comments batch: {}", e);
                HashMap::new()
            }
        };
        target_comments = batch_targets.clone();
        let mut with_status: Vec<(File, bool)> = Vec::with_capacity(all_files.len());
        for file in all_files {
            let needs_update = match batch_targets.get(&file.id) {
                Some(target_comment) => file.comment.as_ref() != Some(target_comment),
                None => false,
            };
            with_status.push((file, needs_update));
        }

        // Filter by comment status
        let statuses: Vec<&str> = query
            .comment_statuses
            .as_ref()
            .unwrap()
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !statuses.is_empty() {
            with_status.retain(|(_, needs_update)| {
                let mut keep = false;
                if statuses.contains(&"needs_update") && *needs_update {
                    keep = true;
                }
                if statuses.contains(&"uptodate") && !*needs_update {
                    keep = true;
                }
                keep
            });
        }

        // Apply paging in Rust
        let start = offset as usize;
        let end = (start + limit as usize).min(with_status.len());
        files = if start < with_status.len() {
            with_status[start..end]
                .iter()
                .map(|(f, _)| f.clone())
                .collect()
        } else {
            vec![]
        };
    } else {
        files = q.fetch_all(pool).await?;
    }

    if files.is_empty() {
        return Ok(vec![]);
    }

    // Get matched services for these files
    let file_ids: Vec<i64> = files.iter().map(|f| f.id).collect();
    let placeholders: Vec<String> = file_ids.iter().map(|_| "?".to_string()).collect();

    let match_sql = format!(
        "SELECT f.id, COALESCE(GROUP_CONCAT(DISTINCT st.service), '') as services
         FROM files f
         LEFT JOIN v_file_track_link v ON v.file_id = f.id
         LEFT JOIN service_tracks st ON st.id = v.track_id
         WHERE f.id IN ({})
         GROUP BY f.id",
        placeholders.join(", ")
    );

    let mut match_query = sqlx::query(&match_sql);
    for id in &file_ids {
        match_query = match_query.bind(id);
    }

    let match_rows = match_query.fetch_all(pool).await?;
    let mut services_map: HashMap<i64, Vec<String>> = HashMap::new();
    for row in match_rows {
        let file_id: i64 = row.try_get("id")?;
        let services_str: String = row.try_get("services")?;
        let services: Vec<String> = if services_str.is_empty() {
            vec![]
        } else {
            services_str.split(',').map(|s| s.to_string()).collect()
        };
        services_map.insert(file_id, services);
    }

    // Convert files to ApiFile with target comment computation
    // Use cached target_comments when pre-computed (comment_statuses filter path)
    let mut api_files = Vec::new();
    for file in files {
        let mut api_file = ApiFile::from(file);

        // Set matched services
        if let Some(services) = services_map.remove(&api_file.id) {
            api_file.matched_services = services;
        }

        // Compute target comment (use cache from comment status filter if available)
        if let Some(cached_target) = target_comments.remove(&api_file.id) {
            api_file.comment_target = cached_target;
            api_file.comment_needs_update =
                api_file.comment.as_ref() != Some(&api_file.comment_target);
        } else {
            match compute_target_comment(pool, api_file.id).await {
                Ok(target_comment) => {
                    api_file.comment_target = target_comment;
                    // Determine if comment needs update
                    api_file.comment_needs_update =
                        api_file.comment.as_ref() != Some(&api_file.comment_target);
                }
                Err(e) => {
                    // Log error but continue - don't fail the entire request
                    tracing::warn!(
                        "Failed to compute target comment for file {}: {}",
                        api_file.id,
                        e
                    );
                    api_file.comment_target = String::new();
                    api_file.comment_needs_update = false;
                }
            }
        }

        api_files.push(api_file);
    }

    // Compute backup fields in batch
    let file_ids: Vec<i64> = api_files.iter().map(|f| f.id).collect();
    if !file_ids.is_empty() {
        let backed_up_ids: std::collections::HashSet<i64> = {
            let placeholders: Vec<String> = file_ids.iter().map(|_| "?".to_string()).collect();
            let sql2 = format!(
                "SELECT DISTINCT file_id FROM file_locations WHERE location_type = 'backup' AND file_id IN ({})",
                placeholders.join(",")
            );
            let mut q = sqlx::query_scalar::<_, i64>(&sql2);
            for id in &file_ids {
                q = q.bind(id);
            }
            q.fetch_all(pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .collect()
        };

        let local_ids: std::collections::HashSet<i64> = {
            let placeholders: Vec<String> = file_ids.iter().map(|_| "?".to_string()).collect();
            let sql_local = format!(
                "SELECT DISTINCT file_id FROM file_locations WHERE location_type = 'local' AND file_id IN ({})",
                placeholders.join(",")
            );
            let mut q = sqlx::query_scalar::<_, i64>(&sql_local);
            for id in &file_ids {
                q = q.bind(id);
            }
            q.fetch_all(pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .collect()
        };

        let has_stem_ids: std::collections::HashSet<i64> = {
            let placeholders: Vec<String> = file_ids.iter().map(|_| "?".to_string()).collect();
            let sql3 = format!(
                "SELECT DISTINCT f.id FROM files f WHERE f.isrc IS NOT NULL AND f.isrc != '' AND f.file_type != 'stem.m4a' \
                 AND EXISTS (SELECT 1 FROM files f2 WHERE f2.isrc = f.isrc AND f2.file_type = 'stem.m4a') \
                 AND f.id IN ({})",
                placeholders.join(",")
            );
            let mut q = sqlx::query_scalar::<_, i64>(&sql3);
            for id in &file_ids {
                q = q.bind(id);
            }
            q.fetch_all(pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .collect()
        };

        for af in &mut api_files {
            af.backed_up = backed_up_ids.contains(&af.id);
            af.is_local = local_ids.contains(&af.id);
            af.has_stem = has_stem_ids.contains(&af.id);
            af.safe_to_delete =
                af.is_local && af.backed_up && (af.has_stem || af.file_type == "wav");
        }
    }

    Ok(api_files)
}

async fn get_files_count(pool: &Pool<Sqlite>, query: &FilesQuery) -> Result<i64> {
    // Build dynamic SQL with the same WHERE clauses as get_files
    let mut sql = String::from("SELECT COUNT(*) as count FROM files WHERE 1=1");

    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        sql.push_str(" AND (title LIKE ? OR artist LIKE ? OR file_path LIKE ? OR genre LIKE ? OR album LIKE ? OR isrc LIKE ? OR comment LIKE ? OR EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND frt.tag_name LIKE ?))");
    }

    if query.bpm_min.is_some() {
        sql.push_str(" AND bpm >= ?");
    }

    if query.bpm_max.is_some() {
        sql.push_str(" AND bpm <= ?");
    }

    if let Some(ref key_str) = query.key {
        let keys: Vec<&str> = key_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !keys.is_empty() {
            let placeholders: Vec<String> = keys.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND musical_key IN ({})", placeholders.join(",")));
        }
    }

    if query.linked_only.unwrap_or(false) {
        sql.push_str(" AND EXISTS (SELECT 1 FROM v_file_track_link v WHERE v.file_id = files.id)");
    }

    if query.unlinked.unwrap_or(false) {
        sql.push_str(
            " AND NOT EXISTS (SELECT 1 FROM v_file_track_link v WHERE v.file_id = files.id)",
        );
    }

    if query.non_default_only.unwrap_or(false) {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND frt.is_default = FALSE)",
        );
    }

    // Tag filter: files that have any of the selected tags
    let mut tag_param_values: Vec<String> = Vec::new();
    if let Some(ref tags_str) = query.tags
        && !tags_str.is_empty()
    {
        let lowered: Vec<String> = tags_str
            .split(',')
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if !lowered.is_empty() {
            let placeholders: Vec<String> = lowered.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(TRIM(frt.tag_name)) IN ({}))",
                placeholders.join(",")
            ));
            tag_param_values = lowered;
        }
    }

    // Service filter
    if let Some(ref services_str) = query.selected_services {
        let services: Vec<&str> = services_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !services.is_empty() {
            let placeholders: Vec<String> = services.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM v_file_track_link vf JOIN service_tracks st ON st.id = vf.track_id WHERE vf.file_id = files.id AND st.service IN ({}))",
                placeholders.join(",")
            ));
        }
    }

    // PMV filter — check file_resolved_tags.prefix (actual tag categories)
    if let Some(ref pmv_cats) = query.pmv_categories {
        let cats: Vec<String> = pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !cats.is_empty() {
            let placeholders: Vec<String> = cats.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) IN ({}))",
                placeholders.join(",")
            ));
        }
    } else if let Some(ref pmv_agg) = query.pmv_aggregate {
        match pmv_agg.as_str() {
            "full" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) = 'p')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) = 'm')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) = 'v')");
            }
            "partial" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) IN ('p','m','v'))");
            }
            "none" => {
                sql.push_str(" AND NOT EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = files.id AND LOWER(frt.prefix) IN ('p','m','v'))");
            }
            _ => {}
        }
    }

    // File type filter
    if let Some(ref ft_str) = query.file_types {
        let types: Vec<&str> = ft_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !types.is_empty() {
            let placeholders: Vec<String> = types.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND file_type IN ({})", placeholders.join(",")));
        }
    }

    // Backup filter
    if let Some(true) = query.backed_up {
        sql.push_str(" AND EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'backup')");
    } else if let Some(false) = query.backed_up {
        sql.push_str(" AND NOT EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'backup')");
    }

    // Local presence filter
    if let Some(true) = query.is_local {
        sql.push_str(" AND EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'local')");
    } else if let Some(false) = query.is_local {
        sql.push_str(" AND NOT EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'local')");
    }

    if let Some(true) = query.safe_to_delete {
        sql.push_str(" AND EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = files.id AND fl.location_type = 'backup')");
        sql.push_str(" AND files.file_type != 'stem.m4a'");
        sql.push_str(" AND EXISTS (SELECT 1 FROM files f2 WHERE f2.isrc = files.isrc AND f2.isrc IS NOT NULL AND f2.file_type = 'stem.m4a')");
    }

    let mut q = sqlx::query(&sql);

    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        let pattern = format!("%{}%", search);
        for _ in 0..8 {
            q = q.bind(pattern.clone());
        }
    }

    if let Some(bpm_min) = query.bpm_min {
        q = q.bind(bpm_min);
    }

    if let Some(bpm_max) = query.bpm_max {
        q = q.bind(bpm_max);
    }

    if let Some(rating_min) = query.rating_min {
        q = q.bind(rating_min);
    }

    if let Some(play_count_min) = query.play_count_min {
        q = q.bind(play_count_min);
    }

    if let Some(ref key_str) = query.key {
        for k in key_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(k);
        }
    }

    // Bind params for service filter
    if let Some(ref services_str) = query.selected_services {
        for s in services_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(s);
        }
    }

    // Bind params for tag filter
    for tag in &tag_param_values {
        q = q.bind(tag.as_str());
    }

    // Bind params for PMV filter
    if let Some(ref pmv_cats) = query.pmv_categories {
        for c in pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(c);
        }
    }

    // Bind params for file type filter
    if let Some(ref ft_str) = query.file_types {
        for t in ft_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(t);
        }
    }

    let row = q.fetch_one(pool).await?;
    let count: i64 = row.try_get("count")?;

    // If comment status filter is active, we need to compute comment_needs_update
    // and filter in Rust for an accurate count
    if let Some(ref cs_str) = query.comment_statuses {
        let statuses: Vec<&str> = cs_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !statuses.is_empty() && count > 0 {
            // Fetch all matching IDs without limit/offset
            let id_sql = sql.replace("SELECT COUNT(*) as count FROM", "SELECT id FROM");
            let mut id_q = sqlx::query_scalar::<_, i64>(&id_sql);
            // Re-bind all params
            let search_pat = query.search.as_ref().and_then(|s| {
                if s.is_empty() {
                    None
                } else {
                    Some(format!("%{}%", s))
                }
            });
            if let Some(ref pat) = search_pat {
                for _ in 0..8 {
                    id_q = id_q.bind(pat.as_str());
                }
            }
            if let Some(bpm_min) = query.bpm_min {
                id_q = id_q.bind(bpm_min);
            }
            if let Some(bpm_max) = query.bpm_max {
                id_q = id_q.bind(bpm_max);
            }
            if let Some(ref key_str) = query.key {
                for k in key_str
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    id_q = id_q.bind(k);
                }
            }
            if let Some(ref services_str) = query.selected_services {
                for s in services_str
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    id_q = id_q.bind(s);
                }
            }
            if let Some(ref ft_str) = query.file_types {
                for t in ft_str
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    id_q = id_q.bind(t);
                }
            }

            // Re-bind tag filter params
            for tag in &tag_param_values {
                id_q = id_q.bind(tag.as_str());
            }

            // Re-bind PMV filter params
            if let Some(ref pmv_cats) = query.pmv_categories {
                for c in pmv_cats
                    .split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                {
                    id_q = id_q.bind(c);
                }
            }

            let ids: Vec<i64> = id_q.fetch_all(pool).await?;

            if ids.is_empty() {
                return Ok(0);
            }

            // Batch compute target comments
            let batch_targets = match compute_target_comments_batch(pool, &ids).await {
                Ok(map) => map,
                Err(e) => {
                    tracing::warn!("Failed to compute target comments batch: {}", e);
                    HashMap::new()
                }
            };

            // Fetch all comments in one query
            let comments: HashMap<i64, String> = {
                let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
                let sql = format!(
                    "SELECT id, COALESCE(comment, '') FROM files WHERE id IN ({})",
                    placeholders.join(",")
                );
                let mut q = sqlx::query(&sql);
                for id in &ids {
                    q = q.bind(id);
                }
                let rows = q.fetch_all(pool).await?;
                rows.iter()
                    .map(|row| {
                        let id: i64 = row.try_get(0).unwrap_or(0);
                        let comment: String = row.try_get(1).unwrap_or_default();
                        (id, comment)
                    })
                    .collect()
            };

            let mut filtered_count: i64 = 0;
            for file_id in ids {
                let current_comment = comments.get(&file_id).map(|s| s.as_str()).unwrap_or("");
                let needs_update = match batch_targets.get(&file_id) {
                    Some(target) => current_comment != target.as_str(),
                    None => false,
                };
                let mut keep = false;
                if statuses.contains(&"needs_update") && needs_update {
                    keep = true;
                }
                if statuses.contains(&"uptodate") && !needs_update {
                    keep = true;
                }
                if keep {
                    filtered_count += 1;
                }
            }
            return Ok(filtered_count);
        }
    }

    Ok(count)
}

async fn get_file_by_id(pool: &Pool<Sqlite>, id: i64) -> Result<ApiFile> {
    let file = sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await?;

    let mut api_file = ApiFile::from(file);

    // Get matched services for this file
    let match_sql = r#"SELECT COALESCE(GROUP_CONCAT(DISTINCT st.service), '') as services
         FROM files f
         LEFT JOIN v_file_track_link v ON v.file_id = f.id
         LEFT JOIN service_tracks st ON st.id = v.track_id
         WHERE f.id = ?"#;

    let services_str: String = sqlx::query_scalar::<Sqlite, String>(match_sql)
        .bind(api_file.id)
        .fetch_one(pool)
        .await?;

    if !services_str.is_empty() {
        api_file.matched_services = services_str.split(',').map(|s| s.to_string()).collect();
    }

    // Compute target comment
    match compute_target_comment(pool, api_file.id).await {
        Ok(target_comment) => {
            api_file.comment_target = target_comment;
            // Determine if comment needs update
            api_file.comment_needs_update =
                api_file.comment.as_ref() != Some(&api_file.comment_target);
        }
        Err(e) => {
            // Log error but continue - don't fail the entire request
            tracing::warn!(
                "Failed to compute target comment for file {}: {}",
                api_file.id,
                e
            );
            api_file.comment_target = String::new();
            api_file.comment_needs_update = false;
        }
    }

    // Compute backup/stem status from file_locations
    api_file.backed_up = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT 1 FROM file_locations WHERE file_id = ? AND location_type = 'backup' LIMIT 1",
    )
    .bind(api_file.id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some();

    api_file.has_stem = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT 1 FROM files f WHERE f.isrc = (SELECT isrc FROM files WHERE id = ?) AND f.isrc IS NOT NULL AND f.isrc != '' AND f.file_type = 'stem.m4a' AND f.id != ? LIMIT 1"
    )
    .bind(api_file.id)
    .bind(api_file.id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some();

    api_file.is_local = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT 1 FROM file_locations WHERE file_id = ? AND location_type = 'local' LIMIT 1",
    )
    .bind(api_file.id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some();

    api_file.safe_to_delete = api_file.is_local
        && api_file.backed_up
        && (api_file.has_stem || api_file.file_type == "wav");

    Ok(api_file)
}

// ── Handler that needs TracksQuery (from tracks.rs) ─────────────────────

async fn find_tag_similar_tracks_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(query): Query<super::tracks::TracksQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(20).min(100);

    match find_tag_similar_tracks(&state.db, id, limit).await {
        Ok(results) => Json(ApiResponse { data: results }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/files/{id}/debug-comment
/// Returns the full comment resolution chain for a file, for debugging.
async fn file_debug_comment_handler(
    Path(id): Path<i64>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let db = &state.db;

    // 1. Fetch the file
    let file = match sqlx::query_as::<_, crate::db::File>("SELECT * FROM files WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
    {
        Ok(Some(f)) => f,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("File with id {} not found", id),
                }),
            )
                .into_response();
        }
        Err(e) => return internal_error(e).into_response(),
    };

    let title = file
        .title
        .clone()
        .unwrap_or_else(|| "(unknown)".to_string());
    let artist = file
        .artist
        .clone()
        .unwrap_or_else(|| "(unknown)".to_string());

    // 2. Get all playlists linked via v_file_track_link
    let playlists = match sqlx::query_as::<_, (String, String)>(
        "SELECT sp.name, sp.service
         FROM service_playlists sp
         JOIN service_playlist_tracks spt ON spt.playlist_id = sp.id
         JOIN v_file_track_link v ON v.track_id = spt.track_id
         WHERE v.file_id = ?",
    )
    .bind(id)
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|(name, service)| DebugPlaylist { name, service })
            .collect::<Vec<_>>(),
        Err(e) => return internal_error(e).into_response(),
    };

    // 3. Get all matched tags (tags matching playlist names)
    let matched_tags = match sqlx::query_as::<_, (i64, String, String, bool)>(
        "SELECT DISTINCT t.id, t.name, tc.name AS category_name,
                EXISTS (SELECT 1 FROM tag_parents tp WHERE tp.tag_id = t.id) AS has_parents
         FROM tags t
         JOIN tag_categories tc ON tc.id = t.category_id
         JOIN service_playlists sp ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
         JOIN service_playlist_tracks spt ON spt.playlist_id = sp.id
         JOIN v_file_track_link v ON v.track_id = spt.track_id
         WHERE v.file_id = ?",
    )
    .bind(id)
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(
                |(tag_id, tag_name, category_name, has_parents)| DebugMatchedTag {
                    tag_id,
                    tag_name,
                    category_name,
                    has_parents,
                },
            )
            .collect::<Vec<_>>(),
        Err(e) => return internal_error(e).into_response(),
    };

    // 4. Get resolved tag rows from file_resolved_tags
    let tag_rows = match sqlx::query_as::<_, (String, String)>(
        "SELECT frt.tag_name, frt.prefix
         FROM file_resolved_tags frt
         WHERE frt.file_id = ?",
    )
    .bind(id)
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|(tag_name, prefix)| DebugTagRow { tag_name, prefix })
            .collect::<Vec<_>>(),
        Err(e) => return internal_error(e).into_response(),
    };

    // 5. Compute PMV presence from tag rows
    let has_phase = tag_rows.iter().any(|r| r.prefix.eq_ignore_ascii_case("p"));
    let has_mood = tag_rows.iter().any(|r| r.prefix.eq_ignore_ascii_case("m"));
    let has_vibe = tag_rows.iter().any(|r| r.prefix.eq_ignore_ascii_case("v"));
    let pmv = DebugPmv {
        phase: has_phase,
        mood: has_mood,
        vibe: has_vibe,
    };

    // 6. Generate the target comment using the same tag rows
    let phase_char = if has_phase { 'P' } else { '-' };
    let mood_char = if has_mood { 'M' } else { '-' };
    let vibe_char = if has_vibe { 'V' } else { '-' };
    let tag_name_refs: Vec<String> = tag_rows.iter().map(|r| r.tag_name.clone()).collect();
    let generated_comment = generate_target_comment(
        phase_char,
        mood_char,
        vibe_char,
        &tag_name_refs,
        file.spotify_id.as_deref(),
        file.soundcloud_id.as_deref(),
        file.youtube_id.as_deref(),
    );

    let response = FileDebugCommentResponse {
        file_id: file.id,
        title,
        artist,
        tag_rows,
        pmv,
        generated_comment,
        current_comment: file.comment,
        playlists,
        matched_tags,
    };

    Json(ApiResponse {
        data: Some(response),
    })
    .into_response()
}

async fn file_stream_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    request: Request<Body>,
) -> Response {
    // 1. Look up file in DB
    let file = match sqlx::query_as::<_, crate::db::File>("SELECT * FROM files WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(f)) => f,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "File not found").into_response();
        }
        Err(e) => {
            tracing::error!("DB error looking up file {}: {}", id, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    // 2. Determine content type from extension
    let file_type_lower = file.file_type.to_lowercase();
    let is_stem_m4a = file_type_lower == "stem.m4a" || file_type_lower == "m4a";
    let content_type = match file_type_lower.as_str() {
        "flac" => "audio/flac",
        "m4a" | "stem.m4a" => "audio/mp4",
        "mp3" | "mpeg" => "audio/mpeg",
        "wav" | "wave" => "audio/wav",
        "aif" | "aiff" => "audio/aiff",
        "ogg" => "audio/ogg",
        "wma" => "audio/x-ms-wma",
        _ => "application/octet-stream",
    };

    // 3. Open file
    let file_path = &file.file_path;
    let metadata = match tokio::fs::metadata(file_path).await {
        Ok(m) => m,
        Err(_) => {
            return (StatusCode::NOT_FOUND, "File not found on disk").into_response();
        }
    };
    let file_size = metadata.len();

    // 4. Parse Range header
    let range_header = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok());

    if let Some(range_str) = range_header {
        // Parse "bytes=start-end"
        if let Some(range_val) = range_str.strip_prefix("bytes=")
            && let Some((start_str, end_str)) = range_val.split_once('-')
        {
            let start: u64 = start_str.parse().unwrap_or(0);
            let end: u64 = end_str.parse().unwrap_or(file_size - 1);
            let end = end.min(file_size - 1);
            let length = end - start + 1;

            // Open file and seek
            let mut file = match TokioFile::open(file_path).await {
                Ok(f) => f,
                Err(_) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "Cannot open file").into_response();
                }
            };

            let mut buf = vec![0u8; length as usize];
            if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
                return (StatusCode::INTERNAL_SERVER_ERROR, "Seek error").into_response();
            }
            if file.read_exact(&mut buf).await.is_err() {
                return (StatusCode::INTERNAL_SERVER_ERROR, "Read error").into_response();
            }

            let content_range = format!("bytes {}-{}/{}", start, end, file_size);
            let headers = [
                (header::CONTENT_TYPE, content_type),
                (header::CONTENT_RANGE, content_range.as_str()),
                (header::CONTENT_LENGTH, &length.to_string()),
                (header::ACCEPT_RANGES, "bytes"),
                (header::CACHE_CONTROL, "no-cache"),
            ];

            return (StatusCode::PARTIAL_CONTENT, headers, buf).into_response();
        }
    }

    // 5. Full-file response (no Range header)
    // For stem.m4a files, intelligently extract the master mix.
    // If the first audio stream is stereo (2ch), use it directly.
    // If it's mono (1ch), it's a stem — mix ALL streams together.
    if is_stem_m4a {
        let stream0_channels: Option<u32> = TokioCommand::new(resolve_tool("ffprobe"))
            .args([
                "-v",
                "error",
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=channels",
                "-of",
                "csv=p=0",
                file_path,
            ])
            .output()
            .await
            .ok()
            .and_then(|out| {
                if out.status.success() {
                    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
                } else {
                    None
                }
            });

        let is_master_at_0 = stream0_channels == Some(2);

        let mut cmd = TokioCommand::new(resolve_tool("ffmpeg"));
        cmd.args(["-i", file_path]);

        if is_master_at_0 {
            // Standard NI Stems: master is stream 0, stereo
            cmd.args(["-map", "0:a:0", "-c:a", "pcm_s16le", "-f", "wav"]);
        } else {
            // Non-standard: stem at stream 0, mix all 5 streams
            cmd.args([
                "-filter_complex",
                "[0:a]amix=inputs=5:duration=longest",
                "-ac",
                "2",
                "-c:a",
                "pcm_s16le",
                "-f",
                "wav",
            ]);
        }
        cmd.arg("pipe:1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        match cmd.output().await {
            Ok(output) if output.status.success() => {
                let headers = [
                    (header::CONTENT_TYPE, "audio/wav"),
                    (header::CONTENT_LENGTH, &output.stdout.len().to_string()),
                    (header::ACCEPT_RANGES, "none"),
                    (header::CACHE_CONTROL, "no-cache"),
                ];
                return (StatusCode::OK, headers, output.stdout).into_response();
            }
            _ => {
                tracing::warn!(
                    "ffmpeg failed for stem.m4a file {} (id={}), serving raw",
                    file_path,
                    id
                );
                // Fall through to raw file serving below
            }
        }
    }

    // Serve raw file
    let mut file = match TokioFile::open(file_path).await {
        Ok(f) => f,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Cannot open file").into_response();
        }
    };

    let mut buf = Vec::with_capacity(file_size as usize);
    if file.read_to_end(&mut buf).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Read error").into_response();
    }

    let headers = [
        (header::CONTENT_TYPE, content_type),
        (header::CONTENT_LENGTH, &file_size.to_string()),
        (header::ACCEPT_RANGES, "bytes"),
        (header::CACHE_CONTROL, "no-cache"),
    ];

    (StatusCode::OK, headers, buf).into_response()
}

// ── Stage for Conversion ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StageForConversionResponse {
    staged: usize,
    directory: String,
}

/// POST /api/files/stage-for-conversion
/// Clears ~/Music/convert_to_stem/ and creates hardlinks for all files
/// matching the given filter criteria. Used by the "Stage for Conversion"
/// button on the Files page.
async fn stage_for_conversion_handler(
    State(state): State<Arc<AppState>>,
    Json(filter): Json<FilesFilterAll>,
) -> impl IntoResponse {
    let target_dir = std::path::PathBuf::from(
        std::env::var("HOME").unwrap_or_else(|_| "/Users/momo".to_string()),
    )
    .join("Music/convert_to_stem");

    // Clear existing files in target directory
    if target_dir.exists() {
        let mut entries = match tokio::fs::read_dir(&target_dir).await {
            Ok(e) => e,
            Err(e) => {
                return internal_error(format!("Failed to read target directory: {e}"))
                    .into_response();
            }
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("flac") {
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
    } else {
        if let Err(e) = tokio::fs::create_dir_all(&target_dir).await {
            return internal_error(format!("Failed to create target directory: {e}"))
                .into_response();
        }
    }

    // Build and execute the filtered query
    let sql = build_files_filter_sql(&filter)
        .replace("SELECT * FROM files", "SELECT file_path FROM files");
    let mut q = sqlx::query_scalar::<_, String>(&sql);

    if let Some(ref search) = filter.search
        && !search.is_empty()
    {
        let pattern = format!("%{}%", search);
        for _ in 0..8 {
            q = q.bind(pattern.clone());
        }
    }

    if let Some(bpm_min) = filter.bpm_min {
        q = q.bind(bpm_min);
    }
    if let Some(bpm_max) = filter.bpm_max {
        q = q.bind(bpm_max);
    }

    if let Some(ref key_str) = filter.key {
        for k in key_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(k);
        }
    }

    if let Some(ref services_str) = filter.selected_services {
        for s in services_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(s);
        }
    }

    if let Some(ref ft_str) = filter.file_types {
        for t in ft_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(t);
        }
    }

    if let Some(ref tags_str) = filter.tags
        && !tags_str.is_empty()
    {
        for t in tags_str
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(t);
        }
    }

    if let Some(ref pmv_cats) = filter.pmv_categories {
        for c in pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            q = q.bind(c);
        }
    }

    let file_paths: Vec<String> = match q.fetch_all(&state.db).await {
        Ok(p) => p,
        Err(e) => {
            return internal_error(format!("Failed to query files: {e}")).into_response();
        }
    };

    // Create hardlinks
    let mut staged = 0usize;
    for src_path in &file_paths {
        let src = std::path::Path::new(src_path);
        let basename = src.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if basename.is_empty() {
            continue;
        }
        let dst = target_dir.join(basename);
        match std::fs::hard_link(src, &dst) {
            Ok(()) => staged += 1,
            Err(e) => {
                tracing::warn!("Failed to hardlink {} → {}: {e}", src_path, dst.display());
            }
        }
    }

    tracing::info!(
        "Staged {staged} files for conversion in {}",
        target_dir.display()
    );

    Json(ApiResponse {
        data: StageForConversionResponse {
            staged,
            directory: target_dir.to_string_lossy().to_string(),
        },
    })
    .into_response()
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/files", get(files_handler))
        .route("/api/files/count", get(files_count_handler))
        .route("/api/files/latest", get(files_latest_handler))
        .route(
            "/api/files/needs-update-count",
            get(files_needs_update_count_handler),
        )
        .route("/api/files/service-links", get(files_service_links_handler))
        .route("/api/files/{id}", get(file_handler))
        .route("/api/files/{id}/detail", get(file_detail_handler))
        .route("/api/files/{id}/variants", get(file_variants_handler))
        .route("/api/files/{id}/sync-comment", post(sync_comment_handler))
        .route("/api/files/{id}/write-comment", post(sync_comment_handler))
        .route(
            "/api/files/{id}/similar-tracks",
            get(find_tag_similar_tracks_handler),
        )
        .route(
            "/api/files/{id}/debug-comment",
            get(file_debug_comment_handler),
        )
        .route("/api/files/{id}/stream", get(file_stream_handler))
        .route("/api/files/bulk-sync", post(bulk_sync_handler))
        .route("/api/files/write-comments", post(bulk_sync_handler))
        .route(
            "/api/files/needs-comment-count",
            post(files_needs_comment_count_by_ids_handler),
        )
        .route(
            "/api/files/write-comments-by-ids",
            post(files_write_comments_by_ids_handler),
        )
        .route(
            "/api/files/needs-comment-count-all",
            post(files_needs_comment_count_all_handler),
        )
        .route(
            "/api/files/write-comments-all",
            post(files_write_comments_all_handler),
        )
        .route("/api/files/key-comparison", get(key_comparison_handler))
        .route(
            "/api/files/stage-for-conversion",
            post(stage_for_conversion_handler),
        )
}
