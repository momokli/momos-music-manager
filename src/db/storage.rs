//! Storage/backup/prune-related database queries.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Row, Sqlite};
use tracing::info;

use super::types::*;

// ============================================================================
// Storage Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FileLocation {
    pub id: i64,
    pub file_id: i64,
    pub location_type: String, // 'local' | 'backup'
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

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ServiceConfig {
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

/// Result of a backup discovery scan
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupDiscoveryResult {
    pub files_on_backup: usize,
    pub already_tracked: usize,
    pub newly_discovered: usize,
    pub missing_from_backup: Vec<(i64, String)>,
}

// ============================================================================
// Service Config
// ============================================================================

/// Get the service configuration for a given service name
pub async fn get_service_config(
    pool: &Pool<Sqlite>,
    service: &str,
) -> Result<Option<ServiceConfig>> {
    let config =
        sqlx::query_as::<_, ServiceConfig>("SELECT * FROM service_config WHERE service = ?")
            .bind(service)
            .fetch_optional(pool)
            .await?;
    Ok(config)
}

/// Update or insert a storage settings row in service_config.
/// Uses the 'storage' service key to store JSON metadata (e.g. stem_preferred).
pub async fn update_storage_setting(pool: &Pool<Sqlite>, meta_json: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO service_config (service, metadata_json, is_connected)
         VALUES ('storage', ?, 1)
         ON CONFLICT(service) DO UPDATE SET metadata_json = excluded.metadata_json, updated_at = unixepoch()",
    )
    .bind(meta_json)
    .execute(pool)
    .await?;
    Ok(())
}

/// Update or insert a service configuration
pub async fn update_service_config(
    pool: &Pool<Sqlite>,
    service: &str,
    user_id: Option<&str>,
    playlist_id: Option<&str>,
) -> Result<()> {
    let now = Utc::now().timestamp();

    sqlx::query(
        r#"
        INSERT OR REPLACE INTO service_config
        (service, user_id, playlist_id, updated_at, created_at)
        VALUES (?, ?, ?, ?, COALESCE((SELECT created_at FROM service_config WHERE service = ?), ?))
        "#,
    )
    .bind(service)
    .bind(user_id)
    .bind(playlist_id)
    .bind(now)
    .bind(service)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}

/// Update the connection status for a service
pub async fn update_service_connection_status(
    pool: &Pool<Sqlite>,
    service: &str,
    is_connected: bool,
) -> Result<()> {
    let now = Utc::now().timestamp();

    sqlx::query(
        r#"
        UPDATE service_config
        SET is_connected = ?, last_checked = ?, updated_at = ?
        WHERE service = ?
        "#,
    )
    .bind(is_connected)
    .bind(now)
    .bind(now)
    .bind(service)
    .execute(pool)
    .await?;

    Ok(())
}

/// Update the sync timestamp for a service
pub async fn update_service_sync_timestamp(pool: &Pool<Sqlite>, service: &str) -> Result<()> {
    let now = Utc::now().timestamp();

    sqlx::query(
        r#"
        UPDATE service_config
        SET last_synced = ?, updated_at = ?
        WHERE service = ?
        "#,
    )
    .bind(now)
    .bind(now)
    .bind(service)
    .execute(pool)
    .await?;

    Ok(())
}

/// Update OAuth tokens for a service
pub async fn update_service_tokens(
    pool: &Pool<Sqlite>,
    service: &str,
    refresh_token: Option<&str>,
    access_token: Option<&str>,
    token_expiry: Option<i64>,
) -> Result<()> {
    let now = Utc::now().timestamp();

    sqlx::query(
        r#"
        UPDATE service_config
        SET refresh_token = ?, access_token = ?, token_expiry = ?, is_connected = 1, last_checked = ?, updated_at = ?
        WHERE service = ?
        "#,
    )
    .bind(refresh_token)
    .bind(access_token)
    .bind(token_expiry)
    .bind(now)
    .bind(now)
    .bind(service)
    .execute(pool)
    .await?;

    Ok(())
}

// ============================================================================
// File Locations
// ============================================================================

/// Record or update a file's location (local or backup)
pub async fn set_file_location(
    pool: &Pool<Sqlite>,
    file_id: i64,
    location_type: &str,
    path: &str,
    file_size: i64,
) -> Result<()> {
    let now = Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO file_locations (file_id, location_type, path, file_size, last_verified, created_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(file_id, location_type) DO UPDATE SET
            path = excluded.path,
            file_size = excluded.file_size,
            last_verified = excluded.last_verified",
    )
    .bind(file_id)
    .bind(location_type)
    .bind(path)
    .bind(file_size)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a file location entry (e.g. after local file deletion)
pub async fn remove_file_location(
    pool: &Pool<Sqlite>,
    file_id: i64,
    location_type: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM file_locations WHERE file_id = ? AND location_type = ?")
        .bind(file_id)
        .bind(location_type)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get all locations for a file
pub async fn get_file_locations(pool: &Pool<Sqlite>, file_id: i64) -> Result<Vec<FileLocation>> {
    let locations = sqlx::query_as::<_, FileLocation>(
        "SELECT * FROM file_locations WHERE file_id = ? ORDER BY location_type",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;
    Ok(locations)
}

// ============================================================================
// Backup
// ============================================================================

/// Get files in a folder that have no backup location recorded
pub async fn get_unbacked_up_files(pool: &Pool<Sqlite>, folder_id: i64) -> Result<Vec<File>> {
    let files = sqlx::query_as::<_, File>(
        "SELECT f.* FROM files f
         JOIN folders fol ON fol.folder_path = substr(f.file_path, 1, length(fol.folder_path))
         WHERE fol.id = ?
           AND f.id NOT IN (
               SELECT file_id FROM file_locations WHERE location_type = 'backup'
           )
         ORDER BY f.file_path",
    )
    .bind(folder_id)
    .fetch_all(pool)
    .await?;
    Ok(files)
}

/// Record a successful backup result
pub async fn record_backup_result(
    pool: &Pool<Sqlite>,
    file_id: i64,
    success: bool,
    file_size: i64,
    backup_path: &str,
) -> Result<()> {
    if success {
        set_file_location(pool, file_id, "backup", backup_path, file_size).await?;
    }
    Ok(())
}

/// Discover files that exist on backup (NAS) but not in the local DB.
/// Called by the BackupDiscovery background task.
pub async fn discover_backup_files(
    pool: &Pool<Sqlite>,
    folder_id: i64,
    remote_files: &[String], // full relative paths from backup
    remote_base: &str,
) -> Result<BackupDiscoveryResult> {
    // 1. Get the folder's local path
    let folder_path: String = sqlx::query_scalar("SELECT folder_path FROM folders WHERE id = ?")
        .bind(folder_id)
        .fetch_one(pool)
        .await?;

    let mut result = BackupDiscoveryResult {
        files_on_backup: remote_files.len(),
        already_tracked: 0,
        newly_discovered: 0,
        missing_from_backup: vec![],
    };

    for rel_path in remote_files {
        // Reconstruct local path: folder_path + / + rel_path
        let local_path = format!("{}/{}", folder_path.trim_end_matches('/'), rel_path);

        // Check if this file exists in DB
        let existing: Option<File> = sqlx::query_as("SELECT * FROM files WHERE file_path = ?")
            .bind(&local_path)
            .fetch_optional(pool)
            .await?;

        if let Some(_f) = existing {
            // File exists in DB but may or may not have backup location
            let has_backup: bool = sqlx::query_scalar(
                "SELECT COUNT(*) FROM file_locations WHERE file_id = ? AND location_type = 'backup'",
            )
            .bind(_f.id)
            .fetch_one(pool)
            .await
            .unwrap_or(0)
                > 0;

            if !has_backup {
                // Create backup location
                let remote_path = format!("{}/{}", remote_base.trim_end_matches('/'), rel_path);
                let _ = set_file_location(pool, _f.id, "backup", &remote_path, _f.file_size).await;
                result.already_tracked += 1;
            } else {
                result.already_tracked += 1;
            }
        } else {
            // File on backup but NOT in DB — create backup-only record
            let path = std::path::Path::new(&local_path);
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let file_type = match ext.to_lowercase().as_str() {
                "flac" => "flac",
                "m4a" if filename.ends_with(".stem.m4a") => "stem.m4a",
                "m4a" => "m4a",
                "wav" => "wav",
                "mp3" => "mp3",
                _ => ext,
            };

            let file_size: i64 = 0; // Will be updated on first scan
            let now = Utc::now().timestamp();

            let _ = sqlx::query(
                "INSERT INTO files (file_path, file_hash, file_type, file_size, last_modified, last_scanned, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&local_path)
            .bind(format!("backup-only-{}", file_size)) // sentinel hash
            .bind(file_type)
            .bind(file_size)
            .bind(now) // last_modified = now (unknown)
            .bind(now) // last_scanned = now
            .bind(now)
            .bind(now)
            .execute(pool)
            .await?;

            // Get the new file ID
            if let Ok(Some(new_file)) = crate::db::get_file_by_path(pool, &local_path).await {
                let remote_path = format!("{}/{}", remote_base.trim_end_matches('/'), rel_path);
                let _ =
                    set_file_location(pool, new_file.id, "backup", &remote_path, file_size).await;
                result.newly_discovered += 1;
            }
        }
    }

    Ok(result)
}

/// Clear all backup locations for files in a folder (for re-backup)
pub async fn clear_backup_status(pool: &Pool<Sqlite>, folder_id: i64) -> Result<()> {
    sqlx::query(
        "DELETE FROM file_locations WHERE location_type = 'backup' AND file_id IN (
            SELECT f.id FROM files f
            JOIN folders fol ON fol.folder_path = substr(f.file_path, 1, length(fol.folder_path))
            WHERE fol.id = ?
        )",
    )
    .bind(folder_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ============================================================================
// WAV Source Tracking
// ============================================================================

/// Get subdirectory names under the stems folder (WAV source dirs)
pub async fn get_wav_source_subdirs(pool: &Pool<Sqlite>, folder_id: i64) -> Result<Vec<String>> {
    // Get the folder path, then scan for subdirectories with WAV files
    let folder_path: String = sqlx::query_scalar("SELECT folder_path FROM folders WHERE id = ?")
        .bind(folder_id)
        .fetch_one(pool)
        .await?;

    let mut subdirs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&folder_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Check if it contains .wav files
                if let Ok(dir_entries) = std::fs::read_dir(&path) {
                    let has_wav = dir_entries.flatten().any(|e| {
                        e.path()
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .map(|ext| ext.eq_ignore_ascii_case("wav"))
                            .unwrap_or(false)
                    });
                    if has_wav {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            subdirs.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    Ok(subdirs)
}

/// Link a WAV source file to its parent stem file
pub async fn set_file_source_of(
    pool: &Pool<Sqlite>,
    file_id: i64,
    source_file_id: i64,
) -> Result<()> {
    sqlx::query("UPDATE files SET source_of = ? WHERE id = ?")
        .bind(source_file_id)
        .bind(file_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get all files whose source_of points to a given file (i.e. WAVs for a stem)
pub async fn get_files_by_source(pool: &Pool<Sqlite>, source_file_id: i64) -> Result<Vec<File>> {
    let files = sqlx::query_as::<_, File>("SELECT * FROM files WHERE source_of = ?")
        .bind(source_file_id)
        .fetch_all(pool)
        .await?;
    Ok(files)
}

// ============================================================================
// Prune
// ============================================================================

/// Get all files that can be safely pruned (backed up, metadata-ready,
/// not in any backpack tag, and currently on local disk).
///
/// Two-step approach (file_resolved_tags is already materialized and fast):
/// 1. Get all backed-up non-WAV file IDs (fast, uses indexes)
/// 2. Get all file IDs with backpack tags via file_resolved_tags
/// 3. Subtract in Rust, then fetch details for remaining candidates
pub async fn get_prune_candidates(pool: &Pool<Sqlite>) -> Result<Vec<PruneCandidate>> {
    // Step 1: backed-up file IDs (fast — file_locations has indexes)
    let backed_up: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT fl.file_id FROM file_locations fl
         JOIN files f ON f.id = fl.file_id
         WHERE fl.location_type = 'backup'
           AND (f.file_type != 'wav' OR (f.file_type = 'wav' AND f.source_of IS NOT NULL))
           AND (
               f.file_type = 'wav'
               OR f.bpm IS NOT NULL
               OR (f.comment IS NOT NULL AND f.comment != '')
           )
           AND EXISTS (
               SELECT 1 FROM file_locations fl2
               WHERE fl2.file_id = f.id AND fl2.location_type = 'local'
           )",
    )
    .fetch_all(pool)
    .await?;

    if backed_up.is_empty() {
        return Ok(vec![]);
    }

    // Step 2: file IDs with any backpack tag (simple EXISTS query)
    let backpack: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT frt.file_id FROM file_resolved_tags frt
         JOIN tags t ON t.id = frt.tag_id
         WHERE t.backpack = 1",
    )
    .fetch_all(pool)
    .await?;

    // Build HashSet for fast lookup
    let backpack_set: HashSet<i64> = backpack.into_iter().collect();

    // Step 3: filter in Rust — candidates = backed_up minus backpack
    let candidate_ids: Vec<i64> = backed_up
        .into_iter()
        .filter(|id| !backpack_set.contains(id))
        .collect();

    if candidate_ids.is_empty() {
        return Ok(vec![]);
    }

    // Step 4: fetch full file details for candidates (limit to avoid over-fetching)
    // Build IN clause dynamically — SQLx doesn't support arrays, so use placeholders
    let placeholders: Vec<String> = candidate_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT f.id, f.file_path, f.file_type, f.file_size,
                COALESCE(f.title, '') as title, COALESCE(f.artist, '') as artist,
                f.isrc, fl.path as backup_path
         FROM files f
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'backup'
         WHERE f.id IN ({})
         ORDER BY f.file_type, f.file_path",
        placeholders.join(",")
    );

    let mut query = sqlx::query(&sql);
    for id in &candidate_ids {
        query = query.bind(id);
    }
    let rows = query.fetch_all(pool).await?;

    let mut candidates = Vec::new();
    for row in rows {
        let file_id: i64 = row.try_get("id")?;
        let ft: String = row.try_get("file_type")?;
        let isrc: Option<String> = row.try_get("isrc")?;
        let reason = if ft == "wav" {
            "wav_backed_up".to_string()
        } else {
            "not_followed".to_string()
        };

        // Compute has_stem_variant:
        // - For non-stem non-WAV files: check if same-ISRC stem.m4a exists in DB
        // - For stems themselves: check if they have WAV sources linked
        // - For WAVs: always true (they are stem variants themselves)
        let has_stem_variant = if ft == "wav" {
            true
        } else if ft != "stem.m4a" {
            // Non-WAV, non-stem: check for same-ISRC stem.m4a
            if let Some(ref isrc_val) = isrc {
                let count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM files WHERE isrc = ? AND file_type = 'stem.m4a'",
                )
                .bind(isrc_val)
                .fetch_one(pool)
                .await
                .unwrap_or(0);
                count > 0
            } else {
                false
            }
        } else {
            // Stems: check if they have WAV sources linked
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM files WHERE source_of = ? AND file_type = 'wav'",
            )
            .bind(file_id)
            .fetch_one(pool)
            .await
            .unwrap_or(0);
            count > 0
        };

        candidates.push(PruneCandidate {
            file_id,
            file_path: row.try_get("file_path")?,
            file_type: row.try_get("file_type")?,
            file_size: row.try_get("file_size")?,
            title: row.try_get("title")?,
            artist: row.try_get("artist")?,
            isrc,
            reason,
            backup_path: row.try_get("backup_path")?,
            has_stem_variant,
        });
    }

    Ok(candidates)
}

/// Delete a local file and remove its 'local' file_location entry.
/// Returns true if the file was actually deleted from disk.
pub async fn delete_local_file_by_id(pool: &Pool<Sqlite>, file_id: i64) -> Result<bool> {
    // Get file path
    let file_path: Option<String> = sqlx::query_scalar("SELECT file_path FROM files WHERE id = ?")
        .bind(file_id)
        .fetch_optional(pool)
        .await?;

    if let Some(path) = file_path {
        let path_ref = std::path::Path::new(&path);
        if path_ref.exists() {
            std::fs::remove_file(path_ref)?;
            info!("Deleted local file: {}", path);
        }
        // Remove local location record
        sqlx::query("DELETE FROM file_locations WHERE file_id = ? AND location_type = 'local'")
            .bind(file_id)
            .execute(pool)
            .await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

// ============================================================================
// Storage Status
// ============================================================================

/// Get aggregate storage statistics
pub async fn get_storage_status(pool: &Pool<Sqlite>) -> Result<StorageStatus> {
    // Local counts: files actually on disk (from file_locations)
    let local_file_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT file_id) FROM file_locations WHERE location_type = 'local'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let local_size_bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(f.file_size), 0) FROM files f \
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'local'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // Tracked counts: ALL files in the DB (including backup-only)
    let tracked_file_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let tracked_size_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(file_size), 0) FROM files")
            .fetch_one(pool)
            .await
            .unwrap_or(0);

    // Per-type local counts & sizes (from file_locations)
    let local_stems: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT f.id) FROM files f \
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'local' \
         WHERE f.file_type = 'stem.m4a'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let local_flacs: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT f.id) FROM files f \
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'local' \
         WHERE f.file_type = 'flac'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let local_mp3s: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT f.id) FROM files f \
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'local' \
         WHERE f.file_type = 'mp3'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let local_wavs: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT f.id) FROM files f \
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'local' \
         WHERE f.file_type = 'wav'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let local_other: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT f.id) FROM files f \
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'local' \
         WHERE f.file_type NOT IN ('stem.m4a','flac','mp3','wav')",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // Per-type local sizes (from file_locations, sum sizes)
    let local_stems_size: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(f.file_size), 0) FROM files f \
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'local' \
         WHERE f.file_type = 'stem.m4a'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let local_flacs_size: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(f.file_size), 0) FROM files f \
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'local' \
         WHERE f.file_type = 'flac'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let local_wavs_size: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(f.file_size), 0) FROM files f \
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'local' \
         WHERE f.file_type = 'wav'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let local_mp3s_size: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(f.file_size), 0) FROM files f \
         JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'local' \
         WHERE f.file_type = 'mp3'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let backup_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT file_id) FROM file_locations WHERE location_type = 'backup'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let wav_source_dirs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_type = 'wav' AND source_of IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // Prune candidates count & size
    let candidates = get_prune_candidates(pool).await?;
    let prune_candidate_count = candidates.len() as i64;
    let prune_candidate_bytes = candidates.iter().map(|c| c.file_size).sum();

    let wav_indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE file_type = 'wav'")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    let wav_backed_up: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT fl.file_id) FROM file_locations fl
         JOIN files f ON f.id = fl.file_id
         WHERE f.file_type = 'wav' AND fl.location_type = 'backup'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    Ok(StorageStatus {
        local_file_count,
        tracked_file_count,
        local_size_bytes,
        tracked_size_bytes,
        local_stems,
        local_flacs,
        local_mp3s,
        local_wavs,
        local_other,
        local_stems_size,
        local_flacs_size,
        local_wavs_size,
        local_mp3s_size,
        backup_count,
        wav_source_dirs,
        prune_candidate_count,
        prune_candidate_bytes,
        wav_indexed,
        wav_backed_up,
    })
}
