use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{get, post, put},
};
use chrono::{DateTime, Duration};
use rspotify::clients::{BaseClient, OAuthClient};
use rspotify::model::Market;
use rspotify::{AuthCodeSpotify, Config, Credentials, OAuth, Token, scopes};
use serde::{Deserialize, Serialize};
use serde_json;
use sqlx::{FromRow, Pool, Row, Sqlite};
use std::sync::Arc;
use tokio_stream::StreamExt;
use tracing;

use crate::AppState;
use crate::config::ServiceCredentials;
#[allow(unused_imports)]
use crate::db::{
    File, Folder, ServiceConfig, ServiceTrack, bulk_check_tags, bulk_create_tags, bulk_review_tags,
    bulk_update_tags, categorize_tag as db_categorize_tag, clear_all_embeddings,
    compute_target_comment, create_tag, create_tag_category, create_tags_from_playlists,
    delete_folder, delete_tag, delete_tag_category, get_all_embeddings, get_embeddings_by_category,
    get_folder_by_id, get_folder_file_count, get_folders as db_get_folders,
    get_playlists_without_tags, get_service_config, get_tag_categories, get_tag_category_by_id,
    get_tag_embedding, get_tag_review_counts, get_unreviewed_tags, scan_folder,
    update_folder_active, update_folder_with_config, update_service_connection_status,
    update_service_tokens, update_tag, update_tag_category_metadata, upsert_tag_embedding,
};
use crate::embeddings::{
    EmbeddingModel, deserialize_embedding, mean_embedding, serialize_embedding, suggest_category,
};
#[allow(unused_imports)]
use crate::tasks::{
    SyncConfig, SyncType, TaskManager, TaskStatus, TaskType, start_write_comment_task,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub category: Option<String>,
    pub category_icon: Option<String>,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCategory {
    pub id: i64,
    pub name: String,
    pub prefix: Option<String>,
    pub icon: String,
    pub is_default: bool,
    pub sort_order: i32,
    pub created_at: Option<i64>,
}

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

// ─── Auto-Categorize Types ───────────────────────────────────────────────────────
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
    pub all_categories: Vec<TagCategory>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorizeRequest {
    pub category_id: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingStatusResponse {
    pub model_loaded: bool,
    pub tags_total: usize,
    pub tags_embedded: usize,
    pub model_version: String,
}

// ─── Bulk Import Types ──────────────────────────────────────────────────────────
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub source_type: String,
    pub identifier: String,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub tags: Vec<Tag>,
    pub isrc: Option<String>,
    pub duration_ms: Option<i64>,
    pub rating: Option<i64>,
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
            rating: Some(file.rating as i32),
            play_count: Some(file.play_count as i32),
            last_played: file.last_played,
            spotify_id: file.spotify_id,
            soundcloud_id: file.soundcloud_id,
            youtube_id: file.youtube_id,
            created_at: file.created_at,
            updated_at: file.updated_at,
            matched_services: vec![],
            comment_target: String::new(),
            comment_needs_update: false,
        }
    }
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
            local_files: vec![],
        }
    }
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
    #[serde(default)]
    pub local_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarityMatch {
    pub candidate: Track,
    pub bpm_diff: f64,
    pub key_relationship: String,
    pub shared_tags: Vec<String>,
    pub similarity_score: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerSeed {
    pub id: i64,
    pub track: Track,
    pub added_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerPreset {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub bpm_tolerance: i64,
    pub bpm_ignore: bool,
    pub harmonic_relative: bool,
    pub harmonic_ignore: bool,
    pub match_setlist: String,
    pub match_artist: String,
    pub match_album: String,
    pub shared_mood_req: i64,
    pub shared_phase_req: i64,
    pub shared_vibe_req: i64,
    pub shared_merkmal_req: i64,
    pub shared_any_req: i64,
    pub config_json: String,
    pub harmonic_modes: Vec<String>,
    pub mandatory_tag_ids: Vec<i64>,
    pub is_default: bool,
    pub created_at: i64,
    pub last_used: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConnection {
    pub service: String,
    pub configured: bool,
    pub connected: bool,
    pub is_syncing: bool,

    pub last_sync: Option<i64>,
    pub playlists_local: i64,
    pub tracks_local: i64,
    pub playlists_remote: i64,
    pub tracks_remote: i64,
    pub sync_current_playlist: Option<i64>,
    pub sync_current_track: Option<i64>,
    pub sync_total_playlists: Option<i64>,
    pub sync_total_tracks: Option<i64>,
    pub sync_log: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInfo {
    pub id: i64,
    pub path: String,
    pub watch_enabled: bool,
    pub scan_recursive: bool,
    pub fixed_extensions: bool,
    pub file_extensions: String,
    pub max_depth: i32,
    pub file_count: i64,
    pub last_scanned: Option<i64>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: i64,
    pub service: String,
    pub playlist_id: String,
    pub name: String,
    pub description: Option<String>,
    pub track_count: i64,
    pub imported_at: i64,
    pub updated_at: i64,
    pub metadata_json: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub key: Option<String>,
    pub tags: Option<String>,
    pub search: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TracksQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub service: Option<String>,
    pub search: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub service: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddSeedRequest {
    pub track_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkTagRequest {
    pub track_ids: Vec<i64>,
    pub tag_names: Vec<String>,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateExplorerPresetRequest {
    pub name: String,
    pub description: Option<String>,
    pub bpm_tolerance: Option<i64>,
    pub bpm_ignore: Option<bool>,
    pub harmonic_relative: Option<bool>,
    pub harmonic_ignore: Option<bool>,
    pub match_setlist: Option<String>,
    pub match_artist: Option<String>,
    pub match_album: Option<String>,
    pub shared_mood_req: Option<i64>,
    pub shared_phase_req: Option<i64>,
    pub shared_vibe_req: Option<i64>,
    pub shared_merkmal_req: Option<i64>,
    pub shared_any_req: Option<i64>,
    pub harmonic_modes: Option<Vec<String>>,
    pub mandatory_tag_ids: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateExplorerPresetRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub bpm_tolerance: Option<i64>,
    pub bpm_ignore: Option<bool>,
    pub harmonic_relative: Option<bool>,
    pub harmonic_ignore: Option<bool>,
    pub match_setlist: Option<String>,
    pub match_artist: Option<String>,
    pub match_album: Option<String>,
    pub shared_mood_req: Option<i64>,
    pub shared_phase_req: Option<i64>,
    pub shared_vibe_req: Option<i64>,
    pub shared_merkmal_req: Option<i64>,
    pub shared_any_req: Option<i64>,
    pub harmonic_modes: Option<Vec<String>>,
    pub mandatory_tag_ids: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerMatchesRequest {
    pub bpm_tolerance: Option<i64>,
    pub bpm_ignore: Option<bool>,
    pub harmonic_relative: Option<bool>,
    pub harmonic_ignore: Option<bool>,
    pub match_setlist: Option<String>,
    pub match_artist: Option<String>,
    pub match_album: Option<String>,
    pub shared_mood_req: Option<i64>,
    pub shared_phase_req: Option<i64>,
    pub shared_vibe_req: Option<i64>,
    pub shared_merkmal_req: Option<i64>,
    pub shared_any_req: Option<i64>,
    pub harmonic_modes: Option<Vec<String>>,
    pub mandatory_tag_ids: Option<Vec<i64>>,
    pub preset_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateServiceConfigRequest {
    pub user_id: Option<String>,
    pub playlist_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddFolderRequest {
    pub path: String,
    pub watch_enabled: bool,
    #[serde(default)]
    pub scan_recursive: bool,
    #[serde(default)]
    pub fixed_extensions: bool,
    #[serde(default = "default_file_extensions")]
    pub file_extensions: String,
    #[serde(default = "default_max_depth")]
    pub max_depth: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFolderRequest {
    pub path: Option<String>,
    pub watch_enabled: Option<bool>,
    pub scan_recursive: Option<bool>,
    pub fixed_extensions: Option<bool>,
    pub file_extensions: Option<String>,
    pub max_depth: Option<i32>,
}

fn default_file_extensions() -> String {
    String::new()
}

fn default_max_depth() -> i32 {
    1
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiResponse<T> {
    pub data: T,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistWithoutTag {
    pub id: i64,
    pub service: String,
    pub name: String,
    pub playlist_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistsWithoutTagsResponse {
    pub playlists: Vec<PlaylistWithoutTag>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTagsFromPlaylistsResponse {
    pub created: usize,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WebSocketEvent {
    NowPlaying {
        track: Track,
        position_ms: i64,
        is_playing: bool,
    },
    PlaybackState {
        is_playing: bool,
    },
    TokenExpired,
    ConnectionStatus {
        connected: bool,
        service: String,
    },
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .without_v07_checks()
        .route("/api/files", get(files_handler))
        .route("/api/files/count", get(files_count_handler))
        .route("/api/files/{id}", get(file_handler))
        .route("/api/files/{id}/sync-comment", post(sync_comment_handler))
        .route("/api/files/{id}/write-comment", post(sync_comment_handler))
        .route("/api/files/bulk-sync", post(bulk_sync_handler))
        .route("/api/files/write-comments", post(bulk_sync_handler))
        .route("/api/tracks", get(tracks_handler))
        .route("/api/tracks/count", get(tracks_count_handler))
        .route("/api/tracks/{id}", get(track_handler))
        .route("/api/tags", get(tags_handler).post(create_tag_handler))
        .route(
            "/api/tags/{id}",
            get(get_tag_handler)
                .put(update_tag_handler)
                .delete(delete_tag_handler),
        )
        .route(
            "/api/tags/from-playlists",
            get(get_playlists_without_tags_handler),
        )
        .route(
            "/api/tags/create-from-playlists",
            post(create_tags_from_playlists_handler),
        )
        .route("/api/tags/unreviewed", get(unreviewed_tags_handler))
        .route("/api/tags/{id}/categorize", put(categorize_tag_handler))
        .route("/api/tags/{id}/suggest", get(suggest_category_handler))
        .route("/api/embeddings/status", get(embeddings_status_handler))
        .route("/api/tags/bulk-import", post(bulk_import_handler))
        .route("/api/tags/bulk-resolve", post(bulk_resolve_handler))
        .route(
            "/api/embeddings/recompute",
            post(recompute_embeddings_handler),
        )
        .route("/api/embeddings/reset-review", post(reset_review_handler))
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
        .route("/api/services", get(services_handler))
        .route("/api/services/{service}/auth", post(service_auth_handler))
        .route(
            "/api/services/{service}/callback",
            get(service_callback_handler),
        )
        .route(
            "/api/services/{service}/config",
            get(service_config_handler),
        )
        .route(
            "/api/services/{service}/config",
            put(update_service_config_handler),
        )
        .route(
            "/api/services/{service}/fetch-counts",
            get(service_fetch_counts_handler),
        )
        .route(
            "/api/services/{service}/sync-status",
            get(service_sync_status_handler),
        )
        .route("/api/services/{service}/sync", post(service_sync_handler))
        .route("/api/playlists", get(playlists_handler))
        .route(
            "/api/playlists/{id}/tracks",
            get(playlist_tracks_handler).post(add_track_to_playlist_handler),
        )
        .route(
            "/api/services/spotify/sync/playlists",
            post(spotify_sync_playlists_handler),
        )
        .route(
            "/api/services/spotify/sync/tracks",
            post(spotify_sync_tracks_handler),
        )
        .route(
            "/api/services/spotify/sync/full",
            post(spotify_sync_full_handler),
        )
        .route(
            "/api/services/spotify/sync/playlists/{playlist_id}/tracks",
            post(spotify_sync_playlist_tracks_handler),
        )
        .route(
            "/api/services/spotify/sync/{task_id}",
            get(spotify_sync_task_handler).delete(spotify_sync_cancel_handler),
        )
        .route("/api/services/{service}/reset", post(service_reset_handler))
        .route("/api/tasks", get(tasks_list_handler))
        .route(
            "/api/tasks/{id}",
            get(task_handler).delete(task_cancel_handler),
        )
        .route("/api/health", get(health_check_handler))
        .route(
            "/api/folders",
            get(folders_handler).post(add_folder_handler),
        )
        .route(
            "/api/folders/{id}",
            get(get_folder_handler)
                .put(update_folder_handler)
                .delete(delete_folder_handler),
        )
        .route("/api/folders/{id}/watch", post(toggle_watch_handler))
        .route("/api/folders/{id}/scan", post(scan_folder_handler))
        .route("/callback", get(legacy_callback_handler))
        .route("/ws/spotify", get(ws_handler))
}

// New generic task API endpoints

/// List tasks with pagination and optional status filter
async fn tasks_list_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TasksQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50).min(200);
    let offset = query.offset.unwrap_or(0);
    let status_filter = query.status.clone().and_then(|s| match s.as_str() {
        "pending" => Some(TaskStatus::Pending),
        "running" => Some(TaskStatus::Running),
        "completed" => Some(TaskStatus::Completed),
        "failed" => Some(TaskStatus::Failed),
        "cancelled" | "canceled" => Some(TaskStatus::Cancelled),
        _ => None,
    });

    let (tasks, total) = state
        .task_manager
        .list_tasks_paginated(limit, offset, status_filter)
        .await;

    Json(ApiResponse {
        data: serde_json::json!({
            "tasks": tasks,
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
    use axum::http::StatusCode;

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
    use axum::http::StatusCode;

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

async fn health_check_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use axum::http::StatusCode;
    use serde_json::json;

    // Check database connection
    match sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&state.db)
        .await
    {
        Ok(_) => axum::Json(json!({
            "status": "ok",
            "database": "connected"
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({
                "status": "error",
                "database": e.to_string()
            })),
        )
            .into_response(),
    }
}

async fn files_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FilesQuery>,
) -> impl IntoResponse {
    match get_files(&state.db, &query).await {
        Ok(files) => Json(ApiResponse { data: files }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn files_count_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FilesQuery>,
) -> impl IntoResponse {
    match get_files_count(&state.db, &query).await {
        Ok(count) => Json(ApiResponse { data: count }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
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

async fn tracks_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TracksQuery>,
) -> impl IntoResponse {
    match get_tracks(&state.db, &query).await {
        Ok(tracks) => Json(ApiResponse { data: tracks }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn tracks_count_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TracksQuery>,
) -> impl IntoResponse {
    match get_tracks_count(&state.db, &query).await {
        Ok(count) => Json(ApiResponse { data: count }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

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

async fn sync_comment_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let task_id =
        crate::tasks::start_write_comment_task(&state.task_manager, &state.db, vec![id]).await;
    Json(ApiResponse {
        data: serde_json::json!({ "taskId": task_id }),
    })
    .into_response()
}

async fn bulk_sync_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Find all files and compute which need comment updates
    let file_ids = match sqlx::query_as::<_, crate::db::File>("SELECT * FROM files")
        .fetch_all(&state.db)
        .await
    {
        Ok(all_files) => {
            let mut ids = Vec::new();
            for file in &all_files {
                // Quick check: compute target and compare
                match crate::db::compute_target_comment(&state.db, file.id).await {
                    Ok(target) => {
                        if file.comment.as_deref() != Some(&target) {
                            ids.push(file.id);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Could not compute target for file {}: {}", file.id, e);
                        // Include file anyway — worker will log the error
                        ids.push(file.id);
                    }
                }
            }
            ids
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to fetch files: {}", e),
                }),
            )
                .into_response();
        }
    };

    if file_ids.is_empty() {
        return Json(ApiResponse {
            data: serde_json::json!({ "taskId": null, "message": "All comments are up to date" }),
        })
        .into_response();
    }

    let task_id =
        crate::tasks::start_write_comment_task(&state.task_manager, &state.db, file_ids).await;
    Json(ApiResponse {
        data: serde_json::json!({ "taskId": task_id }),
    })
    .into_response()
}

async fn explorer_seeds_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_explorer_seeds(&state.db).await {
        Ok(seeds) => Json(ApiResponse { data: seeds }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn add_seed_handler(
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<AddSeedRequest>,
) -> impl IntoResponse {
    // TODO: Implement add seed
    Json(ApiResponse {
        data: "add_seed_handler not implemented",
    })
}

async fn explorer_matches_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match find_similarity_matches(&state.db).await {
        Ok(matches) => Json(ApiResponse { data: matches }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn explorer_matches_with_config_handler(
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<ExplorerMatchesRequest>,
) -> impl IntoResponse {
    // TODO: Implement matches with config
    Json(ApiResponse {
        data: "explorer_matches_with_config not implemented",
    })
}

async fn explorer_presets_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    // TODO: Implement get explorer presets
    Json(ApiResponse {
        data: "explorer_presets not implemented",
    })
}

async fn create_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<CreateExplorerPresetRequest>,
) -> impl IntoResponse {
    // TODO: Implement create explorer preset
    Json(ApiResponse {
        data: "create_explorer_preset not implemented",
    })
}

async fn update_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(_request): Json<UpdateExplorerPresetRequest>,
) -> impl IntoResponse {
    // TODO: Implement update explorer preset
    Json(ApiResponse {
        data: format!("update_explorer_preset not implemented for id {}", id),
    })
}

async fn delete_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // TODO: Implement delete explorer preset
    Json(ApiResponse {
        data: format!("delete_explorer_preset not implemented for id {}", id),
    })
}

async fn use_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // TODO: Implement use explorer preset
    Json(ApiResponse {
        data: format!("use_explorer_preset not implemented for id {}", id),
    })
}

async fn remove_seed_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match remove_explorer_seed(&state.db, id).await {
        Ok(_) => Json(ApiResponse { data: "ok" }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn bulk_tag_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<BulkTagRequest>,
) -> impl IntoResponse {
    match apply_bulk_tags(
        &state.db,
        &request.track_ids,
        &request.tag_names,
        request.category.as_deref(),
    )
    .await
    {
        Ok(_) => Json(ApiResponse { data: "ok" }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}
async fn tags_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_all_tags(&state.db).await {
        Ok(tags) => Json(ApiResponse { data: tags }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn tag_categories_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_tag_categories(&state.db).await {
        Ok(categories) => Json(ApiResponse { data: categories }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

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
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

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
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn delete_tag_category_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match delete_tag_category(&state.db, id).await {
        Ok(_) => Json(ApiResponse { data: () }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

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
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

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
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn create_tag_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateTagRequest>,
) -> impl IntoResponse {
    match create_tag(&state.db, &request.name, request.category_id).await {
        Ok(tag) => {
            // Get tag with category info using helper function
            match get_tag_with_category(&state.db, tag.id).await {
                Ok(Some(api_tag)) => Json(ApiResponse { data: api_tag }).into_response(),
                Ok(None) => {
                    // Fallback: create basic response
                    let api_tag = Tag {
                        id: tag.id,
                        name: tag.name,
                        category: None,
                        category_icon: None,
                        created_at: None,
                    };
                    Json(ApiResponse { data: api_tag }).into_response()
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to fetch tag with category info: {}", e),
                    }),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn update_tag_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<UpdateTagRequest>,
) -> impl IntoResponse {
    match update_tag(&state.db, id, request.name.as_deref(), request.category_id).await {
        Ok(tag) => {
            // Convert to API Tag format with category info
            match get_tag_with_category(&state.db, tag.id).await {
                Ok(Some(api_tag)) => Json(ApiResponse { data: api_tag }).into_response(),
                Ok(None) => {
                    // Fallback: create basic response
                    let api_tag = Tag {
                        id: tag.id,
                        name: tag.name,
                        category: None,
                        category_icon: None,
                        created_at: None,
                    };
                    Json(ApiResponse { data: api_tag }).into_response()
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to fetch tag with category info: {}", e),
                    }),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn delete_tag_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match delete_tag(&state.db, id).await {
        Ok(_) => Json(ApiResponse { data: () }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

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
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

// ─── Auto-Categorize Handlers ─────────────────────────────────────────────────

/// GET /api/tags/unreviewed
/// Returns the queue of unreviewed tags (reviewed_at IS NULL).
async fn unreviewed_tags_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Get unreviewed tags + counts
    let (reviewed, unreviewed) = match get_tag_review_counts(&state.db).await {
        Ok(counts) => counts,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to get review counts: {}", e),
                }),
            )
                .into_response();
        }
    };

    let tags = match get_unreviewed_tags(&state.db).await {
        Ok(tags) => tags,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to get unreviewed tags: {}", e),
                }),
            )
                .into_response();
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

/// PUT /api/tags/{id}/categorize
/// Setzt category_id + reviewed_at für einen Tag.
/// Aktualisiert danach den Embedding-Cache (Category Mean).
async fn categorize_tag_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<CategorizeRequest>,
) -> impl IntoResponse {
    // 1. Hole alten Tag (für alte category_id)
    let _old_tag = match crate::db::get_tag_by_id(&state.db, id).await {
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
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
    };

    // 2. Update category_id + reviewed_at
    match db_categorize_tag(&state.db, id, request.category_id).await {
        Ok(tag) => {
            // 3. Embedding-Cache aktualisieren (falls Modell geladen)
            let mut cache = state.embeddings.lock().await;
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

                if let Some(blob) = embedding_blob {
                    if let Ok(_vec) = deserialize_embedding(&blob) {
                        // Aktualisiere Cache (in-Memory Category Means)
                        // Die Category Means werden beim nächsten suggest
                        // automatisch aus der DB neu geladen
                        tracing::debug!(
                            "Updated embedding for tag '{}' -> category {}",
                            tag.name,
                            request.category_id
                        );
                    }
                }
            }

            // API-Tag mit Category-Info zurückgeben
            match crate::api::get_tag_with_category(&state.db, tag.id).await {
                Ok(Some(api_tag)) => Json(ApiResponse { data: api_tag }).into_response(),
                Ok(None) => Json(ApiResponse {
                    data: Tag {
                        id: tag.id,
                        name: tag.name,
                        category: None,
                        category_icon: None,
                        created_at: None,
                    },
                })
                .into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to fetch tag after categorize: {}", e),
                    }),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

/// GET /api/tags/{id}/suggest
/// Berechnet die AI-Empfehlung für einen Tag:
///   1. Tag-Embedding aus DB laden oder berechnen
///   2. Category-Embeddings aus DB berechnen (Mean pro Kategorie)
///   3. Cosine Similarity zu jeder Category (exkl. Setlist)
///   4. Top-1 + alle Categories zurückgeben
async fn suggest_category_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // 1. Tag aus DB holen
    let tag = match crate::db::get_tag_by_id(&state.db, id).await {
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
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
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
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: format!("Failed to load embedding model: {}", e),
                        }),
                    )
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
                match cache.as_ref().unwrap().embed_text(&tag.name) {
                    Ok(vec) => {
                        let blob = serialize_embedding(&vec);
                        let _ = upsert_tag_embedding(&state.db, tag.id, &blob, "all-MiniLM-L6-v2")
                            .await;
                        vec
                    }
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(ErrorResponse {
                                error: format!("Failed to compute embedding: {}", e),
                            }),
                        )
                            .into_response();
                    }
                }
            }
        },
        _ => {
            // Neu berechnen
            let cache = state.embeddings.lock().await;
            match cache.as_ref().unwrap().embed_text(&tag.name) {
                Ok(vec) => {
                    let blob = serialize_embedding(&vec);
                    let _ =
                        upsert_tag_embedding(&state.db, tag.id, &blob, "all-MiniLM-L6-v2").await;
                    vec
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: format!("Failed to compute embedding: {}", e),
                        }),
                    )
                        .into_response();
                }
            }
        }
    };

    // 4. Alle Kategorien holen (für die Buttons + AI-Suggestion)
    let categories = match get_tag_categories(&state.db).await {
        Ok(cats) => cats,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to get categories: {}", e),
                }),
            )
                .into_response();
        }
    };

    let api_categories: Vec<TagCategory> = categories
        .iter()
        .map(|c| TagCategory {
            id: c.id,
            name: c.name.clone(),
            prefix: Some(c.prefix.clone()),
            icon: c.icon.clone(),
            is_default: c.is_default,
            sort_order: c.sort_order,
            created_at: Some(c.created_at),
        })
        .collect();

    // 5. Category-Embeddings berechnen
    let skip_id = categories
        .iter()
        .find(|c| c.is_default)
        .map(|c| c.id)
        .unwrap_or(-1);

    let mut category_embeddings = std::collections::HashMap::new();
    for cat in &categories {
        if cat.id == skip_id {
            continue; // Setlist überspringen für AI
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
        category_embeddings.insert(cat.id, (cat.name.clone(), mean));
    }

    // 6. Similarity berechnen
    let suggestion = suggest_category(&tag_embedding, &category_embeddings, skip_id);

    let (sug_id, sug_name, confidence) = match suggestion {
        Some(s) => (s.category_id, s.category_name, s.confidence),
        None => {
            // Fallback: erste nicht-default Kategorie
            let fallback = categories.iter().find(|c| !c.is_default);
            match fallback {
                Some(c) => (c.id, c.name.clone(), 0.0),
                None => (-1, "None".to_string(), 0.0),
            }
        }
    };

    Json(ApiResponse {
        data: CategorySuggestionResponse {
            suggested_category_id: sug_id,
            suggested_category_name: sug_name,
            confidence,
            all_categories: api_categories,
        },
    })
    .into_response()
}

/// POST /api/tags/bulk-import
/// Check status of multiple tag names: matched / conflict / not_found
/// Does NOT modify anything — just reports current state.
async fn bulk_import_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<BulkImportRequest>,
) -> impl IntoResponse {
    let names: Vec<String> = request.entries.iter().map(|e| e.name.clone()).collect();
    let category_map: std::collections::HashMap<i64, String> = {
        let cats = match get_tag_categories(&state.db).await {
            Ok(c) => c,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        cats.into_iter().map(|c| (c.id, c.name)).collect()
    };

    let checked = match bulk_check_tags(&state.db, &names).await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
    };

    // Build a lookup: name -> (name, category_id) from request
    let request_map: std::collections::HashMap<&str, i64> = request
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

        let (status, tag_id) = match (current_cat_id, &current_cat_name) {
            (Some(cid), Some(_)) if cid == target_cat_id => ("matched".to_string(), None),
            (Some(cid), Some(cname)) => ("conflict".to_string(), Some(cid)),
            (Some(cid), None) => ("conflict".to_string(), Some(cid)),
            (None, _) => ("not_found".to_string(), None),
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
            status,
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
/// Resolve individual entries: create new tags, move tags to new category, or just mark reviewed.
/// Each entry is processed independently so partial success is possible.
async fn bulk_resolve_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<BulkResolveRequest>,
) -> impl IntoResponse {
    let category_map: std::collections::HashMap<i64, String> = {
        let cats = match get_tag_categories(&state.db).await {
            Ok(c) => c,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        };
        cats.into_iter().map(|c| (c.id, c.name)).collect()
    };

    let mut results = Vec::new();
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
                match bulk_review_tags(&state.db, &[entry.name.clone()]).await {
                    Ok(count) => {
                        // Get tag id
                        let tag_id = match crate::db::get_tag_by_name(&state.db, &entry.name).await
                        {
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

    Json(ApiResponse { data: results }).into_response()
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
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to reset reviewed_at: {}", e),
            }),
        )
            .into_response(),
    }
}

async fn get_playlists_without_tags_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match get_playlists_without_tags(&state.db).await {
        Ok(playlists) => {
            // Convert ServicePlaylist to PlaylistWithoutTag
            let playlists_without_tags: Vec<PlaylistWithoutTag> = playlists
                .into_iter()
                .map(|p| PlaylistWithoutTag {
                    id: p.id,
                    service: p.service,
                    name: p.name,
                    playlist_id: p.playlist_id,
                })
                .collect();

            let count = playlists_without_tags.len();
            let response = PlaylistsWithoutTagsResponse {
                playlists: playlists_without_tags,
                count,
            };

            Json(ApiResponse { data: response }).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn create_tags_from_playlists_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match create_tags_from_playlists(&state.db).await {
        Ok(created) => {
            let response = CreateTagsFromPlaylistsResponse {
                created,
                message: format!("Created {} tags from playlists", created),
            };
            Json(ApiResponse { data: response }).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn services_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_service_connections(&state.db, &state.config).await {
        Ok(services) => Json(ApiResponse { data: services }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn service_auth_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if service != "spotify" && service != "soundcloud" && service != "youtube" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("Unsupported service: {}", service),
            }),
        )
            .into_response();
    }

    // Check if service is configured in .env file
    match service.as_str() {
        "spotify" => {
            if !state.config.is_spotify_configured() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse {
                        data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
                    }),
                )
                    .into_response();
            }
        }
        "soundcloud" => {
            if !state.config.is_soundcloud_configured() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse {
                        data: "SoundCloud not configured. Add SOUNDCLOUD_API_KEY to .env file"
                            .to_string(),
                    }),
                )
                    .into_response();
            }
        }
        "youtube" => {
            if !state.config.is_youtube_configured() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse {
                        data: "YouTube not configured. Add YOUTUBE_API_KEY to .env file"
                            .to_string(),
                    }),
                )
                    .into_response();
            }
        }
        _ => unreachable!(), // Already filtered above
    }

    // Generate authorization URL based on service
    match service.as_str() {
        "spotify" => {
            // Get credentials from .env configuration
            let client_id = match state.config.spotify_client_id() {
                Ok(id) => id,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to get Spotify client ID: {}", e),
                        }),
                    )
                        .into_response();
                }
            };
            let client_secret = match state.config.spotify_client_secret() {
                Ok(secret) => secret,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to get Spotify client secret: {}", e),
                        }),
                    )
                        .into_response();
                }
            };

            tracing::debug!("Spotify OAuth - Client ID: {}", client_id);
            tracing::debug!(
                "Spotify OAuth - Redirect URI: {}",
                state.config.spotify_redirect_uri
            );

            // Create OAuth credentials and generate authorization URL for Spotify
            let creds = Credentials::new(client_id, client_secret);
            let oauth = OAuth {
                redirect_uri: state.config.spotify_redirect_uri.clone(),
                scopes: scopes!(
                    "playlist-read-private",
                    "playlist-read-collaborative",
                    "user-read-playback-state"
                ),
                ..Default::default()
            };

            let spotify_config = Config::default();
            let spotify = AuthCodeSpotify::with_config(creds, oauth, spotify_config);

            match spotify.get_authorize_url(false) {
                Ok(url) => Json(ApiResponse {
                    data: url.to_string(),
                })
                .into_response(),
                Err(e) => {
                    tracing::error!("Failed to generate authorization URL: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to generate authorization URL: {}", e),
                        }),
                    )
                        .into_response()
                }
            }
        }
        "soundcloud" => {
            // SoundCloud OAuth not yet implemented
            (
                StatusCode::NOT_IMPLEMENTED,
                Json(ApiResponse {
                    data: "SoundCloud OAuth not yet implemented".to_string(),
                }),
            )
                .into_response()
        }
        "youtube" => {
            // YouTube OAuth not yet implemented
            (
                StatusCode::NOT_IMPLEMENTED,
                Json(ApiResponse {
                    data: "YouTube OAuth not yet implemented".to_string(),
                }),
            )
                .into_response()
        }
        _ => unreachable!(), // Already filtered above
    }
}

async fn service_callback_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
    Query(params): Query<CallbackParams>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if service != "spotify" && service != "soundcloud" && service != "youtube" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("Unsupported service: {}", service),
            }),
        )
            .into_response();
    }

    // Check for OAuth errors
    if let Some(error) = params.error {
        tracing::error!("OAuth error: {}", error);
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("OAuth error: {}", error),
            }),
        )
            .into_response();
    }

    // Get authorization code
    let code = match params.code {
        Some(code) => code,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: "Missing authorization code".to_string(),
                }),
            )
                .into_response();
        }
    };

    // Get service config from database
    let _config = match get_service_config(&state.db, &service).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            // Create default config for this service if it doesn't exist
            if let Err(e) = crate::db::update_service_config(&state.db, &service, None, None).await
            {
                tracing::error!("Failed to create service config: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse {
                        data: format!("Failed to create service config: {}", e),
                    }),
                )
                    .into_response();
            }
            // Try to get config again
            match get_service_config(&state.db, &service).await {
                Ok(Some(config)) => config,
                Ok(None) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to retrieve created config for {}", service),
                        }),
                    )
                        .into_response();
                }
                Err(e) => {
                    tracing::error!("Failed to get service config after creation: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to get service config: {}", e),
                        }),
                    )
                        .into_response();
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to get service config: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get service config: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Check if service is configured in .env file and get credentials
    match service.as_str() {
        "spotify" => {
            if !state.config.is_spotify_configured() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse {
                        data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
                    }),
                )
                    .into_response();
            }
        }
        "soundcloud" => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(ApiResponse {
                    data: "SoundCloud OAuth not yet implemented".to_string(),
                }),
            )
                .into_response();
        }
        "youtube" => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(ApiResponse {
                    data: "YouTube OAuth not yet implemented".to_string(),
                }),
            )
                .into_response();
        }
        _ => unreachable!(), // Already filtered above
    }

    // Get Spotify credentials from .env
    let client_id = match state.config.spotify_client_id() {
        Ok(id) => id.to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get Spotify client ID: {}", e),
                }),
            )
                .into_response();
        }
    };
    let client_secret = match state.config.spotify_client_secret() {
        Ok(secret) => secret.to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get Spotify client secret: {}", e),
                }),
            )
                .into_response();
        }
    };

    tracing::debug!("Spotify Callback - Client ID: {}", client_id);
    tracing::debug!(
        "Spotify Callback - Redirect URI: {}",
        state.config.spotify_redirect_uri
    );

    // Create OAuth credentials and exchange code for tokens
    let creds = Credentials::new(&client_id, &client_secret);
    let oauth = OAuth {
        redirect_uri: state.config.spotify_redirect_uri.clone(),
        scopes: scopes!(
            "playlist-read-private",
            "playlist-read-collaborative",
            "user-read-playback-state"
        ),
        ..Default::default()
    };

    let spotify_config = Config::default();
    let spotify = AuthCodeSpotify::with_config(creds, oauth, spotify_config);

    match spotify.request_token(&code).await {
        Ok(_) => {
            // Get tokens from spotify client
            let token_lock = spotify.token.lock().await;
            if let Ok(guard) = token_lock {
                if let Some(token) = &*guard {
                    // Store tokens in database
                    let refresh_token = token.refresh_token.clone();
                    let access_token = token.access_token.clone();
                    let token_expiry = token.expires_at.and_then(|dt| Some(dt.timestamp()));

                    if let Err(e) = crate::db::update_service_tokens(
                        &state.db,
                        &service,
                        refresh_token.as_deref(),
                        Some(&access_token),
                        token_expiry,
                    )
                    .await
                    {
                        tracing::error!("Failed to store tokens: {}", e);
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(ApiResponse {
                                data: format!("Failed to store tokens: {}", e),
                            }),
                        )
                            .into_response();
                    }

                    // Update connection status
                    if let Err(e) =
                        update_service_connection_status(&state.db, &service, true).await
                    {
                        tracing::warn!("Failed to update connection status: {}", e);
                    }

                    return Redirect::to("http://localhost:8000").into_response();
                }
            }

            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: "Failed to retrieve tokens from Spotify client".to_string(),
                }),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Failed to exchange code for tokens: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to exchange code for tokens: {}", e),
                }),
            )
                .into_response()
        }
    }
}

async fn legacy_callback_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<CallbackParams>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    let service = "spotify".to_string();

    // Check for OAuth errors
    if let Some(error) = params.error {
        tracing::error!("OAuth error: {}", error);
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("OAuth error: {}", error),
            }),
        )
            .into_response();
    }

    // Get authorization code
    let code = match params.code {
        Some(code) => code,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: "Missing authorization code".to_string(),
                }),
            )
                .into_response();
        }
    };

    // Get service config from database
    let _config = match get_service_config(&state.db, &service).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            // Create default config for this service if it doesn't exist
            if let Err(e) = crate::db::update_service_config(&state.db, &service, None, None).await
            {
                tracing::error!("Failed to create service config: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse {
                        data: format!("Failed to create service config: {}", e),
                    }),
                )
                    .into_response();
            }
            // Try to get config again
            match get_service_config(&state.db, &service).await {
                Ok(Some(config)) => config,
                Ok(None) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to retrieve created config for {}", service),
                        }),
                    )
                        .into_response();
                }
                Err(e) => {
                    tracing::error!("Failed to get service config after creation: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to get service config: {}", e),
                        }),
                    )
                        .into_response();
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to get service config: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get service config: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Check if Spotify is configured in .env file
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        )
            .into_response();
    }

    // Get Spotify credentials from .env
    let client_id = match state.config.spotify_client_id() {
        Ok(id) => id.to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get Spotify client ID: {}", e),
                }),
            )
                .into_response();
        }
    };
    let client_secret = match state.config.spotify_client_secret() {
        Ok(secret) => secret.to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get Spotify client secret: {}", e),
                }),
            )
                .into_response();
        }
    };

    tracing::debug!("Spotify Legacy Callback - Client ID: {}", client_id);
    tracing::debug!(
        "Spotify Legacy Callback - Redirect URI: {}",
        state.config.spotify_redirect_uri
    );

    // Create OAuth credentials and exchange code for tokens
    let creds = Credentials::new(&client_id, &client_secret);
    let oauth = OAuth {
        redirect_uri: state.config.spotify_redirect_uri.clone(),
        scopes: scopes!(
            "playlist-read-private",
            "playlist-read-collaborative",
            "user-read-playback-state"
        ),
        ..Default::default()
    };

    let spotify_config = Config::default();
    let spotify = AuthCodeSpotify::with_config(creds, oauth, spotify_config);

    match spotify.request_token(&code).await {
        Ok(_) => {
            // Get tokens from spotify client
            let token_lock = spotify.token.lock().await;
            if let Ok(guard) = token_lock {
                if let Some(token) = &*guard {
                    // Store tokens in database
                    let refresh_token = token.refresh_token.clone();
                    let access_token = token.access_token.clone();
                    let token_expiry = token.expires_at.and_then(|dt| Some(dt.timestamp()));

                    if let Err(e) = crate::db::update_service_tokens(
                        &state.db,
                        &service,
                        refresh_token.as_deref(),
                        Some(&access_token),
                        token_expiry,
                    )
                    .await
                    {
                        tracing::error!("Failed to store tokens: {}", e);
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(ApiResponse {
                                data: format!("Failed to store tokens: {}", e),
                            }),
                        )
                            .into_response();
                    }

                    // Update connection status
                    if let Err(e) =
                        update_service_connection_status(&state.db, &service, true).await
                    {
                        tracing::warn!("Failed to update connection status: {}", e);
                    }

                    return Redirect::to("http://localhost:8000").into_response();
                }
            }

            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: "Failed to retrieve tokens from Spotify client".to_string(),
                }),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Failed to exchange code for tokens: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to exchange code for tokens: {}", e),
                }),
            )
                .into_response()
        }
    }
}

async fn update_service_config_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
    Json(request): Json<UpdateServiceConfigRequest>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match crate::db::update_service_config(
        &state.db,
        &service,
        request.user_id.as_deref(),
        request.playlist_id.as_deref(),
    )
    .await
    {
        Ok(_) => Json(ApiResponse {
            data: format!("Service {} configuration updated", service),
        })
        .into_response(),
        Err(e) => {
            tracing::error!("Failed to update service config: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to update service config: {}", e),
                }),
            )
                .into_response()
        }
    }
}

async fn service_config_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match crate::db::get_service_config(&state.db, &service).await {
        Ok(Some(config)) => Json(ApiResponse { data: config }).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse {
                data: format!("Service {} not configured", service),
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

async fn service_sync_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if service != "spotify" && service != "soundcloud" && service != "youtube" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("Unsupported service: {}", service),
            }),
        )
            .into_response();
    }

    // Handle different services
    match service.as_str() {
        "spotify" => return spotify_sync_handler(state, service).await.into_response(),
        "soundcloud" => (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiResponse {
                data: "SoundCloud sync not yet implemented".to_string(),
            }),
        )
            .into_response(),
        "youtube" => (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiResponse {
                data: "YouTube sync not yet implemented".to_string(),
            }),
        )
            .into_response(),
        _ => unreachable!(), // Already filtered above
    }
}

async fn service_reset_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if service != "spotify" && service != "soundcloud" && service != "youtube" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("Unsupported service: {}", service),
            }),
        )
            .into_response();
    }

    // Clear tokens and mark as disconnected
    let now = chrono::Utc::now().timestamp();
    match sqlx::query(
        r#"
        UPDATE service_config
        SET refresh_token = NULL, access_token = NULL, token_expiry = NULL,
            is_connected = 0, last_checked = ?, updated_at = ?
        WHERE service = ?
        "#,
    )
    .bind(now)
    .bind(now)
    .bind(&service)
    .execute(&state.db)
    .await
    {
        Ok(_) => Json(ApiResponse {
            data: format!("Successfully reset connection for {}", service),
        })
        .into_response(),
        Err(e) => {
            tracing::error!("Failed to reset service {}: {}", service, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to reset service: {}", e),
                }),
            )
                .into_response()
        }
    }
}

async fn spotify_sync_handler(state: Arc<AppState>, service: String) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Check if Spotify is configured in .env
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        ).into_response();
    }

    // Get service config to check if authenticated
    let service_config = match get_service_config(&state.db, &service).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: format!("Service {} not configured", service),
                }),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!("Failed to get service config: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get service config: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Check if tokens are available
    if service_config.access_token.is_none() || service_config.refresh_token.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!(
                    "Tokens not configured for {}. Please authenticate first.",
                    service
                ),
            }),
        )
            .into_response();
    }

    // Start full sync using TaskManager
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::Full,
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse { data: task_id }).into_response(),
        Err(e) => {
            tracing::error!("Failed to start Spotify sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start sync: {}", e),
                }),
            )
                .into_response()
        }
    }
}

// Spotify sync task management endpoints

/// Get sync task status
async fn spotify_sync_task_handler(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match state.task_manager.get_sync_progress(&task_id).await {
        Some(progress) => Json(ApiResponse { data: progress }).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse {
                data: format!("Task {} not found", task_id),
            }),
        )
            .into_response(),
    }
}

/// Cancel a sync task
async fn spotify_sync_cancel_handler(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

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

/// Sync only playlists (metadata)
async fn spotify_sync_playlists_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Check if Spotify is configured in .env
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        ).into_response();
    }

    // Start playlists-only sync using TaskManager
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::Playlists,
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse { data: task_id }).into_response(),
        Err(e) => {
            tracing::error!("Failed to start Spotify playlists sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start sync: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// Sync tracks for all playlists
async fn spotify_sync_tracks_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Check if Spotify is configured in .env
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        ).into_response();
    }

    // Start tracks-all sync using TaskManager
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::TracksAll,
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse { data: task_id }).into_response(),
        Err(e) => {
            tracing::error!("Failed to start Spotify tracks sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start sync: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// Sync tracks for specific playlist
async fn spotify_sync_playlist_tracks_handler(
    State(state): State<Arc<AppState>>,
    Path(playlist_id): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Check if Spotify is configured in .env
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        ).into_response();
    }

    // Start tracks-for-playlist sync using TaskManager
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::TracksForPlaylist(playlist_id),
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse { data: task_id }).into_response(),
        Err(e) => {
            tracing::error!("Failed to start Spotify playlist tracks sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start sync: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// Full sync (playlists + all tracks)
async fn spotify_sync_full_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Check if Spotify is configured in .env
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        ).into_response();
    }

    // Start full sync using TaskManager
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::Full,
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse { data: task_id }).into_response(),
        Err(e) => {
            tracing::error!("Failed to start Spotify full sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start sync: {}", e),
                }),
            )
                .into_response()
        }
    }
}

// Get paginated playlists from all services
async fn playlists_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PlaylistsQuery>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Default values
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    let search_term = query.search.clone();
    let service_filter = query.service.clone();

    // Build SQL query with count of tracks
    let mut sql = String::from(
        "SELECT sp.*, COUNT(spt.track_id) as track_count
         FROM service_playlists sp
         LEFT JOIN service_playlist_tracks spt ON sp.id = spt.playlist_id",
    );

    let mut conditions = Vec::new();

    if let Some(service) = &service_filter {
        conditions.push(format!("sp.service = '{}'", service.replace("'", "''")));
    }

    if let Some(search) = &search_term {
        if !search.trim().is_empty() {
            conditions.push(format!("sp.name LIKE '%{}%'", search.replace("'", "''")));
        }
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }

    sql.push_str(" GROUP BY sp.id ORDER BY sp.name LIMIT ? OFFSET ?");

    // Execute query
    match sqlx::query_as::<_, Playlist>(&sql)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
    {
        Ok(playlists) => {
            // Get total count for pagination
            let mut total_sql =
                String::from("SELECT COUNT(DISTINCT sp.id) FROM service_playlists sp");

            if !conditions.is_empty() {
                total_sql.push_str(" WHERE ");
                total_sql.push_str(&conditions.join(" AND "));
            }

            match sqlx::query_scalar::<_, i64>(&total_sql)
                .fetch_one(&state.db)
                .await
            {
                Ok(total) => Json(ApiResponse {
                    data: serde_json::json!({
                        "playlists": playlists,
                        "total": total,
                        "limit": limit,
                        "offset": offset,
                    }),
                })
                .into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to get total count: {}", e),
                    }),
                )
                    .into_response(),
            }
        }

        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to fetch playlists: {}", e),
            }),
        )
            .into_response(),
    }
}

// Add endpoint to get sync status
async fn service_fetch_counts_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Only implement for spotify for now
    if service != "spotify" {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiResponse {
                data: format!("Fetch counts not implemented for {}", service),
            }),
        )
            .into_response();
    }

    // Get service config from database
    let config = match get_service_config(&state.db, &service).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: format!("Service {} not configured", service),
                }),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!("Failed to get service config: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get service config: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Check if Spotify is configured in .env file
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file".to_string(),
            }),
        )
            .into_response();
    }

    // Get Spotify credentials from .env
    let client_id = match state.config.spotify_client_id() {
        Ok(id) => id.to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get Spotify client ID: {}", e),
                }),
            )
                .into_response();
        }
    };
    let client_secret = match state.config.spotify_client_secret() {
        Ok(secret) => secret.to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to get Spotify client secret: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Check if refresh_token and access_token are available
    let (refresh_token, access_token) = match (config.refresh_token, config.access_token) {
        (Some(refresh), Some(access)) => (refresh, access),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: format!(
                        "Tokens not configured for {}. Please authenticate first.",
                        service
                    ),
                }),
            )
                .into_response();
        }
    };

    // Create authenticated Spotify client
    let creds = Credentials::new(&client_id, &client_secret);
    let oauth = OAuth {
        redirect_uri: state.config.spotify_redirect_uri.clone(),
        scopes: scopes!(
            "playlist-read-private",
            "playlist-read-collaborative",
            "user-read-playback-state"
        ),
        ..Default::default()
    };

    let spotify_config = Config {
        token_refreshing: true,
        ..Default::default()
    };

    let spotify = AuthCodeSpotify::with_config(creds, oauth, spotify_config);

    // Set the token manually
    {
        let token_lock = spotify.token.lock().await;
        if let Ok(mut guard) = token_lock {
            *guard = Some(Token {
                refresh_token: Some(refresh_token.clone()),
                access_token: access_token.clone(),
                expires_in: Duration::seconds(3600), // Default
                expires_at: config
                    .token_expiry
                    .and_then(|ts| DateTime::from_timestamp(ts as i64, 0)),
                scopes: Default::default(),
            });
        } else {
            tracing::error!("Failed to acquire token lock");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: "Failed to initialize Spotify client".to_string(),
                }),
            )
                .into_response();
        }
    }

    // Fetch user's playlists just to count them
    let mut playlists_stream = spotify.current_user_playlists();
    let mut total_playlists = 0;
    let mut total_tracks = 0;

    while let Some(playlist_result) = playlists_stream.try_next().await.transpose() {
        match playlist_result {
            Ok(playlist) => {
                total_playlists += 1;
                tracing::debug!(
                    "Counting playlist: {} (#{})",
                    playlist.name,
                    total_playlists
                );

                // Count tracks in this playlist
                let mut items_stream =
                    spotify.playlist_items(playlist.id.clone(), None, Some(Market::FromToken));

                while let Some(item_result) = items_stream.try_next().await.transpose() {
                    match item_result {
                        Ok(item) => {
                            if item.track.is_some() {
                                total_tracks += 1;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to fetch playlist item while counting: {}", e);
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to fetch playlist while counting: {}", e);
                break;
            }
        }
    }

    // Update the counts in database without clearing existing data
    let now = chrono::Utc::now().timestamp();
    if let Err(e) = sqlx::query(
        r#"
        UPDATE service_config
        SET remote_playlists_count = ?,
            remote_tracks_count = ?,
            last_synced = ?,
            updated_at = ?
        WHERE service = ?
        "#,
    )
    .bind(total_playlists as i64)
    .bind(total_tracks as i64)
    .bind(now)
    .bind(now)
    .bind(&service)
    .execute(&state.db)
    .await
    {
        tracing::warn!("Failed to update service counts: {}", e);
        // Continue anyway - we still return the counts we fetched
    }

    Json(ApiResponse {
        data: serde_json::json!({
            "service": service,
            "total_playlists": total_playlists,
            "total_tracks": total_tracks,
            "message": format!("Fetched counts: {} playlists, {} tracks", total_playlists, total_tracks)
        }),
    })
    .into_response()
}

async fn service_sync_status_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match get_service_config(&state.db, &service).await {
        Ok(Some(config)) => Json(ApiResponse { data: config }).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse {
                data: format!("Service {} not configured", service),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse {
                data: format!("Failed to get service config: {}", e),
            }),
        )
            .into_response(),
    }
}

async fn folders_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_folders(&state.db).await {
        Ok(folders) => Json(ApiResponse { data: folders }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn add_folder_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AddFolderRequest>,
) -> impl IntoResponse {
    // Normalize and validate folder path
    let normalized_path = match crate::db::normalize_and_validate_folder_path(&request.path) {
        Ok(path) => path,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
                .into_response();
        }
    };

    // Create folder in database with full configuration
    match crate::db::create_folder_with_config(
        &state.db,
        &normalized_path,
        request.watch_enabled,
        request.scan_recursive,
        request.fixed_extensions,
        request.file_extensions,
        request.max_depth,
    )
    .await
    {
        Ok(folder) => {
            let folder_info = FolderInfo {
                id: folder.id,
                path: folder.folder_path,
                watch_enabled: folder.active,
                scan_recursive: folder.scan_recursive,
                fixed_extensions: folder.fixed_extensions,
                file_extensions: folder.file_extensions,
                max_depth: folder.max_depth,
                file_count: 0,
                last_scanned: folder.last_scanned,
            };
            Json(ApiResponse { data: folder_info }).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn toggle_watch_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Get current folder to know active status
    match get_folder_by_id(&state.db, id).await {
        Ok(Some(folder)) => {
            // Toggle active status
            let new_active = !folder.active;
            match update_folder_active(&state.db, id, new_active).await {
                Ok(updated_folder) => {
                    let file_count = get_folder_file_count(&state.db, updated_folder.id)
                        .await
                        .unwrap_or(0);
                    let folder_info = FolderInfo {
                        id: updated_folder.id,
                        path: updated_folder.folder_path,
                        watch_enabled: updated_folder.active,
                        scan_recursive: updated_folder.scan_recursive,
                        fixed_extensions: updated_folder.fixed_extensions,
                        file_extensions: updated_folder.file_extensions,
                        max_depth: updated_folder.max_depth,
                        file_count,
                        last_scanned: updated_folder.last_scanned,
                    };
                    Json(ApiResponse { data: folder_info }).into_response()
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response(),
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Folder not found with id: {}", id),
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

async fn get_folder_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_folder_by_id(&state.db, id).await {
        Ok(Some(folder)) => {
            let file_count = get_folder_file_count(&state.db, folder.id)
                .await
                .unwrap_or(0);
            let folder_info = FolderInfo {
                id: folder.id,
                path: folder.folder_path,
                watch_enabled: folder.active,
                scan_recursive: folder.scan_recursive,
                fixed_extensions: folder.fixed_extensions,
                file_extensions: folder.file_extensions,
                max_depth: folder.max_depth,
                file_count,
                last_scanned: folder.last_scanned,
            };
            Json(ApiResponse { data: folder_info }).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Folder not found with id: {}", id),
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

async fn update_folder_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<UpdateFolderRequest>,
) -> impl IntoResponse {
    // Validate new path if provided
    let normalized_path = if let Some(path) = &request.path {
        match crate::db::normalize_and_validate_folder_path(path) {
            Ok(path) => Some(path),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
                    .into_response();
            }
        }
    } else {
        None
    };

    // Convert watch_enabled to active
    let active = request.watch_enabled;

    // Update folder in database with full configuration
    match update_folder_with_config(
        &state.db,
        id,
        normalized_path.as_deref(),
        active,
        request.scan_recursive,
        request.fixed_extensions,
        request.file_extensions.as_deref(),
        request.max_depth,
    )
    .await
    {
        Ok(folder) => {
            let file_count = get_folder_file_count(&state.db, folder.id)
                .await
                .unwrap_or(0);
            let folder_info = FolderInfo {
                id: folder.id,
                path: folder.folder_path,
                watch_enabled: folder.active,
                scan_recursive: folder.scan_recursive,
                fixed_extensions: folder.fixed_extensions,
                file_extensions: folder.file_extensions,
                max_depth: folder.max_depth,
                file_count,
                last_scanned: folder.last_scanned,
            };
            Json(ApiResponse { data: folder_info }).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

async fn delete_folder_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match delete_folder(&state.db, id).await {
        Ok(()) => Json(ApiResponse {
            data: format!("Folder {} deleted successfully", id),
        })
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

async fn scan_folder_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // First check if folder exists
    match get_folder_by_id(&state.db, id).await {
        Ok(Some(_)) => {
            // Folder exists, spawn a background task for folder scanning
            let db = state.db.clone();
            tokio::spawn(async move {
                match scan_folder(&db, id).await {
                    Ok(file_count) => {
                        tracing::info!("Scanned {} files in folder {}", file_count, id);
                    }
                    Err(e) => {
                        tracing::error!("Failed to scan folder {}: {}", id, e);
                    }
                }
            });

            // Return immediate response
            Json(ApiResponse {
                data: format!("Started scanning folder {}", id),
            })
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Folder not found with id: {}", id),
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

async fn playlist_tracks_handler(
    State(_state): State<Arc<AppState>>,
    Path(playlist_id): Path<i64>,
) -> impl IntoResponse {
    // TODO: Implement fetching tracks for playlist
    Json(ApiResponse {
        data: format!(
            "Playlist tracks endpoint not implemented for playlist_id: {}",
            playlist_id
        ),
    })
    .into_response()
}

async fn add_track_to_playlist_handler(
    State(_state): State<Arc<AppState>>,
    Path(playlist_id): Path<i64>,
) -> impl IntoResponse {
    // TODO: Implement adding track to playlist
    Json(ApiResponse {
        data: format!(
            "Add track to playlist endpoint not implemented for playlist_id: {}",
            playlist_id
        ),
    })
    .into_response()
}

async fn ws_handler() -> impl IntoResponse {
    // TODO: Implement WebSocket handler
    "WebSocket endpoint".into_response()
}

async fn get_files(pool: &Pool<Sqlite>, query: &FilesQuery) -> Result<Vec<ApiFile>> {
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);

    let files = sqlx::query_as::<_, File>("SELECT * FROM files ORDER BY id LIMIT ? OFFSET ?")
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

    if files.is_empty() {
        return Ok(vec![]);
    }

    // Get matched services for these files
    let file_ids: Vec<i64> = files.iter().map(|f| f.id).collect();
    let placeholders: Vec<String> = file_ids.iter().map(|_| "?".to_string()).collect();

    let match_sql = format!(
        "SELECT f.id, COALESCE(GROUP_CONCAT(DISTINCT st.service), '') as services
         FROM files f
         LEFT JOIN service_tracks st ON (
             st.isrc = f.isrc
             OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
             OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
             OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
         )
         WHERE f.id IN ({})
         GROUP BY f.id",
        placeholders.join(", ")
    );

    let mut match_query = sqlx::query(&match_sql);
    for id in &file_ids {
        match_query = match_query.bind(id);
    }

    let match_rows = match_query.fetch_all(pool).await?;
    let mut services_map: std::collections::HashMap<i64, Vec<String>> =
        std::collections::HashMap::new();
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
    let mut api_files = Vec::new();
    for file in files {
        let mut api_file = ApiFile::from(file);

        // Set matched services
        if let Some(services) = services_map.remove(&api_file.id) {
            api_file.matched_services = services;
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

        api_files.push(api_file);
    }

    Ok(api_files)
}

async fn get_files_count(pool: &Pool<Sqlite>, _query: &FilesQuery) -> Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) as count FROM files")
        .fetch_one(pool)
        .await?;

    Ok(row.try_get("count")?)
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
         LEFT JOIN service_tracks st ON (
             st.isrc = f.isrc
             OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
             OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
             OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
         )
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

    Ok(api_file)
}

async fn get_tracks(pool: &Pool<Sqlite>, query: &TracksQuery) -> Result<Vec<ApiServiceTrack>> {
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    let service_filter = query.service.clone();

    let mut sql = "SELECT * FROM service_tracks".to_string();
    let mut params: Vec<String> = Vec::new();

    if let Some(service) = &service_filter {
        sql.push_str(" WHERE service = ?");
        params.push(service.clone());
    }

    sql.push_str(" ORDER BY id LIMIT ? OFFSET ?");

    let mut query_builder = sqlx::query_as::<_, ServiceTrack>(&sql);

    if let Some(service) = &service_filter {
        query_builder = query_builder.bind(service);
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

    let match_sql = format!(
        "SELECT st.id, COALESCE(GROUP_CONCAT(DISTINCT f.file_type), '') as file_types
         FROM service_tracks st
         LEFT JOIN files f ON (
             f.isrc = st.isrc
             OR (st.service = 'spotify' AND f.spotify_id = st.service_id)
             OR (st.service = 'soundcloud' AND f.soundcloud_id = st.service_id)
             OR (st.service = 'youtube' AND f.youtube_id = st.service_id)
         )
         WHERE st.id IN ({})
         GROUP BY st.id",
        placeholders.join(", ")
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

    Ok(tracks
        .into_iter()
        .map(|t| {
            let mut api_track = ApiServiceTrack::from(t);
            if let Some(file_types) = files_map.remove(&api_track.id) {
                api_track.local_files = file_types;
            }
            api_track
        })
        .collect())
}

async fn get_tracks_count(pool: &Pool<Sqlite>, query: &TracksQuery) -> Result<i64> {
    let service_filter = query.service.clone();

    let mut sql = "SELECT COUNT(*) as count FROM service_tracks".to_string();

    if service_filter.is_some() {
        sql.push_str(" WHERE service = ?");
    }

    let mut query_builder = sqlx::query(&sql);

    if let Some(service) = service_filter.as_ref() {
        query_builder = query_builder.bind(service);
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
         LEFT JOIN files f ON (
             f.isrc = st.isrc
             OR (st.service = 'spotify' AND f.spotify_id = st.service_id)
             OR (st.service = 'soundcloud' AND f.soundcloud_id = st.service_id)
             OR (st.service = 'youtube' AND f.youtube_id = st.service_id)
         )
         WHERE st.id = ?"#;

    let file_types_str: String = sqlx::query_scalar::<Sqlite, String>(match_sql)
        .bind(api_track.id)
        .fetch_one(pool)
        .await?;

    if !file_types_str.is_empty() {
        api_track.local_files = file_types_str.split(',').map(|s| s.to_string()).collect();
    }

    Ok(api_track)
}

async fn get_explorer_seeds(pool: &Pool<Sqlite>) -> Result<Vec<ExplorerSeed>> {
    let rows = sqlx::query(
        "SELECT es.id, es.source_type, es.source_id, es.added_at,
                ut.*
         FROM explorer_seeds es
         LEFT JOIN unified_tracks ut ON
             (es.source_type = 'file' AND ut.source_type = 'file' AND ut.id = es.source_id) OR
             (es.source_type = 'service' AND ut.source_type = 'service' AND ut.id = es.source_id)",
    )
    .fetch_all(pool)
    .await?;

    let mut seeds = Vec::new();
    for row in rows {
        let tags_json: String = row.try_get("tags_json")?;
        let tags: Vec<Tag> = serde_json::from_str(&tags_json).unwrap_or_default();

        let track = Track {
            id: row.try_get("id")?,
            source_type: row.try_get("source_type")?,
            identifier: row.try_get("identifier")?,
            title: row.try_get("title")?,
            artist: row.try_get("artist")?,
            bpm: row.try_get("bpm").ok(),
            key: row.try_get("musical_key").ok(),
            tags,
            isrc: row.try_get("isrc").ok(),
            duration_ms: row.try_get("duration_ms").ok(),
            rating: row.try_get("rating").ok(),
        };

        seeds.push(ExplorerSeed {
            id: row.try_get("id")?,
            track,
            added_at: row.try_get("added_at")?,
        });
    }

    Ok(seeds)
}

// async fn add_explorer_seed(pool: &Pool<Sqlite>, track_id: i64) -> Result<ExplorerSeed> {
//     // TODO: Implement proper seed addition with source type detection
//     let row = sqlx::query("INSERT INTO explorer_seeds (source_type, source_id) VALUES ('file', ?) RETURNING id, source_type, source_id, added_at")
//         .bind(track_id)
//         .fetch_one(pool)
//         .await?;
//
//     // Get the track details
//     let track = get_file_by_id(pool, track_id).await?;
//
//     Ok(ExplorerSeed {
//         id: row.try_get("id")?,
//         track,
//         added_at: row.try_get("added_at")?,
//     })
// }

async fn remove_explorer_seed(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM explorer_seeds WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

async fn find_similarity_matches(_pool: &Pool<Sqlite>) -> Result<Vec<SimilarityMatch>> {
    // Explorer feature disabled for now
    Ok(Vec::new())
}

// async fn calculate_similarity(
//     pool: &Pool<Sqlite>,
//     seed: &Track,
//     candidate: &Track,
// ) -> Result<SimilarityMatch> {
//     // Explorer feature disabled for now
//     Ok(SimilarityMatch {
//         candidate: candidate.clone(),
//         bpm_diff: 0.0,
//         key_relationship: "none".to_string(),
//         shared_tags: vec![],
//         similarity_score: 0.0,
//     })
// }

async fn check_key_compatibility(
    pool: &Pool<Sqlite>,
    seed_key: &str,
    candidate_key: &str,
) -> Result<String> {
    let row = sqlx::query(
        "SELECT relationship FROM key_compatibility
         WHERE original_key = ? AND compatible_key = ?",
    )
    .bind(seed_key)
    .bind(candidate_key)
    .fetch_optional(pool)
    .await?;

    Ok(row
        .and_then(|r| r.try_get("relationship").ok())
        .unwrap_or_else(|| "none".to_string()))
}

async fn apply_bulk_tags(
    _pool: &Pool<Sqlite>,
    _track_ids: &[i64],
    _tag_names: &[String],
    _category: Option<&str>,
) -> Result<()> {
    // TODO: Implement bulk tagging
    Ok(())
}

async fn get_all_tags(pool: &Pool<Sqlite>) -> Result<Vec<Tag>> {
    // Use JOIN query since tags_with_categories view doesn't exist
    let rows = sqlx::query(
        "SELECT t.id, t.name, tc.name as category, tc.icon as category_icon, t.created_at
         FROM tags t
         LEFT JOIN tag_categories tc ON t.category_id = tc.id
         ORDER BY t.name",
    )
    .fetch_all(pool)
    .await?;

    let mut tags = Vec::new();
    for row in rows {
        tags.push(Tag {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            category: row.try_get("category").ok(),
            category_icon: row.try_get("category_icon").ok(),
            created_at: row.try_get("created_at").ok(),
        });
    }

    Ok(tags)
}

/// Get a single tag with category information
async fn get_tag_with_category(pool: &Pool<Sqlite>, tag_id: i64) -> Result<Option<Tag>> {
    let row = sqlx::query(
        "SELECT t.id, t.name, tc.name as category, tc.icon as category_icon, t.created_at
         FROM tags t
         LEFT JOIN tag_categories tc ON t.category_id = tc.id
         WHERE t.id = ?",
    )
    .bind(tag_id)
    .fetch_optional(pool)
    .await?;

    if let Some(row) = row {
        Ok(Some(Tag {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            category: row.try_get("category").ok(),
            category_icon: row.try_get("category_icon").ok(),
            created_at: row.try_get("created_at").ok(),
        }))
    } else {
        Ok(None)
    }
}

async fn get_service_connections(
    pool: &Pool<Sqlite>,
    credentials: &ServiceCredentials,
) -> Result<Vec<ServiceConnection>> {
    // Query all service configurations
    let configs = sqlx::query_as::<_, ServiceConfig>(
        "SELECT * FROM service_config WHERE service IN ('spotify', 'soundcloud', 'youtube')",
    )
    .fetch_all(pool)
    .await?;

    // Create a map for quick lookup
    use std::collections::HashMap;
    let config_map: HashMap<String, ServiceConfig> = configs
        .into_iter()
        .map(|config| (config.service.clone(), config))
        .collect();

    // Expected services
    let expected_services = ["spotify", "soundcloud", "youtube"];

    let mut connections = Vec::new();

    for service_name in &expected_services {
        let configured = match *service_name {
            "spotify" => credentials.is_spotify_configured(),
            "soundcloud" => credentials.is_soundcloud_configured(),
            "youtube" => credentials.is_youtube_configured(),
            _ => false,
        };

        // Get counts for this service
        let playlists_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM service_playlists WHERE service = ?",
        )
        .bind(*service_name)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        let tracks_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM service_tracks WHERE service = ?")
                .bind(*service_name)
                .fetch_one(pool)
                .await
                .unwrap_or(0);

        if let Some(config) = config_map.get(*service_name) {
            connections.push(ServiceConnection {
                service: config.service.clone(),
                configured,
                connected: config.is_connected,
                is_syncing: false, // Tracked in memory, not database
                last_sync: config.last_synced,
                playlists_local: playlists_count,
                tracks_local: tracks_count,
                playlists_remote: config.remote_playlists_count,
                tracks_remote: config.remote_tracks_count,
                sync_current_playlist: None, // Tracked in memory
                sync_current_track: None,    // Tracked in memory
                sync_total_playlists: None,  // Tracked in memory
                sync_total_tracks: None,     // Tracked in memory
                sync_log: None,              // Tracked in memory
            });
        } else {
            connections.push(ServiceConnection {
                service: service_name.to_string(),
                configured,
                connected: false,
                is_syncing: false,
                last_sync: None,
                playlists_local: playlists_count,
                tracks_local: tracks_count,
                playlists_remote: 0,
                tracks_remote: 0,
                sync_current_playlist: None,
                sync_current_track: None,
                sync_total_playlists: None,
                sync_total_tracks: None,
                sync_log: None,
            });
        }
    }

    Ok(connections)
}

async fn get_folders(pool: &Pool<Sqlite>) -> Result<Vec<FolderInfo>> {
    let folders = db_get_folders(pool).await?;

    // Convert Folder to FolderInfo with file counts
    let mut folder_infos = Vec::new();
    for folder in folders {
        let file_count = get_folder_file_count(pool, folder.id).await.unwrap_or(0);
        folder_infos.push(FolderInfo {
            id: folder.id,
            path: folder.folder_path,
            watch_enabled: folder.active,
            scan_recursive: folder.scan_recursive,
            fixed_extensions: folder.fixed_extensions,
            file_extensions: folder.file_extensions,
            max_depth: folder.max_depth,
            file_count,
            last_scanned: folder.last_scanned,
        });
    }

    Ok(folder_infos)
}

async fn handle_websocket() {
    // TODO: Implement WebSocket handling
}

impl Default for FilesQuery {
    fn default() -> Self {
        FilesQuery {
            limit: Some(20),
            offset: Some(0),
            bpm_min: None,
            bpm_max: None,
            key: None,
            tags: None,
            search: None,
        }
    }
}
