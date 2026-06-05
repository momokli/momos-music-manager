//! Database types — structs and enums matching the SQL schema.
//!
//! Types unique to a single domain (e.g. CurationTag, ServiceConfig*)
//! are defined in their domain's file and re-exported via mod.rs.
//! This file holds only cross-cutting types.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

// ============================================================================
// Scan Mode
// ============================================================================

#[derive(Debug, Clone)]
pub enum ScanMode {
    Full,
    Incremental { since: Option<i64> },
}

// ============================================================================
// Database Models (8-table schema) — core entity types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TagCategory {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub prefix: String,
    pub sort_order: i32,
    pub is_default: bool,
    pub tag_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub category_id: i64,
    pub sort_order: i64,
    pub created_at: i64,
    pub backpack: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ServiceTrack {
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
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ServicePlaylist {
    pub id: i64,
    pub service: String,
    pub playlist_id: String,
    pub name: String,
    pub description: Option<String>,
    pub metadata_json: Option<String>,
    pub imported_at: i64,
    pub updated_at: i64,
    pub last_fetched_at: Option<i64>,
    pub remote_track_count: i64,
    pub remote_unique_count: i64,
    pub archive_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ServicePlaylistTrack {
    pub playlist_id: i64,
    pub track_id: i64,
    pub position: Option<i32>,
    pub added_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct File {
    pub id: i64,
    pub file_path: String,
    pub file_hash: String,
    pub file_type: String,
    pub file_size: i64,
    pub last_modified: i64,
    pub isrc: Option<String>,
    pub last_scanned: i64,

    // Audio metadata
    pub title: Option<String>,
    pub artist: Option<String>,
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

    // BPM/Key from Traktor/EXIF
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,

    // Traktor stats
    pub rating: i32,
    pub play_count: i32,
    pub last_played: Option<i64>,

    // Direct service IDs
    pub spotify_id: Option<String>,
    pub soundcloud_id: Option<String>,
    pub youtube_id: Option<String>,

    // Source WAV linking
    pub source_of: Option<i64>,
    pub stem_type: Option<String>,
    pub last_verified_local: Option<i64>,

    // Timestamps
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Folder {
    pub id: i64,
    pub folder_path: String,
    pub active: bool,
    pub scan_recursive: bool,
    pub fixed_extensions: bool,
    pub file_extensions: String,
    pub max_depth: i32,
    pub last_scanned: Option<i64>,
    pub scan_sources: bool,
    pub backup_path: Option<String>,
    pub auto_backup: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

// ============================================================================
// File Lifecycle Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FileLocation {
    pub id: i64,
    pub file_id: i64,
    pub location_type: String,
    pub path: String,
    pub file_size: Option<i64>,
    pub last_verified: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneCandidate {
    pub file_id: i64,
    pub file_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub title: String,
    pub artist: String,
    pub isrc: Option<String>,
    pub reason: String,
    pub backup_path: Option<String>,
    pub has_stem_variant: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatus {
    pub local_file_count: i64,
    pub tracked_file_count: i64,
    pub local_size_bytes: i64,
    pub tracked_size_bytes: i64,
    pub local_stems: i64,
    pub local_flacs: i64,
    pub local_mp3s: i64,
    pub local_wavs: i64,
    pub local_other: i64,
    pub local_stems_size: i64,
    pub local_flacs_size: i64,
    pub local_wavs_size: i64,
    pub local_mp3s_size: i64,
    pub backup_count: i64,
    pub wav_source_dirs: i64,
    pub prune_candidate_count: i64,
    pub prune_candidate_bytes: i64,
    pub wav_indexed: i64,
    pub wav_backed_up: i64,
}

// ============================================================================
// Query/Response Types — used by the API layer
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct KeyComparisonRow {
    pub file_id: i64,
    pub file_path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub traktor_bpm: Option<f64>,
    pub traktor_key: Option<String>,
    pub spotify_bpm: Option<f64>,
    pub spotify_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyComparisonSummary {
    pub total: usize,
    pub bpm_match: usize,
    pub key_match: usize,
    pub bpm_mismatch: usize,
    pub key_mismatch: usize,
    pub no_spotify_data: usize,
    pub match_percentage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackedPlaylistInfo {
    pub id: Option<i64>,
    pub service_playlist_id: i64,
    pub name: String,
    pub playlist_id: String,
}
