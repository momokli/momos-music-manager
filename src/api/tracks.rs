// src/api/tracks.rs
//
// Track-related handlers and types extracted from src/api/legacy.rs.

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, apply_sort, internal_error};
use crate::db::{
    File, ServicePlaylist, ServiceTrack, compute_target_comment, ensure_backpack_tag,
    get_track_detail, read_comment_from_file, update_file_comment,
};

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TracksQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub service: Option<String>,
    pub services: Option<String>,
    pub file_types: Option<String>,
    pub file_type_agg: Option<String>,
    pub search: Option<String>,
    pub playlist_id: Option<i64>,
    pub playlists: Option<String>,
    pub tags: Option<String>,
    pub pmv_categories: Option<String>,
    pub pmv_aggregate: Option<String>,
    pub imported_after_days: Option<i64>,
    pub imported_before_days: Option<i64>,
    pub added_after_days: Option<i64>,
    pub added_before_days: Option<i64>,
    pub has_local: Option<bool>,
    pub has_backup: Option<bool>,
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub keys: Option<String>,
    pub rating_min: Option<i32>,
    pub play_count_min: Option<i32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistTagInfo {
    pub playlist_name: String,
    pub tag_name: String,
    pub category: String,
    pub prefix: String,
    pub icon: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackFormatInfo {
    pub file_type: String,
    pub local: bool,
    pub backup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiServiceTrack {
    pub id: i64,
    pub service: String,
    pub service_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub isrc: Option<String>,
    pub duration_ms: Option<i64>,
    pub metadata_json: Option<String>,
    pub imported_at: i64,
    pub updated_at: i64,
    /// Latest `added_at` across all playlist memberships (MAX of service_playlist_tracks.added_at).
    /// Unix timestamp, None if the track appears in no playlists.
    #[serde(default)]
    pub max_added_at: Option<i64>,
    #[serde(default)]
    pub local_files: Vec<String>,
    #[serde(default)]
    pub playlist_names: Vec<String>,
    #[serde(default)]
    pub playlist_tags: Vec<PlaylistTagInfo>,
    #[serde(default)]
    pub format_info: Vec<TrackFormatInfo>,
    #[serde(default)]
    pub in_backpack: bool,
    // ── File metrics from linked files (aggregated) ──
    #[serde(default)]
    pub bpm: Option<f64>,
    /// Display string for BPM — shows all distinct values, e.g. "159.0 / 160"
    #[serde(default)]
    pub bpm_display: Option<String>,
    #[serde(default)]
    pub musical_key: Option<String>,
    #[serde(default)]
    pub rating: Option<i32>,
    #[serde(default)]
    pub play_count: Option<i32>,
    #[serde(default)]
    pub last_played: Option<i64>,
}

impl From<ServiceTrack> for ApiServiceTrack {
    fn from(track: ServiceTrack) -> Self {
        ApiServiceTrack {
            id: track.id,
            service: track.service,
            service_id: track.service_id,
            title: track.title,
            artist: track.artist,
            album: track.album,
            isrc: track.isrc,
            duration_ms: track.duration_ms,
            metadata_json: track.metadata_json,
            imported_at: track.imported_at,
            updated_at: track.updated_at,
            max_added_at: None,
            local_files: vec![],
            playlist_names: vec![],
            playlist_tags: vec![],
            format_info: vec![],
            in_backpack: false,
            bpm: None,
            bpm_display: None,
            musical_key: None,
            rating: None,
            play_count: None,
            last_played: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TracksBulkRequest {
    pub track_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TracksNeedsCommentCountResponse {
    pub total_tracks: usize,
    pub tracks_needing_update: usize,
    pub files_needing_update: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TracksWriteCommentsResponse {
    pub task_id: String,
    pub file_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TracksNeedsRefreshCountResponse {
    pub total_tracks: usize,
    pub tracks_needing_refresh: usize,
    pub files_total: usize,
    pub files_needing_refresh: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TracksRefreshCommentsResponse {
    pub refreshed_count: usize,
    pub file_count: usize,
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// GET /api/tracks
async fn tracks_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TracksQuery>,
) -> impl IntoResponse {
    match get_tracks(&state.db, &query).await {
        Ok(tracks) => Json(ApiResponse { data: tracks }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/tracks/count
async fn tracks_count_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TracksQuery>,
) -> impl IntoResponse {
    match get_tracks_count(&state.db, &query).await {
        Ok(count) => Json(ApiResponse { data: count }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

/// GET /api/tracks/{id}
async fn track_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_track_by_id(&state.db, id).await {
        Ok(track) => Json(ApiResponse { data: track }).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

/// GET /api/tracks/{id}/detail
async fn track_detail_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_track_detail(&state.db, id).await {
        Ok(Some(detail)) => Json(ApiResponse { data: detail }).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Track not found".to_string(),
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

/// POST /api/tracks/needs-comment-count
/// Takes a list of track IDs, finds linked files, and counts how many tracks
/// have at least one linked file whose comment needs updating.
async fn tracks_needs_comment_count_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TracksBulkRequest>,
) -> impl IntoResponse {
    if body.track_ids.is_empty() {
        return Json(ApiResponse {
            data: TracksNeedsCommentCountResponse {
                total_tracks: 0,
                tracks_needing_update: 0,
                files_needing_update: 0,
            },
        })
        .into_response();
    }

    // Find linked files for the requested track IDs
    let placeholders: Vec<String> = body.track_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT DISTINCT v.file_id, v.track_id FROM v_file_track_link v WHERE v.track_id IN ({})",
        placeholders.join(",")
    );

    let mut query = sqlx::query(&sql);
    for id in &body.track_ids {
        query = query.bind(id);
    }

    let rows = match query.fetch_all(&state.db).await {
        Ok(r) => r,
        Err(e) => {
            return internal_error(format!("Failed to find linked files: {}", e)).into_response();
        }
    };

    if rows.is_empty() {
        return Json(ApiResponse {
            data: TracksNeedsCommentCountResponse {
                total_tracks: body.track_ids.len(),
                tracks_needing_update: 0,
                files_needing_update: 0,
            },
        })
        .into_response();
    }

    // Collect unique file IDs and track→file mapping
    use std::collections::{HashMap, HashSet};
    let mut file_ids_set: HashSet<i64> = HashSet::new();
    let mut track_file_map: HashMap<i64, Vec<i64>> = HashMap::new();
    for row in &rows {
        let file_id: i64 = row.try_get("file_id").unwrap_or(0);
        let track_id: i64 = row.try_get("track_id").unwrap_or(0);
        if file_id > 0 && track_id > 0 {
            file_ids_set.insert(file_id);
            track_file_map.entry(track_id).or_default().push(file_id);
        }
    }

    let file_ids: Vec<i64> = file_ids_set.into_iter().collect();

    // Fetch actual file records to get their current comments
    let file_placeholders: Vec<String> = file_ids.iter().map(|_| "?".to_string()).collect();
    let file_sql = format!(
        "SELECT * FROM files WHERE id IN ({})",
        file_placeholders.join(",")
    );
    let mut file_query = sqlx::query_as::<_, File>(&file_sql);
    for id in &file_ids {
        file_query = file_query.bind(id);
    }
    let files = match file_query.fetch_all(&state.db).await {
        Ok(f) => f,
        Err(e) => {
            return internal_error(format!("Failed to fetch files: {}", e)).into_response();
        }
    };

    // Build a map of file_id → File for quick lookup
    let file_map: HashMap<i64, &File> = files.iter().map(|f| (f.id, f)).collect();

    // Check each track: does it have at least one linked file needing an update?
    let mut tracks_needing_update = 0usize;
    let mut files_needing_update = 0usize;
    let mut checked_files: HashSet<i64> = HashSet::new();

    for track_id in &body.track_ids {
        if let Some(linked_files) = track_file_map.get(track_id) {
            let mut track_needs = false;
            for file_id in linked_files {
                if checked_files.contains(file_id) {
                    // Already counted this file; but we still need to know if it needs update
                    // Re-check from the map
                    if let Some(file) = file_map.get(file_id) {
                        let current = file.comment.as_deref().unwrap_or("");
                        if let Ok(target) = compute_target_comment(&state.db, *file_id).await
                            && current != target
                        {
                            track_needs = true;
                            // files_needing_update is deduped by checked_files below
                        }
                    }
                    continue;
                }
                checked_files.insert(*file_id);
                if let Some(file) = file_map.get(file_id) {
                    let current = file.comment.as_deref().unwrap_or("");
                    if let Ok(target) = compute_target_comment(&state.db, *file_id).await
                        && current != target
                    {
                        files_needing_update += 1;
                        track_needs = true;
                    }
                }
            }
            if track_needs {
                tracks_needing_update += 1;
            }
        }
    }

    Json(ApiResponse {
        data: TracksNeedsCommentCountResponse {
            total_tracks: body.track_ids.len(),
            tracks_needing_update,
            files_needing_update,
        },
    })
    .into_response()
}

/// POST /api/tracks/write-comments
/// Takes a list of track IDs, finds linked files whose comments need updating,
/// and queues a write-comment task for those files.
async fn tracks_write_comments_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TracksBulkRequest>,
) -> impl IntoResponse {
    if body.track_ids.is_empty() {
        return Json(ApiResponse {
            data: TracksWriteCommentsResponse {
                task_id: String::new(),
                file_count: 0,
            },
        })
        .into_response();
    }

    // Find linked files for the requested track IDs
    let placeholders: Vec<String> = body.track_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT DISTINCT v.file_id FROM v_file_track_link v WHERE v.track_id IN ({})",
        placeholders.join(",")
    );

    let mut query = sqlx::query(&sql);
    for id in &body.track_ids {
        query = query.bind(id);
    }

    let rows = match query.fetch_all(&state.db).await {
        Ok(r) => r,
        Err(e) => {
            return internal_error(format!("Failed to find linked files: {}", e)).into_response();
        }
    };

    let mut file_ids: Vec<i64> = Vec::new();
    for row in &rows {
        if let Ok(file_id) = row.try_get::<i64, _>("file_id") {
            file_ids.push(file_id);
        }
    }

    if file_ids.is_empty() {
        return Json(ApiResponse {
            data: TracksWriteCommentsResponse {
                task_id: String::new(),
                file_count: 0,
            },
        })
        .into_response();
    }

    // Filter to only files that actually need an update
    let mut needs_update = Vec::new();
    for file_id in &file_ids {
        match compute_target_comment(&state.db, *file_id).await {
            Ok(target) => {
                // Get the current comment
                let file_result = sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ?")
                    .bind(file_id)
                    .fetch_one(&state.db)
                    .await;
                if let Ok(file) = file_result {
                    if file.comment.as_deref() != Some(&target) {
                        needs_update.push(*file_id);
                    }
                } else {
                    // If we can't read the file, include it anyway
                    needs_update.push(*file_id);
                }
            }
            Err(_) => {
                // If we can't compute the target, include it anyway
                needs_update.push(*file_id);
            }
        }
    }

    if needs_update.is_empty() {
        return Json(ApiResponse {
            data: TracksWriteCommentsResponse {
                task_id: String::new(),
                file_count: 0,
            },
        })
        .into_response();
    }

    let file_count = needs_update.len();
    let task_id =
        crate::tasks::start_write_comment_task(&state.task_manager, &state.db, needs_update).await;

    Json(ApiResponse {
        data: TracksWriteCommentsResponse {
            task_id,
            file_count,
        },
    })
    .into_response()
}

/// POST /api/tracks/needs-refresh-count
/// Takes a list of track IDs, finds linked files, reads the actual comment
/// from each file on disk via exiftool, and counts how many are stale.
async fn tracks_needs_refresh_count_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TracksBulkRequest>,
) -> impl IntoResponse {
    let empty = TracksNeedsRefreshCountResponse {
        total_tracks: body.track_ids.len(),
        tracks_needing_refresh: 0,
        files_total: 0,
        files_needing_refresh: 0,
    };

    if body.track_ids.is_empty() {
        return Json(ApiResponse { data: empty }).into_response();
    }

    // Find linked files for the requested track IDs
    let placeholders: Vec<String> = body.track_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT DISTINCT v.file_id, v.track_id, f.file_path, f.comment
         FROM v_file_track_link v
         JOIN files f ON f.id = v.file_id
         WHERE v.track_id IN ({})",
        placeholders.join(",")
    );

    let mut query = sqlx::query(&sql);
    for id in &body.track_ids {
        query = query.bind(id);
    }

    let rows = match query.fetch_all(&state.db).await {
        Ok(r) => r,
        Err(e) => {
            return internal_error(format!("Failed to find linked files: {}", e)).into_response();
        }
    };

    if rows.is_empty() {
        return Json(ApiResponse { data: empty }).into_response();
    }

    use std::collections::HashSet;
    let mut tracks_with_stale: HashSet<i64> = HashSet::new();
    let mut files_checked: HashSet<i64> = HashSet::new();
    let mut files_stale = 0usize;
    let mut files_total = 0usize;

    for row in &rows {
        let file_id: i64 = row.try_get("file_id").unwrap_or(0);
        let track_id: i64 = row.try_get("track_id").unwrap_or(0);
        let file_path: String = row.try_get("file_path").unwrap_or_default();
        let db_comment: Option<String> = row.try_get("comment").ok();

        if file_id == 0 || file_path.is_empty() {
            continue;
        }

        if files_checked.contains(&file_id) {
            continue;
        }
        files_checked.insert(file_id);
        files_total += 1;

        // Read actual comment from the file on disk
        match read_comment_from_file(&file_path).await {
            Ok(disk_comment) => {
                let disk_str = disk_comment.as_deref().unwrap_or("");
                let db_str = db_comment.as_deref().unwrap_or("");
                if disk_str != db_str {
                    files_stale += 1;
                    tracks_with_stale.insert(track_id);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to read comment from '{}': {}", file_path, e);
            }
        }
    }

    Json(ApiResponse {
        data: TracksNeedsRefreshCountResponse {
            total_tracks: body.track_ids.len(),
            tracks_needing_refresh: tracks_with_stale.len(),
            files_total,
            files_needing_refresh: files_stale,
        },
    })
    .into_response()
}

/// POST /api/tracks/refresh-comments
/// Takes a list of track IDs, finds linked files, reads the actual comment
/// from each file on disk via exiftool, and updates the DB if different.
async fn tracks_refresh_comments_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TracksBulkRequest>,
) -> impl IntoResponse {
    if body.track_ids.is_empty() {
        return Json(ApiResponse {
            data: TracksRefreshCommentsResponse {
                refreshed_count: 0,
                file_count: 0,
            },
        })
        .into_response();
    }

    // Find linked files for the requested track IDs
    let placeholders: Vec<String> = body.track_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT DISTINCT v.file_id, f.file_path, f.comment
         FROM v_file_track_link v
         JOIN files f ON f.id = v.file_id
         WHERE v.track_id IN ({})",
        placeholders.join(",")
    );

    let mut query = sqlx::query(&sql);
    for id in &body.track_ids {
        query = query.bind(id);
    }

    let rows = match query.fetch_all(&state.db).await {
        Ok(r) => r,
        Err(e) => {
            return internal_error(format!("Failed to find linked files: {}", e)).into_response();
        }
    };

    if rows.is_empty() {
        return Json(ApiResponse {
            data: TracksRefreshCommentsResponse {
                refreshed_count: 0,
                file_count: 0,
            },
        })
        .into_response();
    }

    use std::collections::HashSet;
    let mut refreshed = 0usize;
    let mut seen: HashSet<i64> = HashSet::new();

    for row in &rows {
        let file_id: i64 = row.try_get("file_id").unwrap_or(0);
        let file_path: String = row.try_get("file_path").unwrap_or_default();
        let db_comment: Option<String> = row.try_get("comment").ok();

        if file_id == 0 || file_path.is_empty() || seen.contains(&file_id) {
            continue;
        }
        seen.insert(file_id);

        // Read actual comment from the file on disk
        match read_comment_from_file(&file_path).await {
            Ok(disk_comment) => {
                let disk_str = disk_comment.as_deref().unwrap_or("");
                let db_str = db_comment.as_deref().unwrap_or("");
                if disk_str != db_str {
                    if let Err(e) = update_file_comment(&state.db, file_id, disk_str).await {
                        tracing::warn!("Failed to update DB comment for file #{}: {}", file_id, e);
                    } else {
                        refreshed += 1;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to read comment from '{}': {}", file_path, e);
            }
        }
    }

    Json(ApiResponse {
        data: TracksRefreshCommentsResponse {
            refreshed_count: refreshed,
            file_count: seen.len(),
        },
    })
    .into_response()
}

/// POST /api/tracks/{id}/backpack
/// Toggles the "backpack" tag on/off for a given track by adding/removing it
/// from the "backpack" local playlist.
async fn track_backpack_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Ensure backpack tag exists
    let _backpack_tag = match ensure_backpack_tag(&state.db).await {
        Ok(t) => t,
        Err(e) => return internal_error(e).into_response(),
    };

    // Find playlists matching the backpack tag name (case-insensitive in v_tag_playlist)
    let playlist = sqlx::query_as::<_, ServicePlaylist>(
        "SELECT s.* FROM service_playlists s WHERE LOWER(TRIM(s.name)) = LOWER(TRIM('backpack')) LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await;

    let playlist_id = match playlist {
        Ok(Some(p)) => p.id,
        Ok(None) => {
            // Create a local playlist for backpack
            let now = chrono::Utc::now().timestamp();
            let result = sqlx::query(
                "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at) VALUES ('local', 'backpack', 'backpack', ?, ?)",
            )
            .bind(now)
            .bind(now)
            .execute(&state.db)
            .await;

            match result {
                Ok(r) => r.last_insert_rowid(),
                Err(e) => return internal_error(e).into_response(),
            }
        }
        Err(e) => return internal_error(e).into_response(),
    };

    // Check if track is already in backpack playlist
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM service_playlist_tracks WHERE playlist_id = ? AND track_id = ?",
    )
    .bind(playlist_id)
    .bind(id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let in_backpack = existing > 0;

    if in_backpack {
        // Remove from backpack
        let _ = sqlx::query(
            "DELETE FROM service_playlist_tracks WHERE playlist_id = ? AND track_id = ?",
        )
        .bind(playlist_id)
        .bind(id)
        .execute(&state.db)
        .await;
    } else {
        // Add to backpack
        let now = chrono::Utc::now().timestamp();
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, added_at) VALUES (?, ?, ?)",
        )
        .bind(playlist_id)
        .bind(id)
        .bind(now)
        .execute(&state.db)
        .await;
    }

    Json(ApiResponse {
        data: serde_json::json!({ "inBackpack": !in_backpack }),
    })
    .into_response()
}

// ── Private Helpers ──────────────────────────────────────────────────────

async fn get_tracks(pool: &Pool<Sqlite>, query: &TracksQuery) -> Result<Vec<ApiServiceTrack>> {
    let limit = query.page_size.or(query.limit).unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    let service_filter = query.service.clone();
    let services_filter = query.services.clone();
    let file_types_filter = query.file_types.clone();
    let file_type_agg_filter = query.file_type_agg.clone();
    let search_pattern = query.search.as_ref().and_then(|s| {
        if s.is_empty() {
            None
        } else {
            Some(format!("%{}%", s))
        }
    });
    let playlist_id_filter = query.playlist_id;
    let playlists_filter = query.playlists.clone();
    let tags_filter = query.tags.clone();
    let pmv_categories_filter = query.pmv_categories.clone();
    let pmv_aggregate_filter = query.pmv_aggregate.clone();
    let imported_after_days_filter = query.imported_after_days;
    let imported_before_days_filter = query.imported_before_days;
    let added_after_days_filter = query.added_after_days;
    let added_before_days_filter = query.added_before_days;

    // If filtering by playlist (multi-name takes precedence over single ID),
    // use DISTINCT to avoid duplicates from the JOIN
    // Pre-compute track IDs for has_local/has_backup filters (one query, not per-row EXISTS)
    let backup_track_ids: std::collections::HashSet<i64> = if query.has_backup == Some(true)
        || query.has_local == Some(true)
    {
        sqlx::query_scalar(
            "SELECT DISTINCT vft.track_id FROM v_file_track_link vft JOIN file_locations fl ON fl.file_id = vft.file_id AND fl.location_type = 'backup'"
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
    } else {
        Default::default()
    };

    let local_track_ids: std::collections::HashSet<i64> = if query.has_local == Some(true) {
        sqlx::query_scalar("SELECT DISTINCT vft.track_id FROM v_file_track_link vft JOIN file_locations fl ON fl.file_id = vft.file_id AND fl.location_type = 'local'")
            .fetch_all(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect()
    } else {
        Default::default()
    };

    let mut sql = if playlists_filter.is_some() {
        "SELECT DISTINCT st.* FROM service_tracks st JOIN service_playlist_tracks spt ON spt.track_id = st.id JOIN service_playlists sp ON sp.id = spt.playlist_id WHERE 1=1".to_string()
    } else if playlist_id_filter.is_some() {
        "SELECT DISTINCT st.* FROM service_tracks st JOIN service_playlist_tracks spt ON spt.track_id = st.id WHERE 1=1"
            .to_string()
    } else {
        "SELECT * FROM service_tracks st WHERE 1=1".to_string()
    };

    if search_pattern.is_some() {
        sql.push_str(" AND (st.title LIKE ? OR st.artist LIKE ? OR st.album LIKE ? OR st.isrc LIKE ? OR EXISTS (SELECT 1 FROM service_playlist_tracks spt2 JOIN service_playlists sp2 ON sp2.id = spt2.playlist_id WHERE spt2.track_id = st.id AND sp2.name LIKE ?))");
    }

    if service_filter.is_some() {
        sql.push_str(" AND st.service = ?");
    }

    if let Some(ref svcs) = services_filter {
        let svc_list: Vec<&str> = svcs
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !svc_list.is_empty() {
            let placeholders: Vec<String> = svc_list.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND st.service IN ({})", placeholders.join(",")));
        }
    }

    if let Some(ref ft_agg) = file_type_agg_filter {
        match ft_agg.as_str() {
            "any" => {
                sql.push_str(
                    " AND EXISTS (SELECT 1 FROM v_file_track_link vft WHERE vft.track_id = st.id)",
                );
            }
            "none" => {
                sql.push_str(" AND NOT EXISTS (SELECT 1 FROM v_file_track_link vft WHERE vft.track_id = st.id)");
            }
            _ => {}
        }
    }

    if let Some(ref ft_types) = file_types_filter {
        let type_list: Vec<&str> = ft_types
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !type_list.is_empty() {
            let placeholders: Vec<String> = type_list.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM v_file_track_link vft2 JOIN files f2 ON f2.id = vft2.file_id WHERE vft2.track_id = st.id AND f2.file_type IN ({}))",
                placeholders.join(",")
            ));
        }
    }

    if playlist_id_filter.is_some() && playlists_filter.is_none() {
        sql.push_str(" AND spt.playlist_id = ?");
    }

    // Playlists filter (multi-name, OR logic)
    if let Some(ref pl_names) = playlists_filter {
        let pl_list: Vec<&str> = pl_names
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !pl_list.is_empty() {
            let lowered: Vec<String> = pl_list.iter().map(|_| "LOWER(?)".to_string()).collect();
            sql.push_str(&format!(" AND LOWER(sp.name) IN ({})", lowered.join(",")));
        }
    }

    // Tags filter
    if let Some(ref tags_str) = tags_filter {
        let tag_list: Vec<&str> = tags_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !tag_list.is_empty() {
            let placeholders: Vec<String> = tag_list.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND trt.tag_name IN ({}))", placeholders.join(",")));
        }
    }

    // PMV filter — categories and aggregate are mutually exclusive
    if let Some(ref pmv_cats) = pmv_categories_filter {
        let cat_list: Vec<String> = pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !cat_list.is_empty() {
            let placeholders: Vec<String> = cat_list.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) IN ({}))", placeholders.join(",")));
        }
    } else if let Some(ref pmv_agg) = pmv_aggregate_filter {
        match pmv_agg.as_str() {
            "full" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) = 'p')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) = 'm')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) = 'v')");
            }
            "partial" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) IN ('p','m','v'))");
            }
            "none" => {
                sql.push_str(" AND NOT EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) IN ('p','m','v'))");
            }
            _ => {}
        }
    }

    // Date filters
    if imported_after_days_filter.is_some() {
        sql.push_str(" AND st.imported_at >= unixepoch('now', ?)");
    }
    if imported_before_days_filter.is_some() {
        sql.push_str(" AND st.imported_at <= unixepoch('now', ?)");
    }
    if added_after_days_filter.is_some() {
        sql.push_str(" AND (SELECT MAX(spt4.added_at) FROM service_playlist_tracks spt4 WHERE spt4.track_id = st.id) >= unixepoch('now', ?)");
    }
    if added_before_days_filter.is_some() {
        sql.push_str(" AND (SELECT MAX(spt4.added_at) FROM service_playlist_tracks spt4 WHERE spt4.track_id = st.id) <= unixepoch('now', ?)");
    }

    // ── File metrics filters ──
    let bpm_min_filter = query.bpm_min;
    let bpm_max_filter = query.bpm_max;
    let keys_filter = query.keys.clone();
    let rating_min_filter = query.rating_min;
    let play_count_min_filter = query.play_count_min;
    if bpm_min_filter.is_some()
        || bpm_max_filter.is_some()
        || keys_filter.is_some()
        || rating_min_filter.is_some()
        || play_count_min_filter.is_some()
    {
        if bpm_min_filter.is_some() {
            sql.push_str(" AND EXISTS (SELECT 1 FROM v_file_track_link vft_bpm JOIN files f_bpm ON f_bpm.id = vft_bpm.file_id WHERE vft_bpm.track_id = st.id AND f_bpm.bpm >= ?)");
        }
        if bpm_max_filter.is_some() {
            sql.push_str(" AND EXISTS (SELECT 1 FROM v_file_track_link vft_bpm2 JOIN files f_bpm2 ON f_bpm2.id = vft_bpm2.file_id WHERE vft_bpm2.track_id = st.id AND f_bpm2.bpm <= ?)");
        }
        if let Some(ref keys_str) = keys_filter {
            let key_list: Vec<&str> = keys_str
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if !key_list.is_empty() {
                let kh: Vec<String> = key_list.iter().map(|_| "?".to_string()).collect();
                sql.push_str(&format!(" AND EXISTS (SELECT 1 FROM v_file_track_link vft_k JOIN files f_k ON f_k.id = vft_k.file_id WHERE vft_k.track_id = st.id AND f_k.musical_key IN ({}))", kh.join(",")));
            }
        }
        if rating_min_filter.is_some() {
            sql.push_str(" AND EXISTS (SELECT 1 FROM v_file_track_link vft_r JOIN files f_r ON f_r.id = vft_r.file_id WHERE vft_r.track_id = st.id AND f_r.rating >= ?)");
        }
        if play_count_min_filter.is_some() {
            sql.push_str(" AND EXISTS (SELECT 1 FROM v_file_track_link vft_p JOIN files f_p ON f_p.id = vft_p.file_id WHERE vft_p.track_id = st.id AND f_p.play_count >= ?)");
        }
    }

    // Format filter (hasLocal / hasBackup)
    if let Some(true) = query.has_local {
        if !local_track_ids.is_empty() {
            let ids: Vec<String> = local_track_ids.iter().map(|id| id.to_string()).collect();
            sql.push_str(&format!(" AND st.id IN ({})", ids.join(",")));
        } else {
            sql.push_str(" AND 1=0");
        }
    }
    if let Some(true) = query.has_backup {
        if !backup_track_ids.is_empty() {
            let ids: Vec<String> = backup_track_ids.iter().map(|id| id.to_string()).collect();
            sql.push_str(&format!(" AND st.id IN ({})", ids.join(",")));
        } else {
            // No tracks have backup files, force empty result
            sql.push_str(" AND 1=0");
        }
    }

    apply_sort(
        &mut sql,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[
            "title",
            "artist",
            "service",
            "album",
            "duration_ms",
            "isrc",
            "imported_at",
            "max_added_at",
        ],
        "id",
    );
    sql.push_str(" LIMIT ? OFFSET ?");

    let mut query_builder = sqlx::query_as::<_, ServiceTrack>(&sql);

    if let Some(ref pattern) = search_pattern {
        query_builder = query_builder
            .bind(pattern)
            .bind(pattern)
            .bind(pattern)
            .bind(pattern)
            .bind(pattern);
    }

    if let Some(service) = &service_filter {
        query_builder = query_builder.bind(service);
    }

    if let Some(ref svcs) = services_filter {
        for s in svcs.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            query_builder = query_builder.bind(s);
        }
    }

    if let Some(ref ft_types) = file_types_filter {
        for t in ft_types
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            query_builder = query_builder.bind(t);
        }
    }

    if let Some(pid) = playlist_id_filter
        && playlists_filter.is_none()
    {
        query_builder = query_builder.bind(pid);
    }

    // Playlists filter binds
    if let Some(ref pl_names) = playlists_filter {
        for name in pl_names
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            query_builder = query_builder.bind(name);
        }
    }

    // Tags filter binds
    if let Some(ref tags_str) = tags_filter {
        for tag in tags_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            query_builder = query_builder.bind(tag);
        }
    }

    // PMV categories filter binds
    if let Some(ref pmv_cats) = pmv_categories_filter {
        for cat in pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            query_builder = query_builder.bind(cat);
        }
    }

    // Date filter binds
    if let Some(days) = imported_after_days_filter {
        query_builder = query_builder.bind(format!("-{} days", days));
    }
    if let Some(days) = imported_before_days_filter {
        query_builder = query_builder.bind(format!("-{} days", days));
    }
    if let Some(days) = added_after_days_filter {
        query_builder = query_builder.bind(format!("-{} days", days));
    }
    if let Some(days) = added_before_days_filter {
        query_builder = query_builder.bind(format!("-{} days", days));
    }

    // ── File metrics filter binds ──
    if let Some(bpm_min) = bpm_min_filter {
        query_builder = query_builder.bind(bpm_min);
    }
    if let Some(bpm_max) = bpm_max_filter {
        query_builder = query_builder.bind(bpm_max);
    }
    if let Some(ref keys_str) = keys_filter {
        for k in keys_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            query_builder = query_builder.bind(k);
        }
    }
    if let Some(rating_min) = rating_min_filter {
        query_builder = query_builder.bind(rating_min);
    }
    if let Some(play_count_min) = play_count_min_filter {
        query_builder = query_builder.bind(play_count_min);
    }

    let tracks = query_builder
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

    if tracks.is_empty() {
        return Ok(vec![]);
    }

    // Get local file types for these tracks
    let track_ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let placeholders: Vec<String> = track_ids.iter().map(|_| "?".to_string()).collect();
    let ids_list = placeholders.join(", ");

    let match_sql = format!(
        "SELECT st.id, COALESCE(GROUP_CONCAT(DISTINCT f.file_type), '') as file_types
         FROM service_tracks st
         LEFT JOIN v_file_track_link v ON v.track_id = st.id
         LEFT JOIN files f ON f.id = v.file_id
         WHERE st.id IN ({})
         GROUP BY st.id",
        ids_list
    );

    let mut match_query = sqlx::query(&match_sql);
    for id in &track_ids {
        match_query = match_query.bind(id);
    }

    let match_rows = match_query.fetch_all(pool).await?;
    let mut files_map: std::collections::HashMap<i64, Vec<String>> =
        std::collections::HashMap::new();
    for row in match_rows {
        let track_id: i64 = row.try_get("id")?;
        let files_str: String = row.try_get("file_types")?;
        let file_types: Vec<String> = if files_str.is_empty() {
            vec![]
        } else {
            files_str.split(',').map(|s| s.to_string()).collect()
        };
        files_map.insert(track_id, file_types);
    }

    // Get playlist names + max added_at for these tracks
    let playlist_sql = format!(
        "SELECT spt.track_id,
                COALESCE(GROUP_CONCAT(DISTINCT sp.name), '') as playlist_names,
                MAX(spt.added_at) as max_added_at
         FROM service_playlist_tracks spt
         JOIN service_playlists sp ON sp.id = spt.playlist_id
         WHERE spt.track_id IN ({})
         GROUP BY spt.track_id",
        ids_list
    );

    let mut playlist_query = sqlx::query(&playlist_sql);
    for id in &track_ids {
        playlist_query = playlist_query.bind(id);
    }

    let playlist_rows = playlist_query.fetch_all(pool).await?;
    let mut playlist_map: std::collections::HashMap<i64, Vec<String>> =
        std::collections::HashMap::new();
    let mut max_added_at_map: std::collections::HashMap<i64, i64> =
        std::collections::HashMap::new();
    for row in playlist_rows {
        let track_id: i64 = row.try_get("track_id")?;
        let names_str: String = row.try_get("playlist_names")?;
        let names: Vec<String> = if names_str.is_empty() {
            vec![]
        } else {
            names_str.split(',').map(|s| s.trim().to_string()).collect()
        };
        playlist_map.insert(track_id, names);
        // max_added_at may be NULL if no rows matched (shouldn't happen due to JOIN)
        if let Ok(ts) = row.try_get::<i64, _>("max_added_at") {
            max_added_at_map.insert(track_id, ts);
        }
    }

    // Get playlist tag info (with category/prefix/icon) for these tracks
    let tag_sql = format!(
        concat!(
            "SELECT spt.track_id, sp.name as playlist_name, t.name as tag_name, ",
            "tc.name as category, tc.prefix, tc.icon ",
            "FROM service_playlist_tracks spt ",
            "JOIN service_playlists sp ON sp.id = spt.playlist_id ",
            "LEFT JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name)) ",
            "LEFT JOIN tag_categories tc ON tc.id = t.category_id ",
            "WHERE spt.track_id IN ({}) AND t.id IS NOT NULL",
        ),
        ids_list
    );

    let mut tag_query = sqlx::query(&tag_sql);
    for id in &track_ids {
        tag_query = tag_query.bind(id);
    }

    let tag_rows = tag_query.fetch_all(pool).await?;
    let mut tag_map: std::collections::HashMap<i64, Vec<PlaylistTagInfo>> =
        std::collections::HashMap::new();
    for row in tag_rows {
        let track_id: i64 = row.try_get("track_id")?;
        let playlist_name: String = row.try_get("playlist_name")?;
        let tag_name: String = row.try_get("tag_name")?;
        let category: String = row.try_get("category")?;
        let prefix: String = row.try_get("prefix")?;
        let icon: String = row.try_get("icon")?;
        tag_map.entry(track_id).or_default().push(PlaylistTagInfo {
            playlist_name,
            tag_name,
            category,
            prefix,
            icon,
        });
    }

    // Get format info (file type + local/backup status) via ISRC
    let isrcs: Vec<&str> = tracks
        .iter()
        .filter_map(|t| t.isrc.as_deref())
        .filter(|i| !i.is_empty())
        .collect();

    let mut formats_by_isrc: std::collections::HashMap<String, Vec<TrackFormatInfo>> =
        std::collections::HashMap::new();

    if !isrcs.is_empty() {
        let isrc_placeholders: Vec<String> = isrcs.iter().map(|_| "?".to_string()).collect();
        let fmt_sql = format!(
            r#"SELECT f.isrc, f.file_type,
                      EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = f.id AND fl.location_type = 'local') as local,
                      EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = f.id AND fl.location_type = 'backup') as backup
               FROM files f
               WHERE f.isrc IN ({})
               ORDER BY f.isrc, f.file_type"#,
            isrc_placeholders.join(",")
        );
        let mut fmt_q = sqlx::query(&fmt_sql);
        for isrc in &isrcs {
            fmt_q = fmt_q.bind(isrc);
        }
        if let Ok(format_rows) = fmt_q.fetch_all(pool).await {
            for row in format_rows {
                let isrc: String = row.try_get("isrc").unwrap_or_default();
                let file_type: String = row.try_get("file_type").unwrap_or_default();
                let local: bool = row.try_get("local").unwrap_or(false);
                let backup: bool = row.try_get("backup").unwrap_or(false);
                formats_by_isrc
                    .entry(isrc)
                    .or_default()
                    .push(TrackFormatInfo {
                        file_type,
                        local,
                        backup,
                    });
            }
        }
    }

    // Compute in_backpack for these tracks (batch query)
    let backpack_sql = format!(
        "SELECT DISTINCT spt.track_id FROM service_playlist_tracks spt
         JOIN service_playlists sp ON sp.id = spt.playlist_id
         LEFT JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
         WHERE spt.track_id IN ({}) AND (t.backpack = 1 OR LOWER(TRIM(sp.name)) = 'backpack')",
        ids_list
    );

    let mut backpack_query = sqlx::query_scalar::<_, i64>(&backpack_sql);
    for id in &track_ids {
        backpack_query = backpack_query.bind(id);
    }

    let backpack_track_ids: std::collections::HashSet<i64> = backpack_query
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();

    // ── Step 7: Aggregate file metrics per track (BPM, Key, Rating, Play Count, Last Played) ──
    // Query all linked files with metrics, ordered by format priority per track.
    // In Rust we keep the first row per track (best format) and compute aggregates.
    #[derive(Debug, Default, Clone)]
    struct AggregatedMetrics {
        bpm_primary: Option<f64>,
        bpm_values: Vec<(String, f64)>, // (file_type, bpm)
        musical_key: Option<String>,
        rating: i32,
        play_count: i32,
        last_played: Option<i64>,
    }

    let metrics_sql = format!(
        concat!(
            "SELECT vft.track_id, f.bpm, f.musical_key, f.rating, f.play_count, ",
            "f.last_played, f.file_type ",
            "FROM v_file_track_link vft ",
            "JOIN files f ON f.id = vft.file_id ",
            "WHERE vft.track_id IN ({}) ",
            "ORDER BY vft.track_id, ",
            "  CASE f.file_type ",
            "    WHEN 'stem.m4a' THEN 0 ",
            "    WHEN 'flac' THEN 1 ",
            "    WHEN 'mp3' THEN 2 ",
            "    WHEN 'wav' THEN 3 ",
            "    ELSE 4 ",
            "  END"
        ),
        ids_list
    );

    let mut metrics_q = sqlx::query(&metrics_sql);
    for id in &track_ids {
        metrics_q = metrics_q.bind(id);
    }

    let metrics_rows = metrics_q.fetch_all(pool).await.unwrap_or_default();
    let mut metrics_map: std::collections::HashMap<i64, AggregatedMetrics> =
        std::collections::HashMap::new();

    for row in &metrics_rows {
        let track_id: i64 = match row.try_get("track_id") {
            Ok(id) => id,
            Err(_) => continue,
        };
        let bpm: Option<f64> = row.try_get("bpm").ok().flatten();
        let musical_key: Option<String> = row.try_get("musical_key").ok().flatten();
        let rating: i32 = row
            .try_get::<Option<i32>, _>("rating")
            .ok()
            .flatten()
            .unwrap_or(0);
        let play_count: i32 = row
            .try_get::<Option<i32>, _>("play_count")
            .ok()
            .flatten()
            .unwrap_or(0);
        let last_played: Option<i64> = row.try_get("last_played").ok().flatten();
        let file_type: String = row.try_get("file_type").unwrap_or_default();

        let entry = metrics_map.entry(track_id).or_default();
        // Set best-format key (first non-null by format priority)
        if entry.musical_key.is_none() {
            entry.musical_key = musical_key.clone();
        }
        // Track distinct BPM values (for display: "159.0 / 160")
        if let Some(b) = bpm {
            if !entry.bpm_values.iter().any(|(_, v)| (v - b).abs() < 0.01) {
                entry.bpm_values.push((file_type, b));
            }
        }
        // Primary BPM is first non-null by format priority (best file)
        if entry.bpm_primary.is_none() && bpm.is_some() {
            entry.bpm_primary = bpm;
        }
        // Rating: max across all files
        if rating > entry.rating {
            entry.rating = rating;
        }
        // Play count: sum across all files
        entry.play_count += play_count;
        // Last played: max across all files
        if let Some(lp) = last_played {
            if entry.last_played.map_or(true, |e| lp > e) {
                entry.last_played = Some(lp);
            }
        }
    }

    Ok(tracks
        .into_iter()
        .map(|t| {
            let mut api_track = ApiServiceTrack::from(t);
            if let Some(file_types) = files_map.remove(&api_track.id) {
                api_track.local_files = file_types;
            }
            if let Some(playlist_names) = playlist_map.remove(&api_track.id) {
                api_track.playlist_names = playlist_names;
            }
            if let Some(playlist_tags) = tag_map.remove(&api_track.id) {
                api_track.playlist_tags = playlist_tags;
            }
            if let Some(ref isrc) = api_track.isrc
                && let Some(formats) = formats_by_isrc.remove(isrc)
            {
                api_track.format_info = formats;
            }
            api_track.max_added_at = max_added_at_map.remove(&api_track.id);
            api_track.in_backpack = backpack_track_ids.contains(&api_track.id);
            // ── Enrich with aggregated file metrics ──
            if let Some(metrics) = metrics_map.remove(&api_track.id) {
                api_track.bpm = metrics.bpm_primary;
                // Build display string: "159.0 / 160" or just "155.0"
                if metrics.bpm_values.len() > 1 {
                    let display: Vec<String> = metrics
                        .bpm_values
                        .iter()
                        .map(|(_, v)| format!("{:.1}", v))
                        .collect();
                    api_track.bpm_display = Some(display.join(" / "));
                } else if let Some(bpm) = metrics.bpm_primary {
                    api_track.bpm_display = Some(format!("{:.1}", bpm));
                }
                api_track.musical_key = metrics.musical_key;
                api_track.rating = Some(metrics.rating);
                api_track.play_count = Some(metrics.play_count);
                api_track.last_played = metrics.last_played;
            }
            api_track
        })
        .collect())
}

async fn get_tracks_count(pool: &Pool<Sqlite>, query: &TracksQuery) -> Result<i64> {
    let service_filter = query.service.clone();
    let services_filter = query.services.clone();
    let file_types_filter = query.file_types.clone();
    let file_type_agg_filter = query.file_type_agg.clone();
    let search_pattern = query.search.as_ref().and_then(|s| {
        if s.is_empty() {
            None
        } else {
            Some(format!("%{}%", s))
        }
    });
    let playlist_id_filter = query.playlist_id;
    let playlists_filter = query.playlists.clone();
    let tags_filter = query.tags.clone();
    let pmv_categories_filter = query.pmv_categories.clone();
    let pmv_aggregate_filter = query.pmv_aggregate.clone();
    let imported_after_days_filter = query.imported_after_days;
    let imported_before_days_filter = query.imported_before_days;
    let added_after_days_filter = query.added_after_days;
    let added_before_days_filter = query.added_before_days;

    let backup_track_ids: std::collections::HashSet<i64> = if query.has_backup == Some(true)
        || query.has_local == Some(true)
    {
        sqlx::query_scalar(
            "SELECT DISTINCT vft.track_id FROM v_file_track_link vft JOIN file_locations fl ON fl.file_id = vft.file_id AND fl.location_type = 'backup'"
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
    } else {
        Default::default()
    };

    let local_track_ids: std::collections::HashSet<i64> = if query.has_local == Some(true) {
        sqlx::query_scalar("SELECT DISTINCT vft.track_id FROM v_file_track_link vft JOIN file_locations fl ON fl.file_id = vft.file_id AND fl.location_type = 'local'")
            .fetch_all(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect()
    } else {
        Default::default()
    };

    let mut sql = if playlists_filter.is_some() {
        "SELECT COUNT(DISTINCT st.id) as count FROM service_tracks st JOIN service_playlist_tracks spt ON spt.track_id = st.id JOIN service_playlists sp ON sp.id = spt.playlist_id WHERE 1=1".to_string()
    } else if playlist_id_filter.is_some() {
        "SELECT COUNT(DISTINCT st.id) as count FROM service_tracks st JOIN service_playlist_tracks spt ON spt.track_id = st.id WHERE 1=1"
            .to_string()
    } else {
        "SELECT COUNT(*) as count FROM service_tracks st WHERE 1=1".to_string()
    };

    if search_pattern.is_some() {
        sql.push_str(" AND (st.title LIKE ? OR st.artist LIKE ? OR st.album LIKE ? OR st.isrc LIKE ? OR EXISTS (SELECT 1 FROM service_playlist_tracks spt2 JOIN service_playlists sp2 ON sp2.id = spt2.playlist_id WHERE spt2.track_id = st.id AND sp2.name LIKE ?))");
    }

    if service_filter.is_some() {
        sql.push_str(" AND st.service = ?");
    }

    if let Some(ref svcs) = services_filter {
        let svc_list: Vec<&str> = svcs
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !svc_list.is_empty() {
            let placeholders: Vec<String> = svc_list.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND st.service IN ({})", placeholders.join(",")));
        }
    }

    if let Some(ref ft_agg) = file_type_agg_filter {
        match ft_agg.as_str() {
            "any" => {
                sql.push_str(
                    " AND EXISTS (SELECT 1 FROM v_file_track_link vft WHERE vft.track_id = st.id)",
                );
            }
            "none" => {
                sql.push_str(" AND NOT EXISTS (SELECT 1 FROM v_file_track_link vft WHERE vft.track_id = st.id)");
            }
            _ => {}
        }
    }

    if let Some(ref ft_types) = file_types_filter {
        let type_list: Vec<&str> = ft_types
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !type_list.is_empty() {
            let placeholders: Vec<String> = type_list.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM v_file_track_link vft2 JOIN files f2 ON f2.id = vft2.file_id WHERE vft2.track_id = st.id AND f2.file_type IN ({}))",
                placeholders.join(",")
            ));
        }
    }

    if playlist_id_filter.is_some() && playlists_filter.is_none() {
        sql.push_str(" AND spt.playlist_id = ?");
    }

    // Playlists filter (multi-name, OR logic)
    if let Some(ref pl_names) = playlists_filter {
        let pl_list: Vec<&str> = pl_names
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !pl_list.is_empty() {
            let lowered: Vec<String> = pl_list.iter().map(|_| "LOWER(?)".to_string()).collect();
            sql.push_str(&format!(" AND LOWER(sp.name) IN ({})", lowered.join(",")));
        }
    }

    // Tags filter
    if let Some(ref tags_str) = tags_filter {
        let tag_list: Vec<&str> = tags_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !tag_list.is_empty() {
            let placeholders: Vec<String> = tag_list.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND trt.tag_name IN ({}))", placeholders.join(",")));
        }
    }

    // PMV filter — categories and aggregate are mutually exclusive
    if let Some(ref pmv_cats) = pmv_categories_filter {
        let cat_list: Vec<String> = pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !cat_list.is_empty() {
            let placeholders: Vec<String> = cat_list.iter().map(|_| "?".to_string()).collect();
            sql.push_str(&format!(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) IN ({}))", placeholders.join(",")));
        }
    } else if let Some(ref pmv_agg) = pmv_aggregate_filter {
        match pmv_agg.as_str() {
            "full" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) = 'p')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) = 'm')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) = 'v')");
            }
            "partial" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) IN ('p','m','v'))");
            }
            "none" => {
                sql.push_str(" AND NOT EXISTS (SELECT 1 FROM track_resolved_tags trt WHERE trt.track_id = st.id AND LOWER(trt.prefix) IN ('p','m','v'))");
            }
            _ => {}
        }
    }

    // Date filters
    if imported_after_days_filter.is_some() {
        sql.push_str(" AND st.imported_at >= unixepoch('now', ?)");
    }
    if imported_before_days_filter.is_some() {
        sql.push_str(" AND st.imported_at <= unixepoch('now', ?)");
    }
    if added_after_days_filter.is_some() {
        sql.push_str(" AND (SELECT MAX(spt4.added_at) FROM service_playlist_tracks spt4 WHERE spt4.track_id = st.id) >= unixepoch('now', ?)");
    }
    if added_before_days_filter.is_some() {
        sql.push_str(" AND (SELECT MAX(spt4.added_at) FROM service_playlist_tracks spt4 WHERE spt4.track_id = st.id) <= unixepoch('now', ?)");
    }

    // Format filter (hasLocal / hasBackup)
    if let Some(true) = query.has_local {
        if !local_track_ids.is_empty() {
            let ids: Vec<String> = local_track_ids.iter().map(|id| id.to_string()).collect();
            sql.push_str(&format!(" AND st.id IN ({})", ids.join(",")));
        } else {
            sql.push_str(" AND 1=0");
        }
    }
    if let Some(true) = query.has_backup {
        if !backup_track_ids.is_empty() {
            let ids: Vec<String> = backup_track_ids.iter().map(|id| id.to_string()).collect();
            sql.push_str(&format!(" AND st.id IN ({})", ids.join(",")));
        } else {
            // No tracks have backup files, force empty result
            sql.push_str(" AND 1=0");
        }
    }

    let mut query_builder = sqlx::query(&sql);

    if let Some(ref pattern) = search_pattern {
        query_builder = query_builder
            .bind(pattern)
            .bind(pattern)
            .bind(pattern)
            .bind(pattern)
            .bind(pattern);
    }

    if let Some(service) = service_filter.as_ref() {
        query_builder = query_builder.bind(service);
    }

    if let Some(ref svcs) = services_filter {
        for s in svcs.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            query_builder = query_builder.bind(s);
        }
    }

    if let Some(ref ft_types) = file_types_filter {
        for t in ft_types
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            query_builder = query_builder.bind(t);
        }
    }

    if let Some(pid) = playlist_id_filter
        && playlists_filter.is_none()
    {
        query_builder = query_builder.bind(pid);
    }

    // Playlists filter binds
    if let Some(ref pl_names) = playlists_filter {
        for name in pl_names
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            query_builder = query_builder.bind(name);
        }
    }

    // Tags filter binds
    if let Some(ref tags_str) = tags_filter {
        for tag in tags_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            query_builder = query_builder.bind(tag);
        }
    }

    // PMV categories filter binds
    if let Some(ref pmv_cats) = pmv_categories_filter {
        for cat in pmv_cats
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            query_builder = query_builder.bind(cat);
        }
    }

    // Date filter binds
    if let Some(days) = imported_after_days_filter {
        query_builder = query_builder.bind(format!("-{} days", days));
    }
    if let Some(days) = imported_before_days_filter {
        query_builder = query_builder.bind(format!("-{} days", days));
    }
    if let Some(days) = added_after_days_filter {
        query_builder = query_builder.bind(format!("-{} days", days));
    }
    if let Some(days) = added_before_days_filter {
        query_builder = query_builder.bind(format!("-{} days", days));
    }

    let row = query_builder.fetch_one(pool).await?;
    Ok(row.try_get("count")?)
}

async fn get_track_by_id(pool: &Pool<Sqlite>, id: i64) -> Result<ApiServiceTrack> {
    let track = sqlx::query_as::<_, ServiceTrack>("SELECT * FROM service_tracks WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await?;

    let mut api_track = ApiServiceTrack::from(track);

    // Get local file types for this track
    let match_sql = r#"SELECT COALESCE(GROUP_CONCAT(DISTINCT f.file_type), '') as file_types
         FROM service_tracks st
         LEFT JOIN v_file_track_link v ON v.track_id = st.id
         LEFT JOIN files f ON f.id = v.file_id
         WHERE st.id = ?"#;

    let file_types_str: String = sqlx::query_scalar::<Sqlite, String>(match_sql)
        .bind(api_track.id)
        .fetch_one(pool)
        .await?;

    if !file_types_str.is_empty() {
        api_track.local_files = file_types_str.split(',').map(|s| s.to_string()).collect();
    }

    // Get playlist tags for this track
    let tag_sql = r"SELECT sp.name as playlist_name, t.name as tag_name,
            tc.name as category, tc.prefix, tc.icon
     FROM service_playlist_tracks spt
     JOIN service_playlists sp ON sp.id = spt.playlist_id
     LEFT JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
     LEFT JOIN tag_categories tc ON tc.id = t.category_id
     WHERE spt.track_id = ? AND t.id IS NOT NULL";

    let tag_rows = sqlx::query(tag_sql)
        .bind(api_track.id)
        .fetch_all(pool)
        .await?;

    for row in tag_rows {
        let playlist_name: String = row.try_get("playlist_name")?;
        let tag_name: String = row.try_get("tag_name")?;
        let category: String = row.try_get("category")?;
        let prefix: String = row.try_get("prefix")?;
        let icon: String = row.try_get("icon")?;
        api_track.playlist_tags.push(PlaylistTagInfo {
            playlist_name,
            tag_name,
            category,
            prefix,
            icon,
        });
    }

    // Get max added_at for this track
    let max_added_at: Option<i64> =
        sqlx::query_scalar("SELECT MAX(added_at) FROM service_playlist_tracks WHERE track_id = ?")
            .bind(api_track.id)
            .fetch_one(pool)
            .await?;
    api_track.max_added_at = max_added_at;

    // Get format info for this track via ISRC
    if let Some(ref isrc) = api_track.isrc
        && !isrc.is_empty()
    {
        let fmt_sql = r#"SELECT f.file_type,
                      EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = f.id AND fl.location_type = 'local') as local,
                      EXISTS (SELECT 1 FROM file_locations fl WHERE fl.file_id = f.id AND fl.location_type = 'backup') as backup
               FROM files f
               WHERE f.isrc = ?
               ORDER BY f.file_type"#;
        if let Ok(format_rows) = sqlx::query(fmt_sql).bind(isrc).fetch_all(pool).await {
            for row in format_rows {
                let file_type: String = row.try_get("file_type").unwrap_or_default();
                let local: bool = row.try_get("local").unwrap_or(false);
                let backup: bool = row.try_get("backup").unwrap_or(false);
                api_track.format_info.push(TrackFormatInfo {
                    file_type,
                    local,
                    backup,
                });
            }
        }
    }

    Ok(api_track)
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/tracks", get(tracks_handler))
        .route("/api/tracks/count", get(tracks_count_handler))
        .route(
            "/api/tracks/needs-comment-count",
            post(tracks_needs_comment_count_handler),
        )
        .route(
            "/api/tracks/write-comments",
            post(tracks_write_comments_handler),
        )
        .route(
            "/api/tracks/needs-refresh-count",
            post(tracks_needs_refresh_count_handler),
        )
        .route(
            "/api/tracks/refresh-comments",
            post(tracks_refresh_comments_handler),
        )
        .route("/api/tracks/{id}", get(track_handler))
        .route("/api/tracks/{id}/detail", get(track_detail_handler))
        .route("/api/tracks/{id}/backpack", post(track_backpack_handler))
}
