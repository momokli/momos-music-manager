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
use tracing::info;

// ============================================================================
// Dump-specific row structs (match DB columns exactly, including reviewed_at etc.)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpTagCategory {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub prefix: String,
    pub sort_order: i32,
    pub is_default: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpTag {
    pub id: i64,
    pub name: String,
    pub category_id: i64,
    pub sort_order: i64,
    pub created_at: i64,
    pub reviewed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpTagEmbedding {
    pub tag_id: i64,
    pub embedding_b64: String, // base64-encoded BLOB
    pub model_version: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpTagEnergyLevel {
    pub tag_id: i64,
    pub energy_level: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpFolder {
    pub id: i64,
    pub folder_path: String,
    pub active: bool,
    pub scan_recursive: bool,
    pub fixed_extensions: bool,
    pub file_extensions: String,
    pub max_depth: i32,
    pub last_scanned: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpServiceConfig {
    pub id: i64,
    pub service: String,
    pub refresh_token: Option<String>,
    pub metadata_json: Option<String>,
    pub access_token: Option<String>,
    pub token_expiry: Option<i64>,
    pub user_id: Option<String>,
    pub playlist_id: Option<String>,
    pub is_connected: bool,
    pub last_checked: Option<i64>,
    pub last_synced: Option<i64>,
    pub remote_playlists_count: i64,
    pub remote_tracks_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpServiceTrack {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpServicePlaylist {
    pub id: i64,
    pub service: String,
    pub playlist_id: String,
    pub name: String,
    pub description: Option<String>,
    pub metadata_json: Option<String>,
    pub imported_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpServicePlaylistTrack {
    pub playlist_id: i64,
    pub track_id: i64,
    pub position: Option<i32>,
    pub added_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpFile {
    pub id: i64,
    pub file_path: String,
    pub file_hash: String,
    pub file_type: String,
    pub file_size: i64,
    pub last_modified: i64,
    pub isrc: Option<String>,
    pub last_scanned: i64,
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
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub rating: i32,
    pub play_count: i32,
    pub last_played: Option<i64>,
    pub spotify_id: Option<String>,
    pub soundcloud_id: Option<String>,
    pub youtube_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpPlaylistSubscription {
    pub id: i64,
    pub service: String,
    pub playlist_id: String,
    pub service_playlist_id: Option<i64>,
    pub subscribed_at: i64,
    pub last_polled_at: Option<i64>,
    pub poll_interval_secs: i64,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DumpDeemixDownload {
    pub id: i64,
    pub spotify_playlist_url: String,
    pub playlist_name: Option<String>,
    pub status: String,
    pub track_count_total: i64,
    pub track_count_downloaded: i64,
    pub error_message: Option<String>,
    pub created_at: i64,
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
    let dump: DataDump = serde_json::from_str(&json)
        .context("Failed to parse dump JSON - schema may have changed")?;

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
        if seen.insert(table) {
            sqlx::query(&format!("DELETE FROM {}", table))
                .execute(&mut *tx)
                .await?;
        }
    }

    // Insert data in FK-safe order (parents first, children last)
    import_tag_categories(&mut tx, &dump.tag_categories).await?;
    import_tags(&mut tx, &dump.tags).await?;
    import_tag_embeddings(&mut tx, &dump.tag_embeddings).await?;
    import_tag_energy_levels(&mut tx, &dump.tag_energy_levels).await?;
    import_folders(&mut tx, &dump.folders).await?;
    import_service_config(&mut tx, &dump.service_config).await?;
    import_service_tracks(&mut tx, &dump.service_tracks).await?;
    import_service_playlists(&mut tx, &dump.service_playlists).await?;
    import_service_playlist_tracks(&mut tx, &dump.service_playlist_tracks).await?;
    import_files(&mut tx, &dump.files).await?;
    import_playlist_subscriptions(&mut tx, &dump.playlist_subscriptions).await?;
    import_deemix_downloads(&mut tx, &dump.deemix_downloads).await?;

    tx.commit().await?;

    // Re-enable foreign keys AFTER the transaction completes
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(pool)
        .await?;

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

    info!("Import complete: {total} rows restored from {dump_path}");
    Ok(())
}

async fn import_tag_categories(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpTagCategory],
) -> Result<()> {
    for r in rows {
        sqlx::query(
            "INSERT INTO tag_categories (id, name, icon, prefix, sort_order, is_default, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.name).bind(&r.icon).bind(&r.prefix)
        .bind(r.sort_order).bind(r.is_default).bind(r.created_at)
        .execute(&mut **tx).await?;
    }
    if !rows.is_empty() {
        info!("  tag_categories: {} rows", rows.len());
    }
    Ok(())
}

async fn import_tags(tx: &mut sqlx::Transaction<'_, Sqlite>, rows: &[DumpTag]) -> Result<()> {
    for r in rows {
        sqlx::query(
            "INSERT INTO tags (id, name, category_id, sort_order, created_at, reviewed_at) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.name).bind(r.category_id)
        .bind(r.sort_order)
        .bind(r.created_at).bind(r.reviewed_at)
        .execute(&mut **tx).await?;
    }
    if !rows.is_empty() {
        info!("  tags: {} rows", rows.len());
    }
    Ok(())
}

async fn import_tag_embeddings(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpTagEmbedding],
) -> Result<()> {
    for r in rows {
        let blob = BASE64
            .decode(&r.embedding_b64)
            .context("Failed to decode tag_embedding base64")?;
        sqlx::query(
            "INSERT INTO tag_embeddings (tag_id, embedding, model_version, updated_at) VALUES (?, ?, ?, ?)"
        )
        .bind(r.tag_id).bind(&blob).bind(&r.model_version).bind(r.updated_at)
        .execute(&mut **tx).await?;
    }
    if !rows.is_empty() {
        info!("  tag_embeddings: {} rows", rows.len());
    }
    Ok(())
}

async fn import_tag_energy_levels(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpTagEnergyLevel],
) -> Result<()> {
    for r in rows {
        sqlx::query(
            "INSERT INTO tag_energy_levels (tag_id, energy_level, created_at) VALUES (?, ?, ?)",
        )
        .bind(r.tag_id)
        .bind(r.energy_level)
        .bind(r.created_at)
        .execute(&mut **tx)
        .await?;
    }
    if !rows.is_empty() {
        info!("  tag_energy_levels: {} rows", rows.len());
    }
    Ok(())
}

async fn import_folders(tx: &mut sqlx::Transaction<'_, Sqlite>, rows: &[DumpFolder]) -> Result<()> {
    for r in rows {
        sqlx::query(
            "INSERT INTO folders (id, folder_path, active, scan_recursive, fixed_extensions, file_extensions, max_depth, last_scanned, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.folder_path).bind(r.active).bind(r.scan_recursive)
        .bind(r.fixed_extensions).bind(&r.file_extensions).bind(r.max_depth)
        .bind(r.last_scanned).bind(r.created_at).bind(r.updated_at)
        .execute(&mut **tx).await?;
    }
    if !rows.is_empty() {
        info!("  folders: {} rows", rows.len());
    }
    Ok(())
}

async fn import_service_config(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpServiceConfig],
) -> Result<()> {
    for r in rows {
        sqlx::query(
            "INSERT INTO service_config (id, service, refresh_token, metadata_json, access_token, token_expiry, user_id, playlist_id, is_connected, last_checked, last_synced, remote_playlists_count, remote_tracks_count, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.service).bind(&r.refresh_token).bind(&r.metadata_json)
        .bind(&r.access_token)
        .bind(r.token_expiry).bind(&r.user_id).bind(&r.playlist_id).bind(r.is_connected)
        .bind(r.last_checked).bind(r.last_synced).bind(r.remote_playlists_count)
        .bind(r.remote_tracks_count).bind(r.created_at).bind(r.updated_at)
        .execute(&mut **tx).await?;
    }
    if !rows.is_empty() {
        info!("  service_config: {} rows", rows.len());
    }
    Ok(())
}

async fn import_service_tracks(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpServiceTrack],
) -> Result<()> {
    for r in rows {
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist, album, isrc, duration_ms, metadata_json, imported_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.service).bind(&r.service_id).bind(&r.title).bind(&r.artist)
        .bind(&r.album).bind(&r.isrc).bind(r.duration_ms).bind(&r.metadata_json)
        .bind(r.imported_at).bind(r.updated_at)
        .execute(&mut **tx).await?;
    }
    if !rows.is_empty() {
        info!("  service_tracks: {} rows", rows.len());
    }
    Ok(())
}

async fn import_service_playlists(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpServicePlaylist],
) -> Result<()> {
    for r in rows {
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name, description, metadata_json, imported_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.service).bind(&r.playlist_id).bind(&r.name)
        .bind(&r.description).bind(&r.metadata_json).bind(r.imported_at).bind(r.updated_at)
        .execute(&mut **tx).await?;
    }
    if !rows.is_empty() {
        info!("  service_playlists: {} rows", rows.len());
    }
    Ok(())
}

async fn import_service_playlist_tracks(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpServicePlaylistTrack],
) -> Result<()> {
    for r in rows {
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at) VALUES (?, ?, ?, ?)"
        )
        .bind(r.playlist_id).bind(r.track_id).bind(r.position).bind(r.added_at)
        .execute(&mut **tx).await?;
    }
    if !rows.is_empty() {
        info!("  service_playlist_tracks: {} rows", rows.len());
    }
    Ok(())
}

async fn import_playlist_subscriptions(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpPlaylistSubscription],
) -> Result<()> {
    for r in rows {
        sqlx::query(
            "INSERT INTO playlist_subscriptions (id, service, playlist_id, service_playlist_id, subscribed_at, last_polled_at, poll_interval_secs, is_active) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.service).bind(&r.playlist_id).bind(r.service_playlist_id)
        .bind(r.subscribed_at).bind(r.last_polled_at).bind(r.poll_interval_secs).bind(r.is_active)
        .execute(&mut **tx).await?;
    }
    if !rows.is_empty() {
        info!("  playlist_subscriptions: {} rows", rows.len());
    }
    Ok(())
}

async fn import_files(tx: &mut sqlx::Transaction<'_, Sqlite>, rows: &[DumpFile]) -> Result<()> {
    for r in rows {
        sqlx::query(
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
        .execute(&mut **tx).await?;
    }
    if !rows.is_empty() {
        info!("  files: {} rows", rows.len());
    }
    Ok(())
}

async fn import_deemix_downloads(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    rows: &[DumpDeemixDownload],
) -> Result<()> {
    for r in rows {
        sqlx::query(
            "INSERT INTO deemix_downloads (id, spotify_playlist_url, playlist_name, status, track_count_total, track_count_downloaded, error_message, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(r.id).bind(&r.spotify_playlist_url).bind(&r.playlist_name)
        .bind(&r.status).bind(r.track_count_total).bind(r.track_count_downloaded)
        .bind(&r.error_message).bind(r.created_at).bind(r.updated_at)
        .execute(&mut **tx).await?;
    }
    if !rows.is_empty() {
        info!("  deemix_downloads: {} rows", rows.len());
    }
    Ok(())
}
