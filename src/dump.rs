//! Dev data dump/restore for development workflow
//!
//! Exports all database tables as JSON so you can quickly restore data
//! after deleting app.db without re-syncing Spotify or re-scanning files.
//!
//! Usage:
//!   cargo run -- dump          # export to dev-data/dump.json
//!   cargo run -- restore       # import from dev-data/dump.json
//!
//! The dump is a snapshot of all rows with their original IDs.
//! Foreign key relationships are preserved.
//! Binary data (tag_embeddings.embedding) is base64-encoded for JSON compatibility.

use std::io::Write;

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Row, Sqlite};
use tracing::{info, warn};

// ============================================================================
// Dump-specific row structs (match DB columns exactly, including reviewed_at etc.)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpTagCategory {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpTag {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub category_id: i64,
    #[serde(default)]
    pub sort_order: i64,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub reviewed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpTagEmbedding {
    #[serde(default)]
    pub tag_id: i64,
    #[serde(default)]
    pub embedding_b64: String, // base64-encoded BLOB
    #[serde(default)]
    pub model_version: String,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpTagEnergyLevel {
    #[serde(default)]
    pub tag_id: i64,
    #[serde(default)]
    pub energy_level: i64,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpFolder {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub folder_path: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub scan_recursive: bool,
    #[serde(default)]
    pub fixed_extensions: bool,
    #[serde(default)]
    pub file_extensions: String,
    #[serde(default)]
    pub max_depth: i32,
    #[serde(default)]
    pub last_scanned: Option<i64>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpServiceConfig {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub metadata_json: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub token_expiry: Option<i64>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub playlist_id: Option<String>,
    #[serde(default)]
    pub is_connected: bool,
    #[serde(default)]
    pub last_checked: Option<i64>,
    #[serde(default)]
    pub last_synced: Option<i64>,
    #[serde(default)]
    pub remote_playlists_count: i64,
    #[serde(default)]
    pub remote_tracks_count: i64,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpServiceTrack {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub service_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub metadata_json: Option<String>,
    #[serde(default)]
    pub imported_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpServicePlaylist {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub playlist_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub metadata_json: Option<String>,
    #[serde(default)]
    pub imported_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpServicePlaylistTrack {
    #[serde(default)]
    pub playlist_id: i64,
    #[serde(default)]
    pub track_id: i64,
    #[serde(default)]
    pub position: Option<i32>,
    #[serde(default)]
    pub added_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpFile {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub file_path: String,
    #[serde(default)]
    pub file_hash: String,
    #[serde(default)]
    pub file_type: String,
    #[serde(default)]
    pub file_size: i64,
    #[serde(default)]
    pub last_modified: i64,
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub last_scanned: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub album_artist: Option<String>,
    #[serde(default)]
    pub track_number: Option<i32>,
    #[serde(default)]
    pub total_tracks: Option<i32>,
    #[serde(default)]
    pub disc_number: Option<i32>,
    #[serde(default)]
    pub total_discs: Option<i32>,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub composer: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub bitrate: Option<i32>,
    #[serde(default)]
    pub sample_rate: Option<i32>,
    #[serde(default)]
    pub channels: Option<i32>,
    #[serde(default)]
    pub bpm: Option<f64>,
    #[serde(default)]
    pub musical_key: Option<String>,
    #[serde(default)]
    pub rating: i32,
    #[serde(default)]
    pub play_count: i32,
    #[serde(default)]
    pub last_played: Option<i64>,
    #[serde(default)]
    pub spotify_id: Option<String>,
    #[serde(default)]
    pub soundcloud_id: Option<String>,
    #[serde(default)]
    pub youtube_id: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpPlaylistSubscription {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub playlist_id: String,
    #[serde(default)]
    pub service_playlist_id: Option<i64>,
    #[serde(default)]
    pub subscribed_at: i64,
    #[serde(default)]
    pub last_polled_at: Option<i64>,
    #[serde(default)]
    pub poll_interval_secs: i64,
    #[serde(default)]
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpDeemixDownload {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub spotify_playlist_url: String,
    #[serde(default)]
    pub playlist_name: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub track_count_total: i64,
    #[serde(default)]
    pub track_count_downloaded: i64,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

// ============================================================================
// Top-level dump structure
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataDump {
    #[serde(default)]
    pub tag_categories: Vec<DumpTagCategory>,
    #[serde(default)]
    pub tags: Vec<DumpTag>,
    #[serde(default)]
    pub tag_embeddings: Vec<DumpTagEmbedding>,
    #[serde(default)]
    pub tag_energy_levels: Vec<DumpTagEnergyLevel>,
    #[serde(default)]
    pub folders: Vec<DumpFolder>,
    #[serde(default)]
    pub service_config: Vec<DumpServiceConfig>,
    #[serde(default)]
    pub service_tracks: Vec<DumpServiceTrack>,
    #[serde(default)]
    pub service_playlists: Vec<DumpServicePlaylist>,
    #[serde(default)]
    pub service_playlist_tracks: Vec<DumpServicePlaylistTrack>,
    #[serde(default)]
    pub files: Vec<DumpFile>,
    #[serde(default)]
    pub playlist_subscriptions: Vec<DumpPlaylistSubscription>,
    #[serde(default)]
    pub deemix_downloads: Vec<DumpDeemixDownload>,
    pub dumped_at: i64,
}

// ============================================================================
// Export
// ============================================================================

/// Export all database tables to a JSON dump file
pub async fn export_dump(pool: &Pool<Sqlite>, dump_path: &str) -> Result<()> {
    info!("Exporting database dump to {dump_path}");

    let dumped_at = chrono::Utc::now().timestamp();

    let dump = DataDump {
        tag_categories: export_tag_categories(pool).await?,
        tags: export_tags(pool).await?,
        tag_embeddings: export_tag_embeddings(pool).await?,
        tag_energy_levels: export_tag_energy_levels(pool).await?,
        folders: export_folders(pool).await?,
        service_config: export_service_config(pool).await?,
        service_tracks: export_service_tracks(pool).await?,
        service_playlists: export_service_playlists(pool).await?,
        service_playlist_tracks: export_service_playlist_tracks(pool).await?,
        files: export_files(pool).await?,
        playlist_subscriptions: export_playlist_subscriptions(pool).await?,
        deemix_downloads: export_deemix_downloads(pool).await?,
        dumped_at,
    };

    let json = serde_json::to_string_pretty(&dump).context("Failed to serialize dump")?;
    let mut file = std::fs::File::create(dump_path).context("Failed to create dump file")?;
    file.write_all(json.as_bytes())
        .context("Failed to write dump file")?;

    // Print summary
    let total = dump.tag_categories.len()
        + dump.tags.len()
        + dump.tag_embeddings.len()
        + dump.tag_energy_levels.len()
        + dump.folders.len()
        + dump.service_config.len()
        + dump.service_tracks.len()
        + dump.service_playlists.len()
        + dump.service_playlist_tracks.len()
        + dump.files.len()
        + dump.playlist_subscriptions.len()
        + dump.deemix_downloads.len();

    info!("Export complete: {total} rows across 12 tables -> {dump_path}");
    Ok(())
}

async fn export_tag_categories(pool: &Pool<Sqlite>) -> Result<Vec<DumpTagCategory>> {
    let rows = sqlx::query("SELECT * FROM tag_categories ORDER BY id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(DumpTagCategory {
                id: r.get("id"),
                name: r.get("name"),
                icon: r.get("icon"),
                prefix: r.get("prefix"),
                sort_order: r.get("sort_order"),
                is_default: r.get("is_default"),
                created_at: r.get("created_at"),
            })
        })
        .collect()
}

async fn export_tags(pool: &Pool<Sqlite>) -> Result<Vec<DumpTag>> {
    let rows = sqlx::query("SELECT * FROM tags ORDER BY id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(DumpTag {
                id: r.get("id"),
                name: r.get("name"),
                category_id: r.get("category_id"),
                sort_order: r.get("sort_order"),
                created_at: r.get("created_at"),
                reviewed_at: r.get("reviewed_at"),
            })
        })
        .collect()
}

async fn export_tag_embeddings(pool: &Pool<Sqlite>) -> Result<Vec<DumpTagEmbedding>> {
    let rows = sqlx::query("SELECT * FROM tag_embeddings ORDER BY tag_id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            let blob: &[u8] = r.get("embedding");
            Ok(DumpTagEmbedding {
                tag_id: r.get("tag_id"),
                embedding_b64: BASE64.encode(blob),
                model_version: r.get("model_version"),
                updated_at: r.get("updated_at"),
            })
        })
        .collect()
}

async fn export_tag_energy_levels(pool: &Pool<Sqlite>) -> Result<Vec<DumpTagEnergyLevel>> {
    let rows = sqlx::query("SELECT * FROM tag_energy_levels ORDER BY tag_id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(DumpTagEnergyLevel {
                tag_id: r.get("tag_id"),
                energy_level: r.get("energy_level"),
                created_at: r.get("created_at"),
            })
        })
        .collect()
}

async fn export_folders(pool: &Pool<Sqlite>) -> Result<Vec<DumpFolder>> {
    let rows = sqlx::query("SELECT * FROM folders ORDER BY id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(DumpFolder {
                id: r.get("id"),
                folder_path: r.get("folder_path"),
                active: r.get("active"),
                scan_recursive: r.get("scan_recursive"),
                fixed_extensions: r.get("fixed_extensions"),
                file_extensions: r.get("file_extensions"),
                max_depth: r.get("max_depth"),
                last_scanned: r.get("last_scanned"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            })
        })
        .collect()
}

async fn export_service_config(pool: &Pool<Sqlite>) -> Result<Vec<DumpServiceConfig>> {
    let rows = sqlx::query("SELECT * FROM service_config ORDER BY id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(DumpServiceConfig {
                id: r.get("id"),
                service: r.get("service"),
                refresh_token: r.get("refresh_token"),
                metadata_json: r.get("metadata_json"),
                access_token: r.get("access_token"),
                token_expiry: r.get("token_expiry"),
                user_id: r.get("user_id"),
                playlist_id: r.get("playlist_id"),
                is_connected: r.get("is_connected"),
                last_checked: r.get("last_checked"),
                last_synced: r.get("last_synced"),
                remote_playlists_count: r.get("remote_playlists_count"),
                remote_tracks_count: r.get("remote_tracks_count"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            })
        })
        .collect()
}

async fn export_service_tracks(pool: &Pool<Sqlite>) -> Result<Vec<DumpServiceTrack>> {
    let rows = sqlx::query("SELECT * FROM service_tracks ORDER BY id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(DumpServiceTrack {
                id: r.get("id"),
                service: r.get("service"),
                service_id: r.get("service_id"),
                title: r.get("title"),
                artist: r.get("artist"),
                album: r.get("album"),
                isrc: r.get("isrc"),
                duration_ms: r.get("duration_ms"),
                metadata_json: r.get("metadata_json"),
                imported_at: r.get("imported_at"),
                updated_at: r.get("updated_at"),
            })
        })
        .collect()
}

async fn export_service_playlists(pool: &Pool<Sqlite>) -> Result<Vec<DumpServicePlaylist>> {
    let rows = sqlx::query("SELECT * FROM service_playlists ORDER BY id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(DumpServicePlaylist {
                id: r.get("id"),
                service: r.get("service"),
                playlist_id: r.get("playlist_id"),
                name: r.get("name"),
                description: r.get("description"),
                metadata_json: r.get("metadata_json"),
                imported_at: r.get("imported_at"),
                updated_at: r.get("updated_at"),
            })
        })
        .collect()
}

async fn export_service_playlist_tracks(
    pool: &Pool<Sqlite>,
) -> Result<Vec<DumpServicePlaylistTrack>> {
    let rows = sqlx::query("SELECT * FROM service_playlist_tracks ORDER BY playlist_id, track_id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(DumpServicePlaylistTrack {
                playlist_id: r.get("playlist_id"),
                track_id: r.get("track_id"),
                position: r.get("position"),
                added_at: r.get("added_at"),
            })
        })
        .collect()
}

async fn export_files(pool: &Pool<Sqlite>) -> Result<Vec<DumpFile>> {
    let rows = sqlx::query("SELECT * FROM files ORDER BY id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(DumpFile {
                id: r.get("id"),
                file_path: r.get("file_path"),
                file_hash: r.get("file_hash"),
                file_type: r.get("file_type"),
                file_size: r.get("file_size"),
                last_modified: r.get("last_modified"),
                isrc: r.get("isrc"),
                last_scanned: r.get("last_scanned"),
                title: r.get("title"),
                artist: r.get("artist"),
                album: r.get("album"),
                album_artist: r.get("album_artist"),
                track_number: r.get("track_number"),
                total_tracks: r.get("total_tracks"),
                disc_number: r.get("disc_number"),
                total_discs: r.get("total_discs"),
                genre: r.get("genre"),
                year: r.get("year"),
                composer: r.get("composer"),
                comment: r.get("comment"),
                duration_ms: r.get("duration_ms"),
                bitrate: r.get("bitrate"),
                sample_rate: r.get("sample_rate"),
                channels: r.get("channels"),
                bpm: r.get("bpm"),
                musical_key: r.get("musical_key"),
                rating: r.get("rating"),
                play_count: r.get("play_count"),
                last_played: r.get("last_played"),
                spotify_id: r.get("spotify_id"),
                soundcloud_id: r.get("soundcloud_id"),
                youtube_id: r.get("youtube_id"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            })
        })
        .collect()
}

async fn export_deemix_downloads(pool: &Pool<Sqlite>) -> Result<Vec<DumpDeemixDownload>> {
    let rows = sqlx::query("SELECT * FROM deemix_downloads ORDER BY id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(DumpDeemixDownload {
                id: r.get("id"),
                spotify_playlist_url: r.get("spotify_playlist_url"),
                playlist_name: r.get("playlist_name"),
                status: r.get("status"),
                track_count_total: r.get("track_count_total"),
                track_count_downloaded: r.get("track_count_downloaded"),
                error_message: r.get("error_message"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            })
        })
        .collect()
}

async fn export_playlist_subscriptions(
    pool: &Pool<Sqlite>,
) -> Result<Vec<DumpPlaylistSubscription>> {
    let rows = sqlx::query("SELECT * FROM playlist_subscriptions ORDER BY id")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(DumpPlaylistSubscription {
                id: r.get("id"),
                service: r.get("service"),
                playlist_id: r.get("playlist_id"),
                service_playlist_id: r.get("service_playlist_id"),
                subscribed_at: r.get("subscribed_at"),
                last_polled_at: r.get("last_polled_at"),
                poll_interval_secs: r.get("poll_interval_secs"),
                is_active: r.get("is_active"),
            })
        })
        .collect()
}

// ============================================================================
// Import
// ============================================================================

/// Import all tables from a JSON dump file
///
/// Deletes existing data first (in FK-safe order), then inserts all rows
/// with their original IDs to preserve relationships.
pub async fn import_dump(pool: &Pool<Sqlite>, dump_path: &str) -> Result<()> {
    info!("Importing database dump from {dump_path}");

    let json = std::fs::read_to_string(dump_path).context("Failed to read dump file")?;

    // Try strict deserialization first (fast path for compatible schemas),
    // fall back to Value-based parsing if schema drifted.
    let dump: DataDump = match serde_json::from_str(&json) {
        Ok(d) => d,
        Err(e) => {
            info!(
                "Strict deserialization failed ({}), trying resilient schema-agnostic parse...",
                e
            );
            parse_dump_resilient(&json)?
        }
    };

    // Disable foreign keys BEFORE the transaction, because PRAGMA foreign_keys
    // is a no-op inside a transaction in SQLite. This lets us clear tables
    // in any order without FK constraint violations.
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(pool)
        .await?;

    let mut tx = pool.begin().await?;

    // Delete existing data in FK-safe order (children first, parents last)
    let tables_to_clear = [
        "tag_embeddings",
        "tag_energy_levels",
        "service_playlist_tracks",
        "service_tracks",
        "service_playlists",
        "files",
        "service_config",
        "folders",
        "tags",
        "playlist_subscriptions",
        "tag_categories",
        "deemix_downloads",
    ];
    // Remove duplicates and reverse so children are deleted first
    let mut seen = std::collections::HashSet::new();
    for table in tables_to_clear.iter().rev() {
        if seen.insert(table)
            && let Err(e) = sqlx::query(&format!("DELETE FROM {}", table))
                .execute(&mut *tx)
                .await
        {
            warn!("Could not clear table {}: {e} — continuing", table);
        }
    }

    // Insert data in FK-safe order (parents first, children last).
    // Each import is isolated — one table failing won't block the others.
    let mut totals: Vec<(&'static str, usize)> = Vec::new();

    macro_rules! try_import {
        ($label:expr, $expr:expr) => {
            match $expr.await {
                Ok(n) => {
                    if n > 0 {
                        info!("  {}: {} rows", $label, n);
                    }
                    totals.push(($label, n));
                }
                Err(e) => warn!("  {}: SKIPPED — {}", $label, e),
            }
        };
    }

    try_import!(
        "tag_categories",
        import_tag_categories(&mut tx, &dump.tag_categories)
    );
    try_import!("tags", import_tags(&mut tx, &dump.tags));
    try_import!(
        "tag_embeddings",
        import_tag_embeddings(&mut tx, &dump.tag_embeddings)
    );
    try_import!(
        "tag_energy_levels",
        import_tag_energy_levels(&mut tx, &dump.tag_energy_levels)
    );
    try_import!("folders", import_folders(&mut tx, &dump.folders));
    try_import!(
        "service_config",
        import_service_config(&mut tx, &dump.service_config)
    );
    try_import!(
        "service_tracks",
        import_service_tracks(&mut tx, &dump.service_tracks)
    );
    try_import!(
        "service_playlists",
        import_service_playlists(&mut tx, &dump.service_playlists)
    );
    try_import!(
        "service_playlist_tracks",
        import_service_playlist_tracks(&mut tx, &dump.service_playlist_tracks)
    );
    try_import!("files", import_files(&mut tx, &dump.files));
    try_import!(
        "playlist_subscriptions",
        import_playlist_subscriptions(&mut tx, &dump.playlist_subscriptions)
    );
    try_import!(
        "deemix_downloads",
        import_deemix_downloads(&mut tx, &dump.deemix_downloads)
    );

    tx.commit().await?;

    // Re-enable foreign keys AFTER the transaction completes
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(pool)
        .await?;

    let grand_total: usize = totals.iter().map(|(_, n)| n).sum();
    info!("Import complete: {grand_total} rows restored from {dump_path}");
    Ok(())
}

/// Fallback: parse the dump JSON with maximum tolerance for schema drift.
/// Uses serde_json::Value to extract what we can, filling in defaults
/// for missing keys and skipping unknown keys.
fn parse_dump_resilient(json: &str) -> Result<DataDump> {
    use serde_json::Value;

    let root: Value =
        serde_json::from_str(json).context("Failed to parse dump JSON even as Value")?;

    let dumped_at = root.get("dumped_at").and_then(|v| v.as_i64()).unwrap_or(0);

    let mut dump = DataDump {
        tag_categories: Vec::new(),
        tags: Vec::new(),
        tag_embeddings: Vec::new(),
        tag_energy_levels: Vec::new(),
        folders: Vec::new(),
        service_config: Vec::new(),
        service_tracks: Vec::new(),
        service_playlists: Vec::new(),
        service_playlist_tracks: Vec::new(),
        files: Vec::new(),
        playlist_subscriptions: Vec::new(),
        deemix_downloads: Vec::new(),
        dumped_at,
    };

    macro_rules! extract_table {
        ($field:ident, $ty:ty, $key:literal) => {
            if let Some(arr) = root.get($key).and_then(|v| v.as_array()) {
                for (i, item) in arr.iter().enumerate() {
                    match serde_json::from_value::<$ty>(item.clone()) {
                        Ok(row) => dump.$field.push(row),
                        Err(e) => warn!("  {} row {}: skipped — {}", $key, i, e),
                    }
                }
            }
        };
    }

    extract_table!(tag_categories, DumpTagCategory, "tag_categories");
    extract_table!(tags, DumpTag, "tags");
    extract_table!(tag_embeddings, DumpTagEmbedding, "tag_embeddings");
    extract_table!(tag_energy_levels, DumpTagEnergyLevel, "tag_energy_levels");
    extract_table!(folders, DumpFolder, "folders");
    extract_table!(service_config, DumpServiceConfig, "service_config");
    extract_table!(service_tracks, DumpServiceTrack, "service_tracks");
    extract_table!(service_playlists, DumpServicePlaylist, "service_playlists");
    extract_table!(
        service_playlist_tracks,
        DumpServicePlaylistTrack,
        "service_playlist_tracks"
    );
    extract_table!(files, DumpFile, "files");
    extract_table!(
        playlist_subscriptions,
        DumpPlaylistSubscription,
        "playlist_subscriptions"
    );
    extract_table!(deemix_downloads, DumpDeemixDownload, "deemix_downloads");

    Ok(dump)
}

async fn import_tag_categories(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpTagCategory],
) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        match sqlx::query(
            "INSERT INTO tag_categories (id, name, icon, prefix, sort_order, is_default, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.name).bind(&r.icon).bind(&r.prefix)
        .bind(r.sort_order).bind(r.is_default).bind(r.created_at)
        .execute(&mut **tx).await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    tag_categories row id={}: {e}", r.id),
        }
    }
    Ok(count)
}

async fn import_tags(tx: &mut sqlx::Transaction<'_, Sqlite>, rows: &[DumpTag]) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        match sqlx::query(
            "INSERT INTO tags (id, name, category_id, sort_order, created_at, reviewed_at) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.name).bind(r.category_id)
        .bind(r.sort_order)
        .bind(r.created_at).bind(r.reviewed_at)
        .execute(&mut **tx).await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    tags row id={}: {e}", r.id),
        }
    }
    Ok(count)
}

async fn import_tag_embeddings(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpTagEmbedding],
) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        let blob = match BASE64.decode(&r.embedding_b64) {
            Ok(b) => b,
            Err(e) => {
                warn!(
                    "    tag_embeddings tag_id={}: base64 decode failed — {e}",
                    r.tag_id
                );
                continue;
            }
        };
        match sqlx::query(
            "INSERT INTO tag_embeddings (tag_id, embedding, model_version, updated_at) VALUES (?, ?, ?, ?)"
        )
        .bind(r.tag_id).bind(&blob).bind(&r.model_version).bind(r.updated_at)
        .execute(&mut **tx).await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    tag_embeddings tag_id={}: {e}", r.tag_id),
        }
    }
    Ok(count)
}

async fn import_tag_energy_levels(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpTagEnergyLevel],
) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        match sqlx::query(
            "INSERT INTO tag_energy_levels (tag_id, energy_level, created_at) VALUES (?, ?, ?)",
        )
        .bind(r.tag_id)
        .bind(r.energy_level)
        .bind(r.created_at)
        .execute(&mut **tx)
        .await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    tag_energy_levels tag_id={}: {e}", r.tag_id),
        }
    }
    Ok(count)
}

async fn import_folders(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpFolder],
) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        match sqlx::query(
            "INSERT INTO folders (id, folder_path, active, scan_recursive, fixed_extensions, file_extensions, max_depth, last_scanned, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.folder_path).bind(r.active).bind(r.scan_recursive)
        .bind(r.fixed_extensions).bind(&r.file_extensions).bind(r.max_depth)
        .bind(r.last_scanned).bind(r.created_at).bind(r.updated_at)
        .execute(&mut **tx).await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    folders row id={}: {e}", r.id),
        }
    }
    Ok(count)
}

async fn import_service_config(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpServiceConfig],
) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        match sqlx::query(
            "INSERT INTO service_config (id, service, refresh_token, metadata_json, access_token, token_expiry, user_id, playlist_id, is_connected, last_checked, last_synced, remote_playlists_count, remote_tracks_count, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.service).bind(&r.refresh_token).bind(&r.metadata_json)
        .bind(&r.access_token)
        .bind(r.token_expiry).bind(&r.user_id).bind(&r.playlist_id).bind(r.is_connected)
        .bind(r.last_checked).bind(r.last_synced).bind(r.remote_playlists_count)
        .bind(r.remote_tracks_count).bind(r.created_at).bind(r.updated_at)
        .execute(&mut **tx).await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    service_config row id={}: {e}", r.id),
        }
    }
    Ok(count)
}

async fn import_service_tracks(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpServiceTrack],
) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        match sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, album, isrc, duration_ms, metadata_json, imported_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.service).bind(&r.service_id).bind(&r.title).bind(&r.artist)
        .bind(&r.album).bind(&r.isrc).bind(r.duration_ms).bind(&r.metadata_json)
        .bind(r.imported_at).bind(r.updated_at)
        .execute(&mut **tx).await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    service_tracks row id={}: {e}", r.id),
        }
    }
    Ok(count)
}

async fn import_service_playlists(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpServicePlaylist],
) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        match sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name, description, metadata_json, imported_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.service).bind(&r.playlist_id).bind(&r.name)
        .bind(&r.description).bind(&r.metadata_json).bind(r.imported_at).bind(r.updated_at)
        .execute(&mut **tx).await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    service_playlists row id={}: {e}", r.id),
        }
    }
    Ok(count)
}

async fn import_service_playlist_tracks(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpServicePlaylistTrack],
) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        match sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at) VALUES (?, ?, ?, ?)"
        )
        .bind(r.playlist_id).bind(r.track_id).bind(r.position).bind(r.added_at)
        .execute(&mut **tx).await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    service_playlist_tracks playlist_id={} track_id={}: {e}", r.playlist_id, r.track_id),
        }
    }
    Ok(count)
}

async fn import_playlist_subscriptions(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpPlaylistSubscription],
) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        match sqlx::query(
            "INSERT INTO playlist_subscriptions (id, service, playlist_id, service_playlist_id, subscribed_at, last_polled_at, poll_interval_secs, is_active) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.service).bind(&r.playlist_id).bind(r.service_playlist_id)
        .bind(r.subscribed_at).bind(r.last_polled_at).bind(r.poll_interval_secs).bind(r.is_active)
        .execute(&mut **tx).await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    playlist_subscriptions row id={}: {e}", r.id),
        }
    }
    Ok(count)
}

async fn import_files(tx: &mut sqlx::Transaction<'_, Sqlite>, rows: &[DumpFile]) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        match sqlx::query(
            r#"
            INSERT INTO files (
                id, file_path, file_hash, file_type, file_size, last_modified, isrc, last_scanned,
                title, artist, album, album_artist, track_number, total_tracks, disc_number, total_discs,
                genre, year, composer, comment, duration_ms, bitrate, sample_rate, channels,
                bpm, musical_key, rating, play_count, last_played,
                spotify_id, soundcloud_id, youtube_id, created_at, updated_at
            ) VALUES (
                ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?
            )
            "#
        )
        .bind(r.id).bind(&r.file_path).bind(&r.file_hash).bind(&r.file_type)
        .bind(r.file_size).bind(r.last_modified).bind(&r.isrc).bind(r.last_scanned)
        .bind(&r.title).bind(&r.artist).bind(&r.album).bind(&r.album_artist)
        .bind(r.track_number).bind(r.total_tracks).bind(r.disc_number).bind(r.total_discs)
        .bind(&r.genre).bind(r.year).bind(&r.composer).bind(&r.comment)
        .bind(r.duration_ms).bind(r.bitrate).bind(r.sample_rate).bind(r.channels)
        .bind(r.bpm).bind(&r.musical_key).bind(r.rating).bind(r.play_count).bind(r.last_played)
        .bind(&r.spotify_id).bind(&r.soundcloud_id).bind(&r.youtube_id)
        .bind(r.created_at).bind(r.updated_at)
        .execute(&mut **tx).await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    files row id={}: {e}", r.id),
        }
    }
    Ok(count)
}

async fn import_deemix_downloads(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpDeemixDownload],
) -> Result<usize> {
    let mut count = 0;
    for r in rows {
        match sqlx::query(
            "INSERT INTO deemix_downloads (id, spotify_playlist_url, playlist_name, status, track_count_total, track_count_downloaded, error_message, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.spotify_playlist_url).bind(&r.playlist_name)
        .bind(&r.status).bind(r.track_count_total).bind(r.track_count_downloaded)
        .bind(&r.error_message).bind(r.created_at).bind(r.updated_at)
        .execute(&mut **tx).await
        {
            Ok(_) => count += 1,
            Err(e) => warn!("    deemix_downloads row id={}: {e}", r.id),
        }
    }
    Ok(count)
}
