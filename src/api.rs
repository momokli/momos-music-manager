// API surface: struct fields used only by serde deserialization are intentionally
// kept for future API consumers. Handler functions not yet wired are for planned routes.
#![allow(dead_code)]

use anyhow::Result;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{Request, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get, post, put},
};
use chrono::{DateTime, Duration};
use rspotify::clients::{BaseClient, OAuthClient};
use rspotify::model::Market;
use rspotify::{AuthCodeSpotify, Config, Credentials, OAuth, Token, scopes};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Row, Sqlite};
use std::io::Write;
use std::sync::Arc;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::process::Command as TokioCommand;
use tokio_stream::StreamExt;
use uuid::Uuid;

use std::collections::HashMap;

use crate::AppState;
use crate::config::ServiceCredentials;
#[allow(unused_imports)]
use crate::db::{
    CurationTag, File, FileDetail, FileDetailPlaylist, FileDetailTag, Folder, KeyComparisonRow,
    KeyComparisonSummary, LinkedTrack, ServiceConfig, ServiceConnections, ServiceTrack,
    TrackDetail, TrackDetailFile, bulk_categorize_tags, bulk_check_tags, bulk_create_tags,
    bulk_review_tags, bulk_update_tags, categorize_tag as db_categorize_tag, clear_all_embeddings,
    compute_target_comment, create_tag, create_tag_category, create_tags_from_playlists,
    delete_folder, delete_tag, delete_tag_category, find_tag_similar_tracks, get_all_embeddings,
    get_curation_queue, get_embeddings_by_category, get_file_detail, get_folder_by_id,
    get_folder_file_count, get_folders as db_get_folders, get_key_comparison,
    get_playlists_without_tags, get_service_config, get_tag_categories, get_tag_category_by_id,
    get_tag_children, get_tag_embedding, get_tag_parents, get_tag_review_counts, get_tags_for_file,
    get_track_detail, get_unreviewed_tags, list_subscriptions, read_comment_from_file, scan_folder,
    set_tag_parents, update_folder_active, update_folder_with_config,
    update_service_connection_status, update_service_tokens, update_tag,
    update_tag_category_metadata, upsert_tag_embedding,
};
use crate::deemix::{
    DeemixAuthRequest, DeemixClient, DeemixCombinedQueueItem, DeemixEnqueueRequest,
};
use crate::digging::{
    DiggingSuggestRequest, TagReorderItem, delete_tag_energy_level, get_multi_seed_suggestions,
    get_tag_energy_levels, reorder_tags_batch, set_tag_energy_level,
};
use crate::embeddings::{
    EmbeddingModel, compute_tag_similarities, deserialize_embedding, mean_embedding,
    serialize_embedding, suggest_category,
};
use crate::tasks::{SyncType, TaskStatus};

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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkSyncRequest {
    linked_only: Option<bool>,
    tags: Option<Vec<String>>,
    non_default_only: Option<bool>,
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NeedsUpdateCountQuery {
    pub linked_only: Option<bool>,
    pub tags: Option<String>,
    pub non_default_only: Option<bool>,
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
            max_added_at: None,
            local_files: vec![],
            playlist_names: vec![],
            playlist_tags: vec![],
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
pub struct PlaylistTagInfo {
    pub playlist_name: String,
    pub tag_name: String,
    pub category: String,
    pub prefix: String,
    pub icon: String,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLocalPlaylistRequest {
    pub name: String,
    pub file_ids: Vec<i64>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionStatus {
    pub id: i64,
    pub service: String,
    pub playlist_id: String,
    pub service_playlist_id: Option<i64>,
    pub playlist_name: Option<String>,
    pub track_count: i64,
    pub subscribed_at: i64,
    pub last_polled_at: Option<i64>,
    pub poll_interval_secs: i64,
    pub is_active: bool,
}

impl From<crate::db::PlaylistSubscription> for SubscriptionStatus {
    fn from(s: crate::db::PlaylistSubscription) -> Self {
        SubscriptionStatus {
            id: s.id,
            service: s.service,
            playlist_id: s.playlist_id,
            service_playlist_id: s.service_playlist_id,
            playlist_name: s.playlist_name,
            track_count: s.track_count,
            subscribed_at: s.subscribed_at,
            last_polled_at: s.last_polled_at,
            poll_interval_secs: s.poll_interval_secs,
            is_active: s.is_active,
        }
    }
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
    pub remote_track_count: i64,
    pub remote_unique_count: i64,
    pub last_fetched_at: Option<i64>,
    pub imported_at: i64,
    pub updated_at: i64,
    pub metadata_json: Option<String>,
    pub tag_name: Option<String>,
    pub archive_deleted: bool,
    pub total_track_count: i64,
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
    pub linked_only: Option<bool>,
    pub unlinked: Option<bool>,
    pub non_default_only: Option<bool>,
    pub selected_services: Option<String>,
    pub pmv_categories: Option<String>,
    pub pmv_aggregate: Option<String>,
    pub file_types: Option<String>,
    pub comment_statuses: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
}

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
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub service: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
    pub categories: Option<String>, // comma-separated category IDs: 1,2,3,4,5
    pub subscribed: Option<bool>,   // true = only subscribed, false = only unsubscribed
    pub stale: Option<bool>,        // true = only playlists where local < remote_unique
    pub archive: Option<String>,    // archived/active/all
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoldersQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeemixQueueQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub status: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
}

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TraktorImportRequest {
    /// Optional custom path to collection.nml.
    /// If omitted, auto-detects from ~/Documents/Native Instruments/Traktor
    custom_path: Option<String>,
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

/// Helper that returns a 500 Internal Server Error JSON response from any Display error.
/// GET /api/version — returns the application version from Cargo.toml
async fn version_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

fn internal_error<E: std::fmt::Display>(e: E) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: e.to_string(),
        }),
    )
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

#[allow(clippy::large_enum_variant)]
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

/// Append ORDER BY clause with whitelist validation.
/// Only allows known column names — safe from SQL injection.
pub fn apply_sort(
    sql: &mut String,
    sort: Option<&str>,
    order: Option<&str>,
    whitelist: &[&str],
    default: &str,
) {
    let sort_col = sort.filter(|s| whitelist.contains(s)).unwrap_or(default);
    let ord = match order {
        Some("desc") => "DESC",
        _ => "ASC",
    };
    sql.push_str(format!(" ORDER BY {} {}", sort_col, ord).as_str());
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .without_v07_checks()
        .route("/api/version", get(version_handler))
        .route("/api/tag-energy-levels", get(tag_energy_levels_handler))
        .route(
            "/api/tag-energy-levels/batch",
            put(reorder_tags_batch_handler),
        )
        .route(
            "/api/tag-energy-levels/{tag_id}",
            put(set_tag_energy_level_handler).delete(delete_tag_energy_level_handler),
        )
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
        .route("/api/tags", get(tags_handler).post(create_tag_handler))
        .route("/api/tags/count", get(tags_count_handler))
        .route("/api/tags/curation-queue", get(curation_queue_handler))
        .route(
            "/api/tags/service-coverage",
            get(tags_service_coverage_handler),
        )
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
        .route(
            "/api/tags/{id}/parents",
            get(tag_parents_handler).put(tag_parents_set_handler),
        )
        .route("/api/tags/{id}/children", get(tag_children_handler))
        .route("/api/embeddings/status", get(embeddings_status_handler))
        .route("/api/tags/bulk-categorize", post(bulk_categorize_handler))
        .route("/api/tags/bulk-import", post(bulk_import_handler))
        .route("/api/tags/bulk-resolve", post(bulk_resolve_handler))
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
        .route("/api/playlists/{id}", get(playlist_detail_handler))
        .route(
            "/api/playlists/{id}/tracks",
            get(playlist_tracks_handler).post(add_track_to_playlist_handler),
        )
        .route(
            "/api/playlists/subscriptions",
            get(subscriptions_list_handler).post(subscribe_handler),
        )
        .route(
            "/api/playlists/subscriptions/{id}",
            delete(unsubscribe_handler),
        )
        .route(
            "/api/playlists/comment-diff-stats",
            get(playlist_comment_diff_stats_handler),
        )
        .route("/api/playlists/local", post(create_local_playlist_handler))
        .route(
            "/api/playlists/{id}/archive",
            put(toggle_playlist_archive_handler),
        )
        .route(
            "/api/services/spotify/sync/playlists",
            post(spotify_sync_playlists_handler),
        )
        .route(
            "/api/services/spotify/sync/new-playlists",
            post(spotify_sync_new_playlists_handler),
        )
        .route(
            "/api/services/spotify/sync/playlists/batch",
            post(spotify_sync_playlists_batch_handler),
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
            "/api/services/spotify/refresh-playlist/{playlist_id}",
            post(spotify_refresh_playlist_handler),
        )
        .route(
            "/api/services/spotify/sync/{task_id}",
            get(spotify_sync_task_handler).delete(spotify_sync_cancel_handler),
        )
        .route("/api/services/{service}/reset", post(service_reset_handler))
        .route("/api/services/deemix/auth", post(deemix_auth_handler))
        .route(
            "/api/services/deemix/queue",
            get(deemix_queue_handler).post(deemix_enqueue_handler),
        )
        .route(
            "/api/services/deemix/queue/{id}/retry",
            post(deemix_retry_handler),
        )
        .route(
            "/api/services/deemix/queue/{id}",
            delete(deemix_delete_handler),
        )
        .route("/api/tasks", get(tasks_list_handler))
        .route(
            "/api/tasks/{id}",
            get(task_handler).delete(task_cancel_handler),
        )
        .route("/api/health", get(health_check_handler))
        .route("/api/dump", get(dump_handler))
        .route(
            "/api/restore",
            post(restore_handler).layer(DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route("/api/digging/suggest", post(digging_suggest_handler))
        .route("/api/files/key-comparison", get(key_comparison_handler))
        .route(
            "/api/folders",
            get(folders_handler).post(add_folder_handler),
        )
        .route("/api/folders/count", get(folders_count_handler))
        .route(
            "/api/folders/{id}",
            get(get_folder_handler)
                .put(update_folder_handler)
                .delete(delete_folder_handler),
        )
        .route("/api/folders/{id}/watch", post(toggle_watch_handler))
        .route("/api/folders/{id}/scan", post(scan_folder_handler))
        .route("/api/traktor/import", post(traktor_import_handler))
        .route("/api/traktor/status", get(traktor_status_handler))
        .route("/callback", get(legacy_callback_handler))
        .route("/ws/spotify", get(ws_handler))
}

// New generic task API endpoints

/// List tasks with pagination and optional status filter
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

    let (tasks, total) = state
        .task_manager
        .list_tasks_paginated(limit, offset, status_filter, sort, order)
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
                    " AND EXISTS (SELECT 1 FROM v_file_tags ft WHERE ft.file_id = files.id AND LOWER(TRIM(ft.tag_name)) IN ({}))",
                    placeholders.join(",")
                ));
            tag_params = lowered;
        }
    }

    if query.non_default_only.unwrap_or(false) {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM v_file_tags ft WHERE ft.file_id = files.id AND ft.is_default = FALSE)",
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
    let task_id =
        crate::tasks::start_write_comment_task(&state.task_manager, &state.db, needs_update).await;

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
        sql.push_str(" AND (title LIKE ? OR artist LIKE ? OR file_path LIKE ?)");
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
            " AND EXISTS (SELECT 1 FROM v_file_tags ft WHERE ft.file_id = files.id AND ft.is_default = FALSE)",
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

    // PMV filter
    if let Some(ref pmv_cats) = filter.pmv_categories {
        let cats: Vec<String> = pmv_cats
            .split(',')
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !cats.is_empty() {
            let mut pmv_clauses: Vec<String> = Vec::new();
            for c in &cats {
                let ch = c.chars().next().unwrap();
                pmv_clauses.push(format!(
                    "(SUBSTR(files.comment, 2, 1) = '{c}' OR SUBSTR(files.comment, 3, 1) = '{c}' OR SUBSTR(files.comment, 4, 1) = '{c}')",
                    c = ch
                ));
            }
            sql.push_str(&format!(
                " AND (files.comment IS NOT NULL AND files.comment LIKE '[___]%' AND ({}))",
                pmv_clauses.join(" OR ")
            ));
        }
    } else if let Some(ref pmv_agg) = filter.pmv_aggregate {
        match pmv_agg.as_str() {
            "full" | "partial" => {
                sql.push_str(
                    " AND (files.comment IS NOT NULL AND files.comment LIKE '[___]%' AND \
                     (SUBSTR(files.comment, 2, 1) IN ('P','M','V') OR \
                      SUBSTR(files.comment, 3, 1) IN ('P','M','V') OR \
                      SUBSTR(files.comment, 4, 1) IN ('P','M','V')))",
                );
            }
            "none" => {
                sql.push_str(
                    " AND (files.comment IS NULL OR files.comment NOT LIKE '[___]%' OR \
                     (SUBSTR(files.comment, 2, 1) NOT IN ('P','M','V') AND \
                      SUBSTR(files.comment, 3, 1) NOT IN ('P','M','V') AND \
                      SUBSTR(files.comment, 4, 1) NOT IN ('P','M','V')))",
                );
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
        q = q.bind(format!("%{}%", search));
        q = q.bind(format!("%{}%", search));
        q = q.bind(format!("%{}%", search));
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
        q = q.bind(format!("%{}%", search));
        q = q.bind(format!("%{}%", search));
        q = q.bind(format!("%{}%", search));
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
    let task_id =
        crate::tasks::start_write_comment_task(&state.task_manager, &state.db, needs_update).await;

    Json(ApiResponse {
        data: FilesBulkWriteCommentsResponse {
            task_id,
            file_count,
        },
    })
    .into_response()
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
    let mut file_query = sqlx::query_as::<_, crate::db::File>(&file_sql);
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
    let file_map: HashMap<i64, &crate::db::File> = files.iter().map(|f| (f.id, f)).collect();

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
                let file_result =
                    sqlx::query_as::<_, crate::db::File>("SELECT * FROM files WHERE id = ?")
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
/// from each file on disk via exiftool, and counts how many tracks have at
/// least one linked file whose on-disk comment differs from the DB.
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
                    if let Err(e) =
                        crate::db::update_file_comment(&state.db, file_id, disk_str).await
                    {
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

/// GET /api/files/service-links
/// Returns counts of files linked to each service (via direct IDs or ISRC matching).
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

/// GET /api/tracks/{id}/detail — Rich detail for a single service track:
/// track metadata + audio features + linked files + tags + playlists.
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

async fn tracks_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TracksQuery>,
) -> impl IntoResponse {
    match get_tracks(&state.db, &query).await {
        Ok(tracks) => Json(ApiResponse { data: tracks }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn tracks_count_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TracksQuery>,
) -> impl IntoResponse {
    match get_tracks_count(&state.db, &query).await {
        Ok(count) => Json(ApiResponse { data: count }).into_response(),
        Err(e) => internal_error(e).into_response(),
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
                    " AND EXISTS (SELECT 1 FROM v_file_tags ft WHERE ft.file_id = files.id AND LOWER(TRIM(ft.tag_name)) IN ({}))",
                    placeholders.join(",")
                ));
            tag_params = lowered;
        }
    }

    if body.non_default_only.unwrap_or(false) {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM v_file_tags ft WHERE ft.file_id = files.id AND ft.is_default = FALSE)",
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
                match crate::db::compute_target_comment(&state.db, file.id).await {
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

    let task_id =
        crate::tasks::start_write_comment_task(&state.task_manager, &state.db, file_ids).await;
    Json(ApiResponse {
        data: serde_json::json!({ "taskId": task_id }),
    })
    .into_response()
}

async fn digging_suggest_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DiggingSuggestRequest>,
) -> impl IntoResponse {
    match get_multi_seed_suggestions(&state.db, &request).await {
        Ok(response) => Json(ApiResponse { data: response }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn explorer_seeds_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_explorer_seeds(&state.db).await {
        Ok(seeds) => Json(ApiResponse { data: seeds }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn add_seed_handler(
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<AddSeedRequest>,
) -> impl IntoResponse {
    // TODO: Implement add seed — needs source type detection (file vs service track)
    Json(ApiResponse {
        data: "add_seed_handler not implemented",
    })
}

async fn explorer_matches_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match find_similarity_matches(&state.db).await {
        Ok(matches) => Json(ApiResponse { data: matches }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn explorer_matches_with_config_handler(
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<ExplorerMatchesRequest>,
) -> impl IntoResponse {
    // TODO: Implement matches with config — apply user's filter/preset configuration to matching
    Json(ApiResponse {
        data: "explorer_matches_with_config not implemented",
    })
}

async fn explorer_presets_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    // TODO: Implement get explorer presets — CRUD for saved match configurations
    Json(ApiResponse {
        data: "explorer_presets not implemented",
    })
}

async fn create_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<CreateExplorerPresetRequest>,
) -> impl IntoResponse {
    // TODO: Implement create explorer preset — save current match config as named preset
    Json(ApiResponse {
        data: "create_explorer_preset not implemented",
    })
}

async fn update_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(_request): Json<UpdateExplorerPresetRequest>,
) -> impl IntoResponse {
    // TODO: Implement update explorer preset — rename or change config of saved preset
    Json(ApiResponse {
        data: format!("update_explorer_preset not implemented for id {}", id),
    })
}

async fn delete_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // TODO: Implement delete explorer preset — remove saved preset by id
    Json(ApiResponse {
        data: format!("delete_explorer_preset not implemented for id {}", id),
    })
}

async fn use_explorer_preset_handler(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // TODO: Implement use explorer preset — apply preset config and return matches
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
        Err(e) => internal_error(e).into_response(),
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
        Err(e) => internal_error(e).into_response(),
    }
}

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

async fn tags_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TagsQuery>,
) -> impl IntoResponse {
    match get_all_tags(&state.db, &query).await {
        Ok(tags) => Json(ApiResponse { data: tags }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn tags_count_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TagsQuery>,
) -> impl IntoResponse {
    match get_tags_count(&state.db, &query).await {
        Ok(count) => Json(ApiResponse { data: count }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn tag_categories_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_tag_categories(&state.db).await {
        Ok(categories) => Json(ApiResponse { data: categories }).into_response(),
        Err(e) => internal_error(e).into_response(),
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
        Err(e) => internal_error(e).into_response(),
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
        Err(e) => internal_error(e).into_response(),
    }
}

async fn delete_tag_category_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match delete_tag_category(&state.db, id).await {
        Ok(_) => Json(ApiResponse { data: () }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

// ─── Tag Energy Levels ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetEnergyLevelRequest {
    energy_level: i32,
}

async fn tag_energy_levels_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_tag_energy_levels(&state.db).await {
        Ok(levels) => Json(ApiResponse { data: levels }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

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

async fn delete_tag_energy_level_handler(
    State(state): State<Arc<AppState>>,
    Path(tag_id): Path<i64>,
) -> impl IntoResponse {
    match delete_tag_energy_level(&state.db, tag_id).await {
        Ok(_) => Json(ApiResponse { data: () }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchReorderRequest {
    tags: Vec<TagReorderItem>,
}

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
///
/// Fast path (model cached in AppState): embed single tag via ML model, upsert to DB,
/// then recompute all pairwise similarities (sub-second for current tag counts).
///
/// Fallback path (no cached model): dispatch a background `RecomputeEmbeddings` task
/// that handles everything.
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
                crate::tasks::start_recompute_embeddings_task(&state.task_manager, &state.db).await;
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
                    let api_tag = Tag {
                        id: tag.id,
                        name: tag.name,
                        category: None,
                        category_icon: None,
                        created_at: None,
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
                    let api_tag = Tag {
                        id: tag.id,
                        name: tag.name,
                        category: None,
                        category_icon: None,
                        created_at: None,
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

async fn delete_tag_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match delete_tag(&state.db, id).await {
        Ok(_) => Json(ApiResponse { data: () }).into_response(),
        Err(e) => internal_error(e).into_response(),
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
        Err(e) => internal_error(e).into_response(),
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
/// Returns count of tags that have matching playlists per service.
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
                    // Die Category Means werden beim nächsten suggest
                    // automatisch aus der DB neu geladen
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
                Err(e) => internal_error(format!("Failed to fetch tag after categorize: {}", e))
                    .into_response(),
            }
        }
        Err(e) => internal_error(e).into_response(),
    }
}

/// POST /api/tags/bulk-categorize
/// Bulk-update category_id for multiple tags.
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

    let api_categories: Vec<TagCategory> = categories
        .iter()
        .filter(|c| Some(c.id) != phase_id)
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

    // 5. Category-Embeddings berechnen (skip Setlist + Phase)
    //     Use the in-memory cache if available
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

        let (status, _tag_id) = match (current_cat_id, &current_cat_name) {
            (Some(cid), Some(_)) if cid == target_cat_id => ("matched".to_string(), None),
            (Some(cid), Some(_cname)) => ("conflict".to_string(), Some(cid)),
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

    // Auto-recompute embeddings and similarities if any tags were created
    if any_created {
        crate::tasks::start_recompute_embeddings_task(&state.task_manager, &state.db).await;
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
        Err(e) => internal_error(e).into_response(),
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
            // Auto-recompute embeddings and similarities for new tags
            if created > 0 {
                crate::tasks::start_recompute_embeddings_task(&state.task_manager, &state.db).await;
            }
            Json(ApiResponse { data: response }).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

async fn services_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match get_service_connections(&state.db, &state.config).await {
        Ok(services) => Json(ApiResponse { data: services }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn service_auth_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if service != "spotify"
        && service != "soundcloud"
        && service != "youtube"
        && service != "deemix"
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("Unsupported service: {}", service),
            }),
        )
            .into_response();
    }

    // Deemix uses its own auth endpoint (/api/services/deemix/auth), not OAuth
    if service == "deemix" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Deemix auth is handled via /api/services/deemix/auth".to_string(),
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

    if service != "spotify"
        && service != "soundcloud"
        && service != "youtube"
        && service != "deemix"
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: format!("Unsupported service: {}", service),
            }),
        )
            .into_response();
    }

    // Deemix does not use OAuth callbacks
    if service == "deemix" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Deemix does not use OAuth callbacks".to_string(),
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
            if let Ok(guard) = token_lock
                && let Some(token) = &*guard
            {
                // Store tokens in database
                let refresh_token = token.refresh_token.clone();
                let access_token = token.access_token.clone();
                let token_expiry = token.expires_at.map(|dt| dt.timestamp());

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
                if let Err(e) = update_service_connection_status(&state.db, &service, true).await {
                    tracing::warn!("Failed to update connection status: {}", e);
                }

                let redirect_url = state.public_url.clone().unwrap_or_else(|| {
                    format!(
                        "http://{}:{}",
                        state.config.server_host, state.config.server_port
                    )
                });
                return Redirect::to(&redirect_url).into_response();
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
            if let Ok(guard) = token_lock
                && let Some(token) = &*guard
            {
                // Store tokens in database
                let refresh_token = token.refresh_token.clone();
                let access_token = token.access_token.clone();
                let token_expiry = token.expires_at.map(|dt| dt.timestamp());

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
                if let Err(e) = update_service_connection_status(&state.db, &service, true).await {
                    tracing::warn!("Failed to update connection status: {}", e);
                }

                let redirect_url = state.public_url.clone().unwrap_or_else(|| {
                    format!(
                        "http://{}:{}",
                        state.config.server_host, state.config.server_port
                    )
                });
                return Redirect::to(&redirect_url).into_response();
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
        Err(e) => internal_error(e).into_response(),
    }
}

async fn service_sync_handler(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if service != "spotify"
        && service != "soundcloud"
        && service != "youtube"
        && service != "deemix"
    {
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
        "spotify" => spotify_sync_handler(state, service).await.into_response(),
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
        "deemix" => (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiResponse {
                data: "Deemix sync uses /api/services/deemix/queue".to_string(),
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

    if service != "spotify"
        && service != "soundcloud"
        && service != "youtube"
        && service != "deemix"
    {
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
    let result = if service == "deemix" {
        // For deemix, clear access_token, metadata_json and mark disconnected
        sqlx::query(
            r#"
            UPDATE service_config
            SET access_token = NULL, metadata_json = NULL,
                is_connected = 0, last_checked = ?, updated_at = ?
            WHERE service = ?
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(&service)
        .execute(&state.db)
        .await
    } else {
        sqlx::query(
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
    };

    match result {
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

// ── Deemix handlers ───────────────────────────────────────────────────

/// POST /api/services/deemix/auth
///
/// Validates ARL against a deemix server, then stores the config.
/// Body: { "arl": "...", "host": "http://localhost:6595" }
async fn deemix_auth_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DeemixAuthRequest>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    let host = request.host.trim_end_matches('/').to_string();
    if host.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Host is required".to_string(),
            }),
        )
            .into_response();
    }
    if request.arl.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "ARL is required".to_string(),
            }),
        )
            .into_response();
    }

    // Build a temporary client to test the ARL
    let client = DeemixClient::new(&host, state.db.clone());
    match client.login_arl(&request.arl).await {
        Ok(_) => {
            // Store ARL as access_token and host in metadata_json
            let metadata = serde_json::json!({"host": host});
            let now = chrono::Utc::now().timestamp();

            let result = sqlx::query(
                r#"
                INSERT INTO service_config (service, access_token, metadata_json, is_connected, last_checked, updated_at, created_at)
                VALUES ('deemix', ?, ?, 1, ?, ?, COALESCE((SELECT created_at FROM service_config WHERE service = 'deemix'), ?))
                ON CONFLICT(service) DO UPDATE SET
                    access_token = excluded.access_token,
                    metadata_json = excluded.metadata_json,
                    is_connected = 1,
                    last_checked = excluded.last_checked,
                    updated_at = excluded.updated_at
                "#,
            )
            .bind(&request.arl)
            .bind(metadata.to_string())
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(&state.db)
            .await;

            match result {
                Ok(_) => {
                    tracing::info!("Deemix configured and connected");
                    Json(ApiResponse {
                        data: serde_json::json!({"status": "connected"}),
                    })
                    .into_response()
                }
                Err(e) => {
                    tracing::error!("Failed to store deemix config: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to store config: {}", e),
                        }),
                    )
                        .into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Deemix auth failed: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: format!("Authentication failed: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// GET /api/services/deemix/queue
///
/// Returns combined list of deemix queue items + local deemix_downloads entries.
async fn deemix_queue_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DeemixQueueQuery>,
) -> impl IntoResponse {
    // Fetch local downloads from deemix_downloads table
    let local_downloads = sqlx::query_as::<
        _,
        (
            i64,
            String,
            Option<String>,
            String,
            i64,
            i64,
            Option<String>,
            Option<i64>,
            Option<i64>,
        ),
    >(
        r#"
        SELECT id, spotify_playlist_url, playlist_name, status,
               track_count_total, track_count_downloaded, error_message,
               created_at, updated_at
        FROM deemix_downloads
        ORDER BY updated_at DESC
        "#,
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    // Try to fetch remote queue from deemix server (if configured)
    let remote_items = match load_deemix_client_from_db(&state.db).await {
        Some(client) => client.get_queue().await.unwrap_or_default(),
        None => std::collections::HashMap::new(),
    };

    // Backfill local deemix_downloads table with remote queue items not yet in DB
    let now = chrono::Utc::now().timestamp();
    for item in remote_items.values() {
        let url = format!("https://open.spotify.com/playlist/{}", item.id);
        let status = match item.status.as_str() {
            "completed" | "withErrors" => "completed",
            "queued" => "queued",
            "downloading" => "downloading",
            _ => "queued",
        };
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO deemix_downloads (spotify_playlist_url, playlist_name, status, track_count_total, track_count_downloaded, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&url)
        .bind(&item.title)
        .bind(status)
        .bind(item.size)
        .bind(item.downloaded)
        .bind(now)
        .bind(now)
        .execute(&state.db)
        .await;
    }

    // Build combined result
    let mut combined: Vec<DeemixCombinedQueueItem> = Vec::new();

    for (id, url, name, status, total, downloaded, error, created, updated) in local_downloads {
        combined.push(DeemixCombinedQueueItem {
            id: Some(id),
            uuid: None,
            spotify_playlist_url: Some(url),
            playlist_name: name,
            status,
            track_count_total: total,
            track_count_downloaded: downloaded,
            error_message: error,
            created_at: created,
            updated_at: updated,
            title: None,
            artist: None,
            progress: 0,
        });
    }

    // Merge remote queue items (they may have richer status info)
    for (uuid, item) in remote_items {
        let status = match item.status.as_str() {
            "completed" => "completed",
            "withErrors" => "completed",
            "queued" => "queued",
            "downloading" => "downloading",
            _ => "queued",
        };

        // Check if we already have this in local list by URL
        let url = format!("https://open.spotify.com/playlist/{}", item.id);
        let existing = combined
            .iter_mut()
            .find(|c| c.spotify_playlist_url.as_deref() == Some(&url));

        if let Some(existing) = existing {
            existing.uuid = Some(uuid);
            existing.status = status.to_string();
            existing.track_count_total = item.size;
            existing.track_count_downloaded = item.downloaded;
            existing.progress = item.progress;
            existing.title = Some(item.title);
            existing.artist = Some(item.artist);
        } else {
            combined.push(DeemixCombinedQueueItem {
                id: None,
                uuid: Some(uuid),
                spotify_playlist_url: Some(url),
                playlist_name: Some(item.title.clone()),
                status: status.to_string(),
                track_count_total: item.size,
                track_count_downloaded: item.downloaded,
                error_message: None,
                created_at: None,
                updated_at: None,
                title: Some(item.title),
                artist: Some(item.artist),
                progress: item.progress,
            });
        }
    }

    // Apply status filter (client-side since combined list merges local + remote)
    let mut filtered: Vec<DeemixCombinedQueueItem> = combined;
    if let Some(ref status_filter) = query.status
        && !status_filter.is_empty()
        && status_filter != "all"
    {
        filtered.retain(|item| item.status == *status_filter);
    }

    // Apply search filter (client-side)
    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        let lower = search.to_lowercase();
        filtered.retain(|item| {
            item.title
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(&lower)
                || item
                    .artist
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&lower)
                || item
                    .playlist_name
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&lower)
                || item
                    .spotify_playlist_url
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&lower)
        });
    }

    // Apply sort (client-side)
    if let Some(sort) = query.sort.as_deref() {
        let order = query.order.as_deref().unwrap_or("asc");
        match (sort, order) {
            ("title", "asc") => filtered.sort_by(|a, b| {
                a.title
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.title.as_deref().unwrap_or(""))
            }),
            ("title", "desc") => filtered.sort_by(|a, b| {
                b.title
                    .as_deref()
                    .unwrap_or("")
                    .cmp(a.title.as_deref().unwrap_or(""))
            }),
            ("artist", "asc") => filtered.sort_by(|a, b| {
                a.artist
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.artist.as_deref().unwrap_or(""))
            }),
            ("artist", "desc") => filtered.sort_by(|a, b| {
                b.artist
                    .as_deref()
                    .unwrap_or("")
                    .cmp(a.artist.as_deref().unwrap_or(""))
            }),
            ("playlist_name", "asc") => filtered.sort_by(|a, b| {
                a.playlist_name
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.playlist_name.as_deref().unwrap_or(""))
            }),
            ("playlist_name", "desc") => filtered.sort_by(|a, b| {
                b.playlist_name
                    .as_deref()
                    .unwrap_or("")
                    .cmp(a.playlist_name.as_deref().unwrap_or(""))
            }),
            ("status", "asc") => filtered.sort_by(|a, b| a.status.cmp(&b.status)),
            ("status", "desc") => filtered.sort_by(|a, b| b.status.cmp(&a.status)),
            ("progress", "asc") => filtered.sort_by(|a, b| a.progress.cmp(&b.progress)),
            ("progress", "desc") => filtered.sort_by(|a, b| b.progress.cmp(&a.progress)),
            ("created_at", "asc") => {
                filtered.sort_by(|a, b| a.created_at.unwrap_or(0).cmp(&b.created_at.unwrap_or(0)))
            }
            ("created_at", "desc") => {
                filtered.sort_by(|a, b| b.created_at.unwrap_or(0).cmp(&a.created_at.unwrap_or(0)))
            }
            ("updated_at", "asc") => {
                filtered.sort_by(|a, b| a.updated_at.unwrap_or(0).cmp(&b.updated_at.unwrap_or(0)))
            }
            ("updated_at", "desc") => {
                filtered.sort_by(|a, b| b.updated_at.unwrap_or(0).cmp(&a.updated_at.unwrap_or(0)))
            }
            ("track_count_total", "asc") => {
                filtered.sort_by(|a, b| a.track_count_total.cmp(&b.track_count_total))
            }
            ("track_count_total", "desc") => {
                filtered.sort_by(|a, b| b.track_count_total.cmp(&a.track_count_total))
            }
            ("track_count_downloaded", "asc") => {
                filtered.sort_by(|a, b| a.track_count_downloaded.cmp(&b.track_count_downloaded))
            }
            ("track_count_downloaded", "desc") => {
                filtered.sort_by(|a, b| b.track_count_downloaded.cmp(&a.track_count_downloaded))
            }
            _ => {}
        }
    }

    // Apply pagination (client-side)
    let total = filtered.len() as i64;
    let page_limit = query.page_size.or(query.limit).unwrap_or(100).min(1000) as usize;
    let page_offset = query.offset.unwrap_or(0) as usize;
    let paged: Vec<DeemixCombinedQueueItem> = filtered
        .into_iter()
        .skip(page_offset)
        .take(page_limit)
        .collect();

    Json(ApiResponse {
        data: serde_json::json!({
            "items": paged,
            "total": total,
        }),
    })
    .into_response()
}

/// POST /api/services/deemix/queue
///
/// Add a Spotify playlist URL to the deemix download queue.
/// Body: { "url": "https://open.spotify.com/playlist/..." }
async fn deemix_enqueue_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DeemixEnqueueRequest>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if request.url.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "URL is required".to_string(),
            }),
        )
            .into_response();
    }

    // Insert into local deemix_downloads table
    let now = chrono::Utc::now().timestamp();
    let insert_result = sqlx::query(
        r#"
        INSERT INTO deemix_downloads (spotify_playlist_url, status, created_at, updated_at)
        VALUES (?, 'queued', ?, ?)
        ON CONFLICT(spotify_playlist_url) DO UPDATE SET
            status = 'queued',
            error_message = NULL,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(&request.url)
    .bind(now)
    .bind(now)
    .execute(&state.db)
    .await;

    if let Err(e) = insert_result {
        tracing::error!("Failed to insert deemix download: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse {
                data: format!("Failed to queue download: {}", e),
            }),
        )
            .into_response();
    }

    // Forward to deemix server
    if let Some(client) = load_deemix_client_from_db(&state.db).await
        && let Err(e) = client.add_to_queue(&request.url).await
    {
        tracing::error!("Failed to forward URL to deemix server: {}", e);
        return (
            StatusCode::BAD_GATEWAY,
            Json(ApiResponse {
                data: format!("Deemix server rejected the request: {}", e),
            }),
        )
            .into_response();
    }

    Json(ApiResponse {
        data: "Playlist added to download queue",
    })
    .into_response()
}

/// POST /api/services/deemix/queue/{id}/retry
///
/// Retry a failed download.
async fn deemix_retry_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Get the URL from the local download
    let url: Option<String> =
        sqlx::query_scalar("SELECT spotify_playlist_url FROM deemix_downloads WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

    let url = match url {
        Some(u) => u,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse {
                    data: format!("Download queue item {} not found", id),
                }),
            )
                .into_response();
        }
    };

    // Reset status to queued locally
    let now = chrono::Utc::now().timestamp();
    let _ = sqlx::query(
        "UPDATE deemix_downloads SET status = 'queued', error_message = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(now)
    .bind(id)
    .execute(&state.db)
    .await;

    // Forward to deemix server (best-effort)
    if let Some(client) = load_deemix_client_from_db(&state.db).await {
        // We need to find the UUID — first get the queue to find it
        match client.get_queue().await {
            Ok(queue) => {
                for (uuid, item) in &queue {
                    let item_url = format!("https://open.spotify.com/playlist/{}", item.id);
                    if item_url == url {
                        if let Err(e) = client.retry_download(uuid).await {
                            tracing::warn!("Failed to retry download on deemix: {}", e);
                        }
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get deemix queue for retry: {}", e);
            }
        }
    }

    Json(ApiResponse {
        data: "Download queued for retry",
    })
    .into_response()
}

/// DELETE /api/services/deemix/queue/{id}
///
/// Remove a queue item from the local database.
async fn deemix_delete_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    match sqlx::query("DELETE FROM deemix_downloads WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(result) => {
            if result.rows_affected() == 0 {
                (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse {
                        data: format!("Download queue item {} not found", id),
                    }),
                )
                    .into_response()
            } else {
                Json(ApiResponse {
                    data: "Download queue item removed",
                })
                .into_response()
            }
        }
        Err(e) => {
            tracing::error!("Failed to delete deemix download: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to delete: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// Helper: load a DeemixClient from the database config.
async fn load_deemix_client_from_db(db: &Pool<Sqlite>) -> Option<DeemixClient> {
    DeemixClient::from_db(db.clone()).await
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

/// Start a new-playlist sync: fetch playlist list from Spotify, diff against DB,
/// only sync metadata + tracks for playlists that don't yet exist.
async fn spotify_sync_new_playlists_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Check if Spotify is configured in .env
    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file"
                    .to_string(),
            }),
        )
            .into_response();
    }

    // Start new-playlists sync using TaskManager
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::NewPlaylists,
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse { data: task_id }).into_response(),
        Err(e) => {
            tracing::error!("Failed to start new-playlist sync: {}", e);
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

/// Refresh a single playlist's remote track count from Spotify metadata.
/// Fast: only 1 API call, no track streaming. Returns old and new counts.
async fn spotify_refresh_playlist_handler(
    State(state): State<Arc<AppState>>,
    Path(playlist_id): Path<String>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured",
            }),
        )
            .into_response();
    }

    let client = match crate::spotify::client::SpotifyClient::from_stored_tokens(
        state.db.clone(),
        &state.config,
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            return internal_error(format!("Failed to create Spotify client: {}", e))
                .into_response();
        }
    };

    // Get the old remote count
    let old_remote: Option<i64> =
        sqlx::query_scalar("SELECT remote_track_count FROM service_playlists WHERE service = 'spotify' AND playlist_id = ?")
            .bind(&playlist_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    // Fetch playlist metadata from Spotify (1 API call, no track streaming)
    let playlist = match client.get_playlist(&playlist_id).await {
        Ok(p) => p,
        Err(e) => {
            return internal_error(format!("Failed to fetch playlist: {}", e)).into_response();
        }
    };

    let new_total = playlist.tracks.total as i64;
    let playlist_name = playlist.name.clone();
    let local_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM service_playlist_tracks spt JOIN service_playlists sp ON sp.id = spt.playlist_id WHERE sp.service = 'spotify' AND sp.playlist_id = ?",
    )
    .bind(&playlist_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    // Update remote_track_count
    let mut conn = match state.db.acquire().await {
        Ok(c) => c,
        Err(e) => {
            return internal_error(format!("DB connection error: {}", e)).into_response();
        }
    };
    if let Err(e) =
        crate::db::update_playlist_fetch_tracking(&mut conn, "spotify", &playlist_id, new_total)
            .await
    {
        return internal_error(format!("Failed to update: {}", e)).into_response();
    }
    drop(conn);

    let changed = old_remote != Some(new_total);

    Json(ApiResponse {
        data: serde_json::json!({
            "playlistId": playlist_id,
            "name": playlist_name,
            "oldRemoteCount": old_remote,
            "newRemoteCount": new_total,
            "localCount": local_count,
            "changed": changed,
        }),
    })
    .into_response()
}

/// Batch sync: fetch tracks for multiple playlists matching a criterion.
/// `mode`: "stale" (local != remote) or "recent" (not fetched in 15+ min).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSyncRequest {
    pub mode: String,
}

async fn spotify_sync_playlists_batch_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BatchSyncRequest>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    if !state.config.is_spotify_configured() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: "Spotify not configured. Add SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to .env file"
                    .to_string(),
            }),
        )
            .into_response();
    }

    // Query for matching Spotify playlists based on mode
    let playlist_ids: Vec<String> = match body.mode.as_str() {
        "stale" => {
            // Playlists where local < remote_unique (missing tracks).
            // Uses remote_unique_count instead of remote_track_count to avoid
            // false positives from episodes/duplicates that don't map to tracks.
            match sqlx::query_scalar::<_, String>(
                r#"
                SELECT sp.playlist_id
                FROM service_playlists sp
                WHERE sp.service = 'spotify'
                  AND (SELECT COUNT(*) FROM service_playlist_tracks spt WHERE spt.playlist_id = sp.id) < sp.remote_unique_count
                "#,
            )
            .fetch_all(&state.db)
            .await
            {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!("Failed to query stale playlists: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to query stale playlists: {}", e),
                        }),
                    )
                        .into_response();
                }
            }
        }
        "recent" => {
            // Playlists not fetched within the last 15 minutes (900 seconds)
            match sqlx::query_scalar::<_, String>(
                r#"
                SELECT sp.playlist_id
                FROM service_playlists sp
                WHERE sp.service = 'spotify'
                  AND (
                      sp.last_fetched_at IS NULL
                      OR sp.last_fetched_at < unixepoch() - 900
                  )
                "#,
            )
            .fetch_all(&state.db)
            .await
            {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!("Failed to query recent playlists: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse {
                            data: format!("Failed to query recent playlists: {}", e),
                        }),
                    )
                        .into_response();
                }
            }
        }
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    data: format!("Invalid mode '{}'. Must be 'stale' or 'recent'.", other),
                }),
            )
                .into_response();
        }
    };

    if playlist_ids.is_empty() {
        return Json(ApiResponse {
            data: serde_json::json!({
                "taskId": null,
                "playlistCount": 0,
                "message": "No matching playlists found"
            }),
        })
        .into_response();
    }

    // Spawn a single batch task for all matching playlists
    match crate::tasks::start_spotify_sync_task(
        &state.task_manager,
        &state.db,
        &state.config,
        SyncType::TracksForPlaylistList(playlist_ids.clone()),
    )
    .await
    {
        Ok(task_id) => Json(ApiResponse {
            data: serde_json::json!({
                "taskId": task_id,
                "playlistCount": playlist_ids.len(),
                "message": format!("Started batch sync for {} playlist(s)", playlist_ids.len())
            }),
        })
        .into_response(),
        Err(e) => {
            tracing::error!("Failed to start batch sync: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse {
                    data: format!("Failed to start batch sync: {}", e),
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

async fn create_local_playlist_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateLocalPlaylistRequest>,
) -> impl IntoResponse {
    use serde_json::json;

    let pool = &state.db;

    // Validate
    if request.name.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Playlist name cannot be empty"
            })),
        )
            .into_response();
    }
    if request.file_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "At least one file ID required"
            })),
        )
            .into_response();
    }

    // 1. Ensure a service_track exists for each file
    let mut track_ids: Vec<i64> = Vec::with_capacity(request.file_ids.len());
    let mut new_tracks: i64 = 0;

    for &file_id in &request.file_ids {
        // Look up the file
        let file = match sqlx::query_as::<_, crate::db::File>("SELECT * FROM files WHERE id = ?")
            .bind(file_id)
            .fetch_optional(pool)
            .await
        {
            Ok(Some(f)) => f,
            Ok(None) => continue,
            Err(e) => {
                tracing::error!("DB error looking up file {}: {}", file_id, e);
                continue;
            }
        };

        // Try to find existing service_track (ISRC match or previous local track)
        let existing_track: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM service_tracks WHERE (service = 'local' AND service_id = CAST(? AS TEXT)) OR (isrc IS NOT NULL AND isrc = ?) LIMIT 1"
        )
        .bind(file_id)
        .bind(&file.isrc)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);

        if let Some(track_id) = existing_track {
            track_ids.push(track_id);
        } else {
            let title = file.title.as_deref().unwrap_or("Unknown");
            let artist = file.artist.as_deref().unwrap_or("Unknown");
            let result = sqlx::query(
                "INSERT INTO service_tracks (service, service_id, title, artist, isrc, imported_at) VALUES ('local', ?, ?, ?, ?, unixepoch())"
            )
            .bind(file_id.to_string())
            .bind(title)
            .bind(artist)
            .bind(&file.isrc)
            .execute(pool)
            .await;

            match result {
                Ok(r) => {
                    track_ids.push(r.last_insert_rowid());
                    new_tracks += 1;
                }
                Err(e) => {
                    tracing::error!("Failed to create local track for file {}: {}", file_id, e);
                    continue;
                }
            }
        }
    }

    if track_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "No valid files could be added"
            })),
        )
            .into_response();
    }

    // 2. Create the local playlist
    // Generate a unique playlist_id since local playlists have no service-side ID
    let playlist_id_str = format!("local-{}", Uuid::new_v4());
    let playlist_result = sqlx::query(
        "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at) VALUES ('local', ?, ?, unixepoch(), unixepoch())"
    )
    .bind(&playlist_id_str)
    .bind(&request.name)
    .execute(pool)
    .await;

    let playlist_id = match playlist_result {
        Ok(r) => r.last_insert_rowid(),
        Err(e) => {
            tracing::error!("Failed to create playlist: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to create playlist"
                })),
            )
                .into_response();
        }
    };

    // 3. Add tracks to playlist
    for track_id in &track_ids {
        if let Err(e) = sqlx::query(
            "INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, added_at) VALUES (?, ?, unixepoch())"
        )
        .bind(playlist_id)
        .bind(track_id)
        .execute(pool)
        .await
        {
            tracing::error!("Failed to add track {} to playlist {}: {}", track_id, playlist_id, e);
        }
    }

    let response = json!({
        "playlistId": playlist_id,
        "trackCount": track_ids.len(),
        "newTrackCount": new_tracks,
    });

    (StatusCode::OK, Json(ApiResponse { data: response })).into_response()
}

/// Toggle the archive_deleted flag for a playlist.
/// When enabled, deleted tracks are still considered active for tag resolution.
async fn toggle_playlist_archive_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use serde_json::json;

    let archive = body
        .get("archiveDeleted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    match crate::db::set_playlist_archive_deleted(&state.db, id, archive).await {
        Ok(()) => Json(json!({"data": {"id": id, "archiveDeleted": archive}})).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

// Get paginated playlists from all services
async fn playlists_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PlaylistsQuery>,
) -> impl IntoResponse {
    use sqlx::QueryBuilder;

    // Default values
    let limit = query.page_size.or(query.limit).unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    let search_term = query.search.clone();
    let service_filter = query.service.clone();

    // Build main query with bind parameters (no string interpolation)
    // LEFT JOIN v_tag_playlist with DISTINCT subquery to avoid cartesian product
    // when multiple tags match the same playlist via case-insensitive name matching.
    let mut main_builder = QueryBuilder::new(
        "SELECT sp.*, \n               COUNT(CASE WHEN spt.deleted_at IS NULL THEN 1 END) as track_count, \n               COUNT(spt.track_id) as total_track_count,\n               vtp.tag_name\n         FROM service_playlists sp\n         LEFT JOIN service_playlist_tracks spt ON sp.id = spt.playlist_id\n         LEFT JOIN (SELECT DISTINCT playlist_id, tag_name FROM v_tag_playlist) vtp ON vtp.playlist_id = sp.id",
    );

    let mut count_builder =
        QueryBuilder::new("SELECT COUNT(DISTINCT sp.id) FROM service_playlists sp");

    let mut has_where = false;

    if let Some(ref service) = service_filter {
        let clause = " WHERE sp.service = ";
        main_builder.push(clause);
        main_builder.push_bind(service.clone());
        count_builder.push(clause);
        count_builder.push_bind(service.clone());
        has_where = true;
    }

    if let Some(ref search) = search_term
        && !search.trim().is_empty()
    {
        let clause = if has_where {
            " AND sp.name LIKE "
        } else {
            " WHERE sp.name LIKE "
        };
        main_builder.push(clause);
        main_builder.push_bind(format!("%{}%", search));
        if has_where {
            count_builder.push(" AND sp.name LIKE ");
        } else {
            count_builder.push(" WHERE sp.name LIKE ");
        }
        count_builder.push_bind(format!("%{}%", search));
        has_where = true;
    }

    // Category filter using v_playlist_tag_category view (category IDs)
    // NOTE: push() treats ? as literal SQL, NOT as a bind placeholder.
    // We push the prefix + paren, then push_bind each value (which emits its own ?),
    // then close the paren. See commit 44ca2b8 for the same fix on PMV filter.
    if let Some(ref cats) = query.categories {
        let cat_ids: Vec<i64> = cats
            .split(',')
            .filter_map(|s| s.trim().parse::<i64>().ok())
            .filter(|id| *id > 0)
            .collect();
        if !cat_ids.is_empty() {
            main_builder
                .push(" LEFT JOIN v_playlist_tag_category vptc ON vptc.playlist_id = sp.id");
            count_builder
                .push(" LEFT JOIN v_playlist_tag_category vptc ON vptc.playlist_id = sp.id");

            let clause = if has_where {
                " AND vptc.category_id IN ("
            } else {
                " WHERE vptc.category_id IN ("
            };
            main_builder.push(clause);
            count_builder.push(clause);
            for (i, id) in cat_ids.iter().enumerate() {
                if i > 0 {
                    main_builder.push(", ");
                    count_builder.push(", ");
                }
                main_builder.push_bind(*id);
                count_builder.push_bind(*id);
            }
            main_builder.push(")");
            count_builder.push(")");
            has_where = true;
        }
    }

    // Subscription filter
    if let Some(subscribed) = query.subscribed {
        let clause = if has_where { " AND " } else { " WHERE " };
        if subscribed {
            let sub = "EXISTS (SELECT 1 FROM playlist_subscriptions ps WHERE ps.service = sp.service AND ps.playlist_id = sp.playlist_id)";
            main_builder.push(format!("{}{}", clause, sub));
            count_builder.push(format!("{}{}", clause, sub));
        } else {
            let sub = "NOT EXISTS (SELECT 1 FROM playlist_subscriptions ps WHERE ps.service = sp.service AND ps.playlist_id = sp.playlist_id)";
            main_builder.push(format!("{}{}", clause, sub));
            count_builder.push(format!("{}{}", clause, sub));
        }
        has_where = true;
    }

    // Stale filter: local track count < remote_unique_count
    if let Some(true) = query.stale {
        let stale_clause = if has_where { " AND " } else { " WHERE " };
        let stale_sub = "(SELECT COUNT(*) FROM service_playlist_tracks spt2 WHERE spt2.playlist_id = sp.id) < sp.remote_unique_count";
        main_builder.push(format!("{}{}", stale_clause, stale_sub));
        count_builder.push(format!("{}{}", stale_clause, stale_sub));
    }

    // Archive filter: archived (archive_deleted = true), active (archive_deleted = false), all
    if let Some(ref archive_filter) = query.archive {
        let clause = if has_where { " AND " } else { " WHERE " };
        match archive_filter.as_str() {
            "archived" => {
                main_builder.push(format!("{}sp.archive_deleted = 1", clause));
                count_builder.push(format!("{}sp.archive_deleted = 1", clause));
            }
            "active" => {
                main_builder.push(format!("{}sp.archive_deleted = 0", clause));
                count_builder.push(format!("{}sp.archive_deleted = 0", clause));
            }
            _ => {} // "all" — no filter
        }
    }

    main_builder.push(" GROUP BY sp.id");

    // Dynamic sort with whitelist + column name mapping
    let sort_col_short = query.sort.as_deref().unwrap_or("name");
    let sort_col = match sort_col_short {
        "track_count" => "track_count",
        "service" => "sp.service",
        "imported_at" => "sp.imported_at",
        "updated_at" => "sp.updated_at",
        _ => "sp.name", // default: sort by name
    };
    let ord = match query.order.as_deref() {
        Some("desc") => "DESC",
        _ => "ASC",
    };
    main_builder.push(format!(" ORDER BY {} {}", sort_col, ord).as_str());

    main_builder.push(" LIMIT ");
    main_builder.push_bind(limit);
    main_builder.push(" OFFSET ");
    main_builder.push_bind(offset);

    // Execute main query
    let playlists = match main_builder
        .build_query_as::<Playlist>()
        .fetch_all(&state.db)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            return internal_error(format!("Failed to fetch playlists: {}", e)).into_response();
        }
    };

    // Execute count query
    let total = match count_builder
        .build_query_scalar::<i64>()
        .fetch_one(&state.db)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            return internal_error(format!("Failed to get total count: {}", e)).into_response();
        }
    };

    // Get deemix status via SQL LEFT JOIN (matches playlist_id in URL)
    let playlist_ids: Vec<String> = playlists.iter().map(|p| p.playlist_id.clone()).collect();
    // Single query with IN clause to find all deemix matches
    let mut deemix_statuses: std::collections::HashMap<String, (Option<String>, Option<i64>)> =
        std::collections::HashMap::new();

    // Build placeholders for IN clause
    let placeholders: Vec<String> = playlist_ids.iter().map(|_| "?".to_string()).collect();
    if !placeholders.is_empty() {
        let sql = format!(
            "SELECT sp.playlist_id, dd.status, dd.id
             FROM service_playlists sp
             LEFT JOIN deemix_downloads dd ON dd.spotify_playlist_url LIKE '%/' || sp.playlist_id
             WHERE sp.playlist_id IN ({})",
            placeholders.join(",")
        );
        let mut q = sqlx::query(&sql);
        for pid in &playlist_ids {
            q = q.bind(pid);
        }
        if let Ok(rows) = q.fetch_all(&state.db).await {
            for row in rows {
                let pid: String = row.try_get("playlist_id").unwrap_or_default();
                let status: Option<String> = row.try_get("status").ok();
                let dd_id: Option<i64> = row.try_get("id").ok();
                deemix_statuses.insert(pid, (status, dd_id));
            }
        }
    }

    // Fallback: check live deemix queue for playlists not found in local table
    let now = chrono::Utc::now().timestamp();
    if let Some(client) = load_deemix_client_from_db(&state.db).await
        && let Ok(remote_queue) = client.get_queue().await
    {
        for p in &playlists {
            if deemix_statuses.contains_key(&p.playlist_id) {
                continue;
            }
            for item in remote_queue.values() {
                if item.id == p.playlist_id {
                    let status = match item.status.as_str() {
                        "completed" | "withErrors" => "completed",
                        "queued" => "queued",
                        "downloading" => "downloading",
                        _ => "queued",
                    };
                    deemix_statuses.insert(p.playlist_id.clone(), (Some(status.to_string()), None));
                    // Backfill into local table for future lookups
                    let url = format!("https://open.spotify.com/playlist/{}", item.id);
                    let _ = sqlx::query(
                        "INSERT OR IGNORE INTO deemix_downloads (spotify_playlist_url, playlist_name, status, track_count_total, track_count_downloaded, created_at, updated_at)
                         VALUES (?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&url)
                    .bind(&item.title)
                    .bind(status)
                    .bind(item.size)
                    .bind(item.downloaded)
                    .bind(now)
                    .bind(now)
                    .execute(&state.db)
                    .await;
                    break;
                }
            }
        }
    }

    // Build enriched playlist objects with deemix status
    let playlists_with_deemix: Vec<serde_json::Value> = playlists
        .iter()
        .map(|p| {
            let (deemix_status, deemix_id) = deemix_statuses
                .get(&p.playlist_id)
                .cloned()
                .unwrap_or((None, None));
            serde_json::json!({
                "id": p.id,
                "service": p.service,
                "playlistId": p.playlist_id,
                "name": p.name,
                "description": p.description,
                "trackCount": p.track_count,
                "localTrackCount": p.track_count,
                "totalTrackCount": p.total_track_count,
                "remoteTrackCount": p.remote_track_count,
                "remoteUniqueCount": p.remote_unique_count,
                "lastFetchedAt": p.last_fetched_at,
                "importedAt": p.imported_at,
                "updatedAt": p.updated_at,
                "metadataJson": p.metadata_json,
                "tagName": p.tag_name,
                "archiveDeleted": p.archive_deleted,
                "deemixStatus": deemix_status,
                "deemixId": deemix_id,
            })
        })
        .collect();

    Json(ApiResponse {
        data: serde_json::json!({
            "playlists": playlists_with_deemix,
            "total": total,
            "limit": limit,
            "offset": offset,
        }),
    })
    .into_response()
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
                    .and_then(|ts| DateTime::from_timestamp(ts, 0)),
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

async fn folders_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FoldersQuery>,
) -> impl IntoResponse {
    match get_folders(&state.db, &query).await {
        Ok(folders) => Json(ApiResponse { data: folders }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn folders_count_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FoldersQuery>,
) -> impl IntoResponse {
    match get_folders_count(&state.db, &query).await {
        Ok(count) => Json(ApiResponse { data: count }).into_response(),
        Err(e) => internal_error(e).into_response(),
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
        Err(e) => internal_error(e).into_response(),
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
                Err(e) => internal_error(e).into_response(),
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Folder not found with id: {}", id),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
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
        Err(e) => internal_error(e).into_response(),
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
        Err(e) => internal_error(e).into_response(),
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
        Err(e) => internal_error(e).into_response(),
    }
}

async fn scan_folder_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    use axum::http::StatusCode;

    // Determine scan mode from query param (default: incremental)
    let scan_mode = match params.get("mode").map(|s| s.as_str()) {
        Some("full") => crate::db::ScanMode::Full,
        _ => crate::db::ScanMode::Incremental { since: None },
    };

    // First check if folder exists
    match get_folder_by_id(&state.db, id).await {
        Ok(Some(_)) => {
            // Folder exists, spawn a background task for folder scanning
            let db = state.db.clone();
            tokio::spawn(async move {
                match scan_folder(&db, id, scan_mode).await {
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
        Err(e) => internal_error(e).into_response(),
    }
}

async fn playlist_detail_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Query playlist by id with track count
    let row = sqlx::query(
        "SELECT sp.*, COUNT(spt.track_id) as track_count
         FROM service_playlists sp
         LEFT JOIN service_playlist_tracks spt ON spt.playlist_id = sp.id
         WHERE sp.id = ?
         GROUP BY sp.id",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(row)) => {
            let playlist = serde_json::json!({
                "id": row.try_get::<i64, _>("id").unwrap_or(0),
                "service": row.try_get::<String, _>("service").unwrap_or_default(),
                "playlistId": row.try_get::<String, _>("playlist_id").unwrap_or_default(),
                "name": row.try_get::<String, _>("name").unwrap_or_default(),
                "description": row.try_get::<Option<String>, _>("description").unwrap_or(None),
                "trackCount": row.try_get::<i64, _>("track_count").unwrap_or(0),
                "importedAt": row.try_get::<i64, _>("imported_at").unwrap_or(0),
                "updatedAt": row.try_get::<i64, _>("updated_at").unwrap_or(0),
            });
            Json(ApiResponse { data: playlist }).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse {
                data: serde_json::json!({"error": "Playlist not found"}),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn playlist_tracks_handler(
    State(_state): State<Arc<AppState>>,
    Path(playlist_id): Path<i64>,
) -> impl IntoResponse {
    // TODO: Implement fetching tracks for playlist — query service_playlist_tracks for given playlist_id
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
    // TODO: Implement adding track to playlist — insert into service_playlist_tracks
    Json(ApiResponse {
        data: format!(
            "Add track to playlist endpoint not implemented for playlist_id: {}",
            playlist_id
        ),
    })
    .into_response()
}

/// Query params for the key comparison endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyComparisonQuery {
    /// Filter by tag name (optional — returns all linked files if omitted)
    tag: Option<String>,
    /// Max results (default 500)
    limit: Option<i64>,
}

/// Compare Traktor vs Spotify BPM/Key for linked files.
///
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

async fn ws_handler() -> impl IntoResponse {
    // TODO: Implement WebSocket handler — for real-time task progress updates to frontend
    "WebSocket endpoint".into_response()
}

async fn get_files(pool: &Pool<Sqlite>, query: &FilesQuery) -> Result<Vec<ApiFile>> {
    let limit = query.page_size.or(query.limit).unwrap_or(100);
    let offset = query.offset.unwrap_or(0);

    // Build dynamic SQL with WHERE clauses for filtering
    let mut sql = String::from("SELECT * FROM files WHERE 1=1");

    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        sql.push_str(" AND (title LIKE ? OR artist LIKE ? OR file_path LIKE ?)");
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
            " AND EXISTS (SELECT 1 FROM v_file_tags ft WHERE ft.file_id = files.id AND ft.is_default = FALSE)",
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
                " AND EXISTS (SELECT 1 FROM v_file_tags ft WHERE ft.file_id = files.id AND LOWER(TRIM(ft.tag_name)) IN ({}))",
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

    // PMV filter — check comment bracket for phase/mood/vibe chars
    if let Some(ref pmv_cats) = query.pmv_categories {
        let cats: Vec<String> = pmv_cats
            .split(',')
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !cats.is_empty() {
            let mut pmv_clauses: Vec<String> = Vec::new();
            for c in &cats {
                let ch = c.chars().next().unwrap();
                pmv_clauses.push(format!(
                    "(SUBSTR(files.comment, 2, 1) = '{c}' OR SUBSTR(files.comment, 3, 1) = '{c}' OR SUBSTR(files.comment, 4, 1) = '{c}')",
                    c = ch
                ));
            }
            sql.push_str(&format!(
                " AND (files.comment IS NOT NULL AND files.comment LIKE '[___]%' AND ({}))",
                pmv_clauses.join(" OR ")
            ));
        }
    } else if let Some(ref pmv_agg) = query.pmv_aggregate {
        match pmv_agg.as_str() {
            "full" => {
                sql.push_str(
                    " AND (files.comment IS NOT NULL AND files.comment LIKE '[___]%' AND \
                     (SUBSTR(files.comment, 2, 1) IN ('P','M','V') OR \
                      SUBSTR(files.comment, 3, 1) IN ('P','M','V') OR \
                      SUBSTR(files.comment, 4, 1) IN ('P','M','V')))",
                );
            }
            "partial" => {
                sql.push_str(
                    " AND (files.comment IS NOT NULL AND files.comment LIKE '[___]%' AND \
                     (SUBSTR(files.comment, 2, 1) IN ('P','M','V') OR \
                      SUBSTR(files.comment, 3, 1) IN ('P','M','V') OR \
                      SUBSTR(files.comment, 4, 1) IN ('P','M','V')))",
                );
            }
            "none" => {
                sql.push_str(
                    " AND (files.comment IS NULL OR files.comment NOT LIKE '[___]%' OR \
                     (SUBSTR(files.comment, 2, 1) NOT IN ('P','M','V') AND \
                      SUBSTR(files.comment, 3, 1) NOT IN ('P','M','V') AND \
                      SUBSTR(files.comment, 4, 1) NOT IN ('P','M','V')))",
                );
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

    apply_sort(
        &mut sql,
        query.sort.as_deref(),
        query.order.as_deref(),
        &[
            "title",
            "artist",
            "bpm",
            "key",
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
        q = q.bind(format!("%{}%", search));
        q = q.bind(format!("%{}%", search));
        q = q.bind(format!("%{}%", search));
    }

    if let Some(bpm_min) = query.bpm_min {
        q = q.bind(bpm_min);
    }

    if let Some(bpm_max) = query.bpm_max {
        q = q.bind(bpm_max);
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
    let mut target_comments: std::collections::HashMap<i64, String> =
        std::collections::HashMap::new();
    if has_comment_filter {
        // Fetch ALL matching files (no LIMIT/OFFSET) to apply comment status filter before pagination
        let all_files = q.fetch_all(pool).await?;

        if all_files.is_empty() {
            return Ok(vec![]);
        }

        // Compute comment_needs_update for all files and cache target comments
        let mut with_status: Vec<(File, bool)> = Vec::with_capacity(all_files.len());
        for file in all_files {
            match compute_target_comment(pool, file.id).await {
                Ok(target_comment) => {
                    let needs_update = file.comment.as_ref() != Some(&target_comment);
                    target_comments.insert(file.id, target_comment);
                    with_status.push((file, needs_update));
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to compute target comment for file {}: {}",
                        file.id,
                        e
                    );
                    with_status.push((file, false));
                }
            }
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

    Ok(api_files)
}

async fn get_files_count(pool: &Pool<Sqlite>, query: &FilesQuery) -> Result<i64> {
    // Build dynamic SQL with the same WHERE clauses as get_files
    let mut sql = String::from("SELECT COUNT(*) as count FROM files WHERE 1=1");

    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        sql.push_str(" AND (title LIKE ? OR artist LIKE ? OR file_path LIKE ?)");
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
            " AND EXISTS (SELECT 1 FROM v_file_tags ft WHERE ft.file_id = files.id AND ft.is_default = FALSE)",
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
                " AND EXISTS (SELECT 1 FROM v_file_tags ft WHERE ft.file_id = files.id AND LOWER(TRIM(ft.tag_name)) IN ({}))",
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

    // PMV filter
    if let Some(ref pmv_cats) = query.pmv_categories {
        let cats: Vec<String> = pmv_cats
            .split(',')
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !cats.is_empty() {
            let mut pmv_clauses: Vec<String> = Vec::new();
            for c in &cats {
                let ch = c.chars().next().unwrap();
                pmv_clauses.push(format!(
                    "(SUBSTR(files.comment, 2, 1) = '{c}' OR SUBSTR(files.comment, 3, 1) = '{c}' OR SUBSTR(files.comment, 4, 1) = '{c}')",
                    c = ch
                ));
            }
            sql.push_str(&format!(
                " AND (files.comment IS NOT NULL AND files.comment LIKE '[___]%' AND ({}))",
                pmv_clauses.join(" OR ")
            ));
        }
    } else if let Some(ref pmv_agg) = query.pmv_aggregate {
        match pmv_agg.as_str() {
            "full" | "partial" => {
                sql.push_str(
                    " AND (files.comment IS NOT NULL AND files.comment LIKE '[___]%' AND \
                     (SUBSTR(files.comment, 2, 1) IN ('P','M','V') OR \
                      SUBSTR(files.comment, 3, 1) IN ('P','M','V') OR \
                      SUBSTR(files.comment, 4, 1) IN ('P','M','V')))",
                );
            }
            "none" => {
                sql.push_str(
                    " AND (files.comment IS NULL OR files.comment NOT LIKE '[___]%' OR \
                     (SUBSTR(files.comment, 2, 1) NOT IN ('P','M','V') AND \
                      SUBSTR(files.comment, 3, 1) NOT IN ('P','M','V') AND \
                      SUBSTR(files.comment, 4, 1) NOT IN ('P','M','V')))",
                );
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

    let mut q = sqlx::query(&sql);

    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        q = q.bind(format!("%{}%", search));
        q = q.bind(format!("%{}%", search));
        q = q.bind(format!("%{}%", search));
    }

    if let Some(bpm_min) = query.bpm_min {
        q = q.bind(bpm_min);
    }

    if let Some(bpm_max) = query.bpm_max {
        q = q.bind(bpm_max);
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
                id_q = id_q
                    .bind(pat.as_str())
                    .bind(pat.as_str())
                    .bind(pat.as_str());
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

            let ids: Vec<i64> = id_q.fetch_all(pool).await?;

            let mut filtered_count: i64 = 0;
            for file_id in ids {
                match compute_target_comment(pool, file_id).await {
                    Ok(target_comment) => {
                        let comment: Option<String> =
                            sqlx::query_scalar("SELECT comment FROM files WHERE id = ?")
                                .bind(file_id)
                                .fetch_optional(pool)
                                .await?
                                .flatten();

                        let needs_update = comment.as_ref() != Some(&target_comment);
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
                    Err(_) => {
                        filtered_count += 1;
                    }
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

    Ok(api_file)
}

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
    let mut sql = if playlists_filter.is_some() {
        "SELECT DISTINCT st.* FROM service_tracks st JOIN service_playlist_tracks spt ON spt.track_id = st.id JOIN service_playlists sp ON sp.id = spt.playlist_id WHERE 1=1".to_string()
    } else if playlist_id_filter.is_some() {
        "SELECT DISTINCT st.* FROM service_tracks st JOIN service_playlist_tracks spt ON spt.track_id = st.id WHERE 1=1"
            .to_string()
    } else {
        "SELECT * FROM service_tracks st WHERE 1=1".to_string()
    };

    if search_pattern.is_some() {
        sql.push_str(" AND (title LIKE ? OR artist LIKE ? OR album LIKE ?)");
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
                " AND EXISTS (SELECT 1 FROM v_file_track_link vft2 JOIN files f2 ON f2.id = vft2.file_id WHERE vft2.track_id = st.id AND f2.file_type IN ({})))",
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
            sql.push_str(&format!(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND vtt.tag_name IN ({}))", placeholders.join(",")));
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
            sql.push_str(&format!(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) IN ({}))", placeholders.join(",")));
        }
    } else if let Some(ref pmv_agg) = pmv_aggregate_filter {
        match pmv_agg.as_str() {
            "full" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) = 'p')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) = 'm')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) = 'v')");
            }
            "partial" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) IN ('p','m','v'))");
            }
            "none" => {
                sql.push_str(" AND NOT EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) IN ('p','m','v'))");
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
        query_builder = query_builder.bind(pattern).bind(pattern).bind(pattern);
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
            api_track.max_added_at = max_added_at_map.remove(&api_track.id);
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

    let mut sql = if playlists_filter.is_some() {
        "SELECT COUNT(DISTINCT st.id) as count FROM service_tracks st JOIN service_playlist_tracks spt ON spt.track_id = st.id JOIN service_playlists sp ON sp.id = spt.playlist_id WHERE 1=1".to_string()
    } else if playlist_id_filter.is_some() {
        "SELECT COUNT(DISTINCT st.id) as count FROM service_tracks st JOIN service_playlist_tracks spt ON spt.track_id = st.id WHERE 1=1"
            .to_string()
    } else {
        "SELECT COUNT(*) as count FROM service_tracks WHERE 1=1".to_string()
    };

    if search_pattern.is_some() {
        sql.push_str(" AND (title LIKE ? OR artist LIKE ? OR album LIKE ?)");
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
                " AND EXISTS (SELECT 1 FROM v_file_track_link vft2 JOIN files f2 ON f2.id = vft2.file_id WHERE vft2.track_id = st.id AND f2.file_type IN ({})))",
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
            sql.push_str(&format!(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND vtt.tag_name IN ({}))", placeholders.join(",")));
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
            sql.push_str(&format!(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) IN ({}))", placeholders.join(",")));
        }
    } else if let Some(ref pmv_agg) = pmv_aggregate_filter {
        match pmv_agg.as_str() {
            "full" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) = 'p')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) = 'm')");
                sql.push_str(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) = 'v')");
            }
            "partial" => {
                sql.push_str(" AND EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) IN ('p','m','v'))");
            }
            "none" => {
                sql.push_str(" AND NOT EXISTS (SELECT 1 FROM v_track_tags vtt WHERE vtt.track_id = st.id AND LOWER(vtt.prefix) IN ('p','m','v'))");
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

    let mut query_builder = sqlx::query(&sql);

    if let Some(ref pattern) = search_pattern {
        query_builder = query_builder.bind(pattern).bind(pattern).bind(pattern);
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

// NOTE: add_explorer_seed was removed — the add_seed_handler endpoint above is the intended
// entry point and needs implementation there instead.

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
    // TODO: Implement bulk tagging — batch assign tags to multiple tracks at once
    Ok(())
}

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
                COALESCE(vfc.file_count, 0) as file_count
         FROM tags t
         LEFT JOIN tag_categories tc ON t.category_id = tc.id
         LEFT JOIN v_tag_file_counts vfc ON vfc.tag_id = t.id
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
async fn get_tag_with_category(pool: &Pool<Sqlite>, tag_id: i64) -> Result<Option<Tag>> {
    let row = sqlx::query("SELECT * FROM v_tags_with_categories WHERE id = ?")
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
        "SELECT * FROM service_config WHERE service IN ('spotify', 'soundcloud', 'youtube', 'deemix')",
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
    let expected_services = ["spotify", "soundcloud", "youtube", "deemix"];

    let mut connections = Vec::new();

    for service_name in &expected_services {
        let configured = match *service_name {
            "spotify" => credentials.is_spotify_configured(),
            "soundcloud" => credentials.is_soundcloud_configured(),
            "youtube" => credentials.is_youtube_configured(),
            // Deemix is configured via web UI (DB), not env vars
            "deemix" => config_map.contains_key("deemix"),
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

async fn get_folders(pool: &Pool<Sqlite>, query: &FoldersQuery) -> Result<Vec<FolderInfo>> {
    let limit = query.page_size.or(query.limit).unwrap_or(100);
    let offset = query.offset.unwrap_or(0);

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

    // Apply search filter (client-side)
    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        let lower = search.to_lowercase();
        folder_infos.retain(|f| f.path.to_lowercase().contains(&lower));
    }

    // Apply sort (client-side)
    if let Some(sort) = query.sort.as_deref() {
        let order = query.order.as_deref().unwrap_or("asc");
        match (sort, order) {
            ("path", "asc") => folder_infos.sort_by(|a, b| a.path.cmp(&b.path)),
            ("path", "desc") => folder_infos.sort_by(|a, b| b.path.cmp(&a.path)),
            ("file_count", "asc") => folder_infos.sort_by(|a, b| a.file_count.cmp(&b.file_count)),
            ("file_count", "desc") => folder_infos.sort_by(|a, b| b.file_count.cmp(&a.file_count)),
            ("watch_enabled", "asc") => {
                folder_infos.sort_by(|a, b| a.watch_enabled.cmp(&b.watch_enabled))
            }
            ("watch_enabled", "desc") => {
                folder_infos.sort_by(|a, b| b.watch_enabled.cmp(&a.watch_enabled))
            }
            ("scan_recursive", "asc") => {
                folder_infos.sort_by(|a, b| a.scan_recursive.cmp(&b.scan_recursive))
            }
            ("scan_recursive", "desc") => {
                folder_infos.sort_by(|a, b| b.scan_recursive.cmp(&a.scan_recursive))
            }
            ("last_scanned", "asc") => {
                folder_infos.sort_by(|a, b| a.last_scanned.cmp(&b.last_scanned))
            }
            ("last_scanned", "desc") => {
                folder_infos.sort_by(|a, b| b.last_scanned.cmp(&a.last_scanned))
            }
            ("max_depth", "asc") => folder_infos.sort_by(|a, b| a.max_depth.cmp(&b.max_depth)),
            ("max_depth", "desc") => folder_infos.sort_by(|a, b| b.max_depth.cmp(&a.max_depth)),
            _ => {}
        }
    }

    // Apply pagination (client-side)
    let paged: Vec<FolderInfo> = folder_infos
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    Ok(paged)
}

pub async fn get_folders_count(pool: &Pool<Sqlite>, query: &FoldersQuery) -> Result<i64> {
    let folders = db_get_folders(pool).await?;

    // Convert to FolderInfo for search filtering
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

    // Apply search filter (client-side)
    if let Some(ref search) = query.search
        && !search.is_empty()
    {
        let lower = search.to_lowercase();
        folder_infos.retain(|f| f.path.to_lowercase().contains(&lower));
    }

    Ok(folder_infos.len() as i64)
}

async fn handle_websocket() {
    // TODO: Implement WebSocket handling — upgrade connection, manage client set, broadcast task updates
}

/// Start a Traktor collection.nml import in the background.
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

/// GET /api/playlists/subscriptions
async fn subscriptions_list_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let subscriptions = match crate::db::list_subscriptions(&state.db).await {
        Ok(subs) => subs,
        Err(e) => {
            return internal_error(format!("Failed to list subscriptions: {}", e)).into_response();
        }
    };

    let statuses: Vec<SubscriptionStatus> = subscriptions
        .into_iter()
        .map(SubscriptionStatus::from)
        .collect();

    Json(ApiResponse { data: statuses }).into_response()
}

/// POST /api/playlists/subscriptions
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeRequest {
    service: String,
    playlist_id: String,
}

async fn subscribe_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SubscribeRequest>,
) -> impl IntoResponse {
    match crate::db::subscribe_to_playlist(
        &state.db,
        &body.service,
        &body.playlist_id,
        None,
    )
    .await
    {
        Ok(id) => Json(ApiResponse {
            data: serde_json::json!({"id": id, "service": body.service, "playlistId": body.playlist_id}),
        })
        .into_response(),
        Err(e) => internal_error(format!("Failed to subscribe: {}", e)).into_response(),
    }
}

/// DELETE /api/playlists/subscriptions/{id}
async fn unsubscribe_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match crate::db::unsubscribe_from_playlist(&state.db, id).await {
        Ok(()) => Json(ApiResponse {
            data: serde_json::json!({"unsubscribed": true}),
        })
        .into_response(),
        Err(e) => internal_error(format!("Failed to unsubscribe: {}", e)).into_response(),
    }
}

/// GET /api/playlists/comment-diff-stats
/// For each subscribed playlist, count of linked files needing comment updates
async fn playlist_comment_diff_stats_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    use std::collections::HashMap;

    // 1. Get all subscribed playlists
    let subscriptions = match crate::db::list_subscriptions(&state.db).await {
        Ok(subs) => subs,
        Err(e) => {
            return internal_error(format!("Failed to list subscriptions: {}", e)).into_response();
        }
    };

    if subscriptions.is_empty() {
        return Json(ApiResponse {
            data: serde_json::json!({ "playlists": [], "total": 0 }),
        })
        .into_response();
    }

    // 2. Get all files
    let files = sqlx::query_as::<_, File>("SELECT * FROM files")
        .fetch_all(&state.db)
        .await;

    let files = match files {
        Ok(f) => f,
        Err(e) => {
            return internal_error(format!("Failed to fetch files: {}", e)).into_response();
        }
    };

    // 3. Build map: playlist_id -> count
    let mut playlist_counts: HashMap<i64, i64> = HashMap::new();

    for file in &files {
        let needs_update = match compute_target_comment(&state.db, file.id).await {
            Ok(target) => file.comment.as_deref().unwrap_or("") != target,
            Err(_) => false,
        };

        if !needs_update {
            continue;
        }

        let playlist_rows = sqlx::query_as::<_, (i64,)>(
            r#"SELECT DISTINCT sp.id
             FROM service_playlists sp
             JOIN service_playlist_tracks spt ON spt.playlist_id = sp.id
             JOIN v_file_track_link v ON v.track_id = spt.track_id
             WHERE v.file_id = ?"#,
        )
        .bind(file.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        for (playlist_id,) in playlist_rows {
            *playlist_counts.entry(playlist_id).or_insert(0) += 1i64;
        }
    }

    // 4. Build response: only subscribed playlists, with names
    let sub_map: HashMap<i64, &crate::db::PlaylistSubscription> = subscriptions
        .iter()
        .filter_map(|s| s.service_playlist_id.map(|id| (id, s)))
        .collect();

    let mut result: Vec<serde_json::Value> = Vec::new();
    for (playlist_id, count) in &playlist_counts {
        if let Some(sub) = sub_map.get(playlist_id) {
            result.push(serde_json::json!({"subscriptionId": sub.id, "playlistId": sub.playlist_id, "playlistName": sub.playlist_name, "service": sub.service, "filesNeedingUpdate": count}));
        }
    }

    // Sort by count descending
    result.sort_by(|a, b| {
        let a_count = a["filesNeedingUpdate"].as_i64().unwrap_or(0);
        let b_count = b["filesNeedingUpdate"].as_i64().unwrap_or(0);
        b_count.cmp(&a_count)
    });

    Json(ApiResponse {
        data: serde_json::json!({"playlists": result, "total": result.len()}),
    })
    .into_response()
}

/// GET /api/files/{id}/similar-tracks
/// Find tracks with semantically similar tags using tag similarity embeddings.
async fn find_tag_similar_tracks_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(query): Query<TracksQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(20).min(100);

    match crate::db::find_tag_similar_tracks(&state.db, id, limit).await {
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

    // 4. Get resolved tag rows from v_file_resolved_tags
    let tag_rows = match sqlx::query_as::<_, (String, String)>(
        "SELECT frt.tag_name, frt.prefix
         FROM v_file_resolved_tags frt
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
    let generated_comment = crate::comment::generate_target_comment(
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
        let stream0_channels: Option<u32> = TokioCommand::new("ffprobe")
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

        let mut cmd = TokioCommand::new("ffmpeg");
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

// ─── Tag Parents / Children Handlers ─────────────────────────────────────────

/// Request body for setting tag parents
#[derive(Debug, Deserialize)]
struct SetTagParentsRequest {
    #[serde(rename = "parentTagIds")]
    parent_tag_ids: Vec<i64>,
}

/// GET /api/tags/{id}/parents
/// Returns the parent tags for a given tag.
async fn tag_parents_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_tag_parents(&state.db, id).await {
        Ok(parents) => {
            // Convert Tag to API Tag with category info
            let mut api_tags: Vec<Tag> = Vec::new();
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
/// Sets (replaces) parent tags for a tag. Only Setlist tags can have parents.
async fn tag_parents_set_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<SetTagParentsRequest>,
) -> impl IntoResponse {
    match set_tag_parents(&state.db, id, &request.parent_tag_ids).await {
        Ok(parents) => {
            let mut api_tags: Vec<Tag> = Vec::new();
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
/// Returns all tags that use this tag as a parent (reverse lookup).
async fn tag_children_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match get_tag_children(&state.db, id).await {
        Ok(children) => {
            let mut api_tags: Vec<Tag> = Vec::new();
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
            linked_only: None,
            unlinked: None,
            non_default_only: None,
            selected_services: None,
            pmv_categories: None,
            pmv_aggregate: None,
            file_types: None,
            comment_statuses: None,
            sort: None,
            order: None,
            page_size: None,
        }
    }
}
