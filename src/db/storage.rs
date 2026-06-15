//! Storage/backup/prune-related database queries.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Row, Sqlite};
use tracing::info;
use unicode_normalization::UnicodeNormalization;

use super::types::*;

// FileLocation is defined canonically in types.rs.

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

/// A file that should be pulled from backup to local disk.
/// Used by the backpack sync system to ensure offline availability.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullCandidate {
    pub file_id: i64,
    pub local_path: String,
    pub backup_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub title: String,
    pub artist: String,
    pub isrc: Option<String>,
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

/// Size statistics for backpack-tagged files — used by the Backpack page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackpackSizeStats {
    pub tag_count: i64,
    pub track_count: i64,
    pub local_bytes: i64,
    pub target_bytes: i64,
    pub needs_pull_bytes: i64,
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
        // Normalize to NFC to match DB (NAS stores NFD, DB stores NFC)
        let rel_path: String = rel_path.nfc().collect();
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

/// Remove file_locations.backup entries for files that no longer exist on NAS.
///
/// `remote_files` should be paths relative to the backup root (e.g. "Artist - Title.flac"),
/// already normalized to NFC by the caller.
/// Returns the number of stale entries removed.
pub async fn cleanup_stale_backup_entries(
    pool: &Pool<Sqlite>,
    folder_id: i64,
    remote_files: &[String],
) -> Result<usize> {
    // Build NFC-normalized set of NAS filenames (just the filename, not full path)
    let nas_filenames: std::collections::HashSet<String> = remote_files
        .iter()
        .map(|p| {
            std::path::Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .nfc()
                .collect::<String>()
        })
        .collect();

    // Get all files with backup entries for this folder
    #[derive(sqlx::FromRow)]
    struct BackupEntry {
        file_id: i64,
        path: String,
    }

    let backed_up: Vec<BackupEntry> = sqlx::query_as(
        "SELECT fl.file_id, fl.path FROM file_locations fl
         JOIN files f ON f.id = fl.file_id
         JOIN folders fol ON fol.folder_path = substr(f.file_path, 1, length(fol.folder_path))
         WHERE fl.location_type = 'backup' AND fol.id = ?",
    )
    .bind(folder_id)
    .fetch_all(pool)
    .await?;

    let mut removed = 0usize;
    for entry in &backed_up {
        let filename = std::path::Path::new(&entry.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if !nas_filenames.contains(&filename) {
            sqlx::query(
                "DELETE FROM file_locations WHERE file_id = ? AND location_type = 'backup'",
            )
            .bind(entry.file_id)
            .execute(pool)
            .await?;
            removed += 1;
        }
    }

    Ok(removed)
}

// ============================================================================
// Backup Verification
// ============================================================================

/// Get the oldest backup records for a folder, limited by sample_size.
/// Returns (file_id, file_locations.id, file_size, remote_path).
pub async fn get_verify_backup_candidates(
    pool: &Pool<Sqlite>,
    folder_id: i64,
    sample_size: usize,
) -> Result<Vec<(i64, i64, i64, String)>> {
    let records = sqlx::query_as::<_, (i64, i64, i64, String)>(
        "SELECT fl.file_id, fl.id, fl.file_size, fl.path
         FROM file_locations fl
         JOIN files f ON f.id = fl.file_id
         JOIN folders fol ON fol.folder_path = substr(f.file_path, 1, length(fol.folder_path))
         WHERE fl.location_type = 'backup' AND fol.id = ?
         ORDER BY fl.last_verified ASC NULLS FIRST, fl.created_at ASC
         LIMIT ?",
    )
    .bind(folder_id)
    .bind(sample_size as i64)
    .fetch_all(pool)
    .await?;
    Ok(records)
}

/// Verify a sample of backup records for a folder using SSH.
/// Returns (verified, missing, errors).
pub async fn verify_backup_records(
    pool: &Pool<Sqlite>,
    folder_id: i64,
    engine: &crate::backup::BackupEngine,
    sample_size: usize,
) -> Result<(usize, usize, usize)> {
    let candidates = get_verify_backup_candidates(pool, folder_id, sample_size).await?;
    let mut verified = 0usize;
    let mut missing = 0usize;
    let mut errors = 0usize;

    let mut backfilled = 0usize;
    for (file_id, loc_id, file_size, remote_path) in &candidates {
        if *file_size <= 0 {
            // Zero-size record — can't verify by comparison.
            // Resolve by checking remote directly: if the file exists, update
            // the record with the real size. If not, remove the stale record.
            match engine.remote_file_size(remote_path).await {
                Ok(Some(actual_size)) if actual_size > 0 => {
                    let _ = sqlx::query(
                        "UPDATE file_locations SET file_size = ?, last_verified = ? WHERE id = ?",
                    )
                    .bind(actual_size)
                    .bind(Utc::now().timestamp())
                    .bind(loc_id)
                    .execute(pool)
                    .await;
                    backfilled += 1;
                }
                Ok(_) => {
                    tracing::warn!(
                        "Backup verification: file #{} at {} not found on NAS (zero-size record), removing",
                        file_id,
                        remote_path
                    );
                    let _ = sqlx::query(
                        "DELETE FROM file_locations WHERE id = ? AND location_type = 'backup'",
                    )
                    .bind(loc_id)
                    .execute(pool)
                    .await;
                    missing += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        "Backup verification: failed to resolve zero-size file #{}: {}",
                        file_id,
                        e
                    );
                    errors += 1;
                }
            }
            continue;
        }
        match engine.verify_file(remote_path, *file_size).await {
            Ok(true) => {
                let _ = sqlx::query("UPDATE file_locations SET last_verified = ? WHERE id = ?")
                    .bind(Utc::now().timestamp())
                    .bind(loc_id)
                    .execute(pool)
                    .await;
                verified += 1;
            }
            Ok(false) => {
                tracing::warn!(
                    "Backup verification: file #{} at {} not found on NAS, removing backup record",
                    file_id,
                    remote_path
                );
                let _ = sqlx::query(
                    "DELETE FROM file_locations WHERE id = ? AND location_type = 'backup'",
                )
                .bind(loc_id)
                .execute(pool)
                .await;
                missing += 1;
            }
            Err(e) => {
                tracing::warn!(
                    "Backup verification: failed to verify file #{}: {}",
                    file_id,
                    e
                );
                errors += 1;
            }
        }
    }
    if backfilled > 0 {
        tracing::info!(
            "Backup verification: backfilled {} zero-size records with actual remote sizes",
            backfilled
        );
    }

    Ok((verified, missing, errors))
}

// ============================================================================
// Backup Size Backfill
// ============================================================================

/// A record needing size backfill.
#[derive(Debug, FromRow)]
pub struct BackfillRecord {
    pub file_id: i64,
    pub location_id: i64,
    pub remote_path: String,
}

/// Get file_locations.backup records that have file_size=0 or NULL.
pub async fn get_records_needing_backfill(pool: &Pool<Sqlite>) -> Result<Vec<BackfillRecord>> {
    let records = sqlx::query_as::<_, BackfillRecord>(
        "SELECT fl.file_id, fl.id AS location_id, fl.path AS remote_path
         FROM file_locations fl
         WHERE fl.location_type = 'backup' AND (fl.file_size = 0 OR fl.file_size IS NULL)
         ORDER BY fl.file_id",
    )
    .fetch_all(pool)
    .await?;
    Ok(records)
}

/// For backup records with file_size=0, attempt to get the actual
/// remote file size via SSH and update the record.
/// Returns (total_checked, fixed, failed).
pub async fn backfill_backup_sizes(
    pool: &Pool<Sqlite>,
    engine: &crate::backup::BackupEngine,
) -> Result<(usize, usize, usize)> {
    let records = get_records_needing_backfill(pool).await?;
    let total = records.len();
    let mut fixed = 0usize;
    let mut failed = 0usize;

    for record in &records {
        match engine.remote_file_size(&record.remote_path).await {
            Ok(Some(size)) if size > 0 => {
                let _ = sqlx::query(
                    "UPDATE file_locations SET file_size = ?, last_verified = ? WHERE id = ?",
                )
                .bind(size)
                .bind(Utc::now().timestamp())
                .bind(record.location_id)
                .execute(pool)
                .await;
                fixed += 1;
            }
            Ok(_) => {
                tracing::warn!(
                    "Backfill: file #{} remote size is 0 or missing",
                    record.file_id
                );
                failed += 1;
            }
            Err(e) => {
                tracing::warn!("Backfill: failed to stat file #{}: {}", record.file_id, e);
                failed += 1;
            }
        }
    }

    tracing::info!(
        "Backfill complete: {}/{} fixed, {}/{} failed",
        fixed,
        total,
        failed,
        total
    );
    Ok((total, fixed, failed))
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

/// Get all files that can be safely pruned (backed up, not in any backpack
/// tag, and currently on local disk).
///
/// A file is safe to delete if: backed up + local + not in backpack.
/// No other gates — the user trusts backup.
///
/// Two-step approach (file_resolved_tags is already materialized and fast):
/// 1. Get all backed-up+local file IDs (fast, uses indexes)
/// 2. Get all file IDs with backpack tags via file_resolved_tags
/// 3. Subtract in Rust, then fetch details for remaining candidates
pub async fn get_prune_candidates(pool: &Pool<Sqlite>) -> Result<Vec<PruneCandidate>> {
    // Step 1: backed-up file IDs (fast — file_locations has indexes)
    let backed_up: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT fl.file_id FROM file_locations fl
         JOIN files f ON f.id = fl.file_id
         WHERE fl.location_type = 'backup'
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
    let backpack_set: HashSet<i64> = backpack.iter().copied().collect();

    tracing::info!(
        "Prune: {} backed-up+local files, {} backpack-protected",
        backed_up.len(),
        backpack.len()
    );

    // Step 3: filter in Rust — candidates = backed_up minus backpack
    let candidate_ids: Vec<i64> = backed_up
        .into_iter()
        .filter(|id| !backpack_set.contains(id))
        .collect();

    if candidate_ids.is_empty() {
        tracing::info!(
            "Prune: 0 candidates after removing {} backpack-protected files",
            backpack.len()
        );
        return Ok(vec![]);
    }

    tracing::info!(
        "Prune: {} candidates after removing backpack",
        candidate_ids.len()
    );

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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    /// Create an in-memory SQLite DB with the minimal schema for storage tests.
    async fn test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

        // files: core file table (just the columns we need for storage tests)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL UNIQUE,
                file_hash TEXT NOT NULL DEFAULT '',
                file_type TEXT NOT NULL,
                file_size INTEGER NOT NULL DEFAULT 0,
                last_modified INTEGER NOT NULL DEFAULT 0,
                last_scanned INTEGER NOT NULL DEFAULT 0,
                isrc TEXT,
                title TEXT,
                artist TEXT,
                album TEXT,
                album_artist TEXT,
                track_number INTEGER,
                total_tracks INTEGER,
                disc_number INTEGER,
                total_discs INTEGER,
                genre TEXT,
                year INTEGER,
                composer TEXT,
                comment TEXT,
                duration_ms INTEGER,
                bitrate INTEGER,
                sample_rate INTEGER,
                channels INTEGER,
                bpm REAL,
                musical_key TEXT,
                rating INTEGER NOT NULL DEFAULT 0,
                play_count INTEGER NOT NULL DEFAULT 0,
                last_played INTEGER,
                spotify_id TEXT,
                soundcloud_id TEXT,
                youtube_id TEXT,
                source_of INTEGER,
                stem_type TEXT,
                last_verified_local INTEGER,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // file_locations: tracks where a file physically exists
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS file_locations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                location_type TEXT NOT NULL CHECK (location_type IN ('local', 'backup')),
                path TEXT NOT NULL,
                file_size INTEGER,
                last_verified INTEGER,
                created_at INTEGER DEFAULT (unixepoch()),
                UNIQUE(file_id, location_type)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // tags: minimal for backpack checking
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE COLLATE NOCASE,
                category_id INTEGER NOT NULL DEFAULT 1,
                sort_order INTEGER DEFAULT 0,
                created_at INTEGER DEFAULT (unixepoch()),
                backpack INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // file_resolved_tags: materialized tag resolution
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS file_resolved_tags (
                file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                tag_id INTEGER NOT NULL,
                tag_name TEXT NOT NULL,
                category_id INTEGER NOT NULL,
                category_name TEXT NOT NULL,
                prefix TEXT NOT NULL,
                sort_order INTEGER DEFAULT 0,
                created_at INTEGER,
                is_default INTEGER DEFAULT 0,
                PRIMARY KEY (file_id, tag_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // service_config: for service config tests
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS service_config (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service TEXT NOT NULL UNIQUE,
                refresh_token TEXT,
                metadata_json TEXT,
                access_token TEXT,
                token_expiry INTEGER,
                user_id TEXT,
                playlist_id TEXT,
                is_connected INTEGER NOT NULL DEFAULT 0,
                last_checked INTEGER,
                last_synced INTEGER,
                remote_playlists_count INTEGER NOT NULL DEFAULT 0,
                remote_tracks_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // folders: for unbacked-up files queries
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS folders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                folder_path TEXT NOT NULL UNIQUE,
                active INTEGER NOT NULL DEFAULT 1,
                scan_recursive INTEGER NOT NULL DEFAULT 1,
                fixed_extensions INTEGER NOT NULL DEFAULT 0,
                file_extensions TEXT NOT NULL DEFAULT '',
                max_depth INTEGER NOT NULL DEFAULT 10,
                last_scanned INTEGER,
                scan_sources INTEGER NOT NULL DEFAULT 0,
                backup_path TEXT,
                auto_backup INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    // Helper: insert a file row, returning its id
    async fn insert_file(
        pool: &SqlitePool,
        id: i64,
        path: &str,
        file_type: &str,
        size: i64,
        isrc: Option<&str>,
        bpm: Option<f64>,
        comment: Option<&str>,
    ) -> i64 {
        let comment_val = comment.map(|s| s.to_string());
        sqlx::query(
            "INSERT INTO files (id, file_path, file_hash, file_type, file_size, last_modified, last_scanned, isrc, title, artist, bpm, comment, created_at, updated_at)
             VALUES (?, ?, '', ?, ?, 0, 0, ?, ?, ?, ?, ?, 0, 0)",
        )
        .bind(id)
        .bind(path)
        .bind(file_type)
        .bind(size)
        .bind(isrc)
        .bind(path) // title = path as placeholder
        .bind("Test Artist")
        .bind(bpm)
        .bind(comment_val)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    // ── File Location CRUD ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_file_location_create_and_get() {
        let pool = test_db().await;
        insert_file(
            &pool,
            1,
            "/test/song.flac",
            "flac",
            12345,
            None,
            Some(120.0),
            None,
        )
        .await;

        set_file_location(&pool, 1, "local", "/test/song.flac", 12345)
            .await
            .unwrap();

        let locations = get_file_locations(&pool, 1).await.unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].location_type, "local");
        assert_eq!(locations[0].file_size, Some(12345));
    }

    #[tokio::test]
    async fn test_file_location_upsert_updates_path() {
        let pool = test_db().await;
        insert_file(&pool, 1, "/test/song.flac", "flac", 100, None, None, None).await;

        set_file_location(&pool, 1, "local", "/test/song.flac", 100)
            .await
            .unwrap();
        // Update with new size
        set_file_location(&pool, 1, "local", "/test/song.flac", 200)
            .await
            .unwrap();

        let locations = get_file_locations(&pool, 1).await.unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].file_size, Some(200));
    }

    #[tokio::test]
    async fn test_file_location_multiple_types() {
        let pool = test_db().await;
        insert_file(&pool, 1, "/test/song.flac", "flac", 100, None, None, None).await;

        set_file_location(&pool, 1, "local", "/test/song.flac", 100)
            .await
            .unwrap();
        set_file_location(&pool, 1, "backup", "/backup/test/song.flac", 100)
            .await
            .unwrap();

        let locations = get_file_locations(&pool, 1).await.unwrap();
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].location_type, "backup");
        assert_eq!(locations[1].location_type, "local");

        // Remove local
        remove_file_location(&pool, 1, "local").await.unwrap();
        let locations = get_file_locations(&pool, 1).await.unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].location_type, "backup");
    }

    #[tokio::test]
    async fn test_file_location_remove_non_existent() {
        let pool = test_db().await;
        // Removing a location that doesn't exist should not error
        remove_file_location(&pool, 999, "local").await.unwrap();
    }

    #[tokio::test]
    async fn test_record_backup_result() {
        let pool = test_db().await;
        insert_file(&pool, 1, "/test/song.flac", "flac", 100, None, None, None).await;

        record_backup_result(&pool, 1, true, 100, "/backup/test/song.flac")
            .await
            .unwrap();

        let locations = get_file_locations(&pool, 1).await.unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].location_type, "backup");
        assert_eq!(locations[0].path, "/backup/test/song.flac");
    }

    #[tokio::test]
    async fn test_record_backup_result_failure() {
        let pool = test_db().await;
        insert_file(&pool, 1, "/test/song.flac", "flac", 100, None, None, None).await;

        record_backup_result(&pool, 1, false, 0, "/backup/test/song.flac")
            .await
            .unwrap();

        // Failure should not create a backup location
        let locations = get_file_locations(&pool, 1).await.unwrap();
        assert!(locations.is_empty());
    }

    // ── WAV Source Linking ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_set_and_get_files_by_source() {
        let pool = test_db().await;
        insert_file(
            &pool,
            1,
            "/test/stem.stem.m4a",
            "stem.m4a",
            1000,
            None,
            None,
            None,
        )
        .await;
        insert_file(
            &pool,
            2,
            "/test/sub/vocals.wav",
            "wav",
            500,
            None,
            None,
            None,
        )
        .await;
        insert_file(
            &pool,
            3,
            "/test/sub/drums.wav",
            "wav",
            400,
            None,
            None,
            None,
        )
        .await;

        set_file_source_of(&pool, 2, 1).await.unwrap();
        set_file_source_of(&pool, 3, 1).await.unwrap();

        let sources = get_files_by_source(&pool, 1).await.unwrap();
        assert_eq!(sources.len(), 2);
        assert!(sources.iter().all(|f| f.file_type == "wav"));
    }

    #[tokio::test]
    async fn test_get_files_by_source_empty() {
        let pool = test_db().await;
        insert_file(
            &pool,
            1,
            "/test/stem.stem.m4a",
            "stem.m4a",
            1000,
            None,
            None,
            None,
        )
        .await;

        let sources = get_files_by_source(&pool, 1).await.unwrap();
        assert!(sources.is_empty());
    }

    // ── Prune Candidates ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_prune_candidates_basic() {
        let pool = test_db().await;
        // File must be: backed up + local + not in backpack
        let fid = insert_file(
            &pool,
            1,
            "/test/song.flac",
            "flac",
            1000,
            Some("ISRC-1"),
            Some(120.0),
            None,
        )
        .await;
        set_file_location(&pool, fid, "local", "/test/song.flac", 1000)
            .await
            .unwrap();
        set_file_location(&pool, fid, "backup", "/backup/test/song.flac", 1000)
            .await
            .unwrap();

        let candidates = get_prune_candidates(&pool).await.unwrap();
        assert_eq!(candidates.len(), 1, "file should be a prune candidate");
        assert_eq!(candidates[0].file_id, fid);
        assert_eq!(candidates[0].reason, "not_followed");
    }

    #[tokio::test]
    async fn test_prune_candidates_excludes_backpack() {
        let pool = test_db().await;
        let fid = insert_file(
            &pool,
            1,
            "/test/song.flac",
            "flac",
            1000,
            Some("ISRC-1"),
            Some(120.0),
            None,
        )
        .await;
        set_file_location(&pool, fid, "local", "/test/song.flac", 1000)
            .await
            .unwrap();
        set_file_location(&pool, fid, "backup", "/backup/test/song.flac", 1000)
            .await
            .unwrap();

        // Give it a backpack tag
        sqlx::query("INSERT INTO tags (id, name, category_id, backpack) VALUES (1, 'keep', 1, 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix)
             VALUES (?, 1, 'keep', 1, 'Setlist', 'S')",
        )
        .bind(fid)
        .execute(&pool)
        .await
        .unwrap();

        let candidates = get_prune_candidates(&pool).await.unwrap();
        assert_eq!(candidates.len(), 0, "backpack files should be excluded");
    }

    #[tokio::test]
    async fn test_prune_candidates_requires_local() {
        let pool = test_db().await;
        let fid = insert_file(
            &pool,
            1,
            "/test/song.flac",
            "flac",
            1000,
            Some("ISRC-1"),
            Some(120.0),
            None,
        )
        .await;
        // Backed up but NOT local
        set_file_location(&pool, fid, "backup", "/backup/test/song.flac", 1000)
            .await
            .unwrap();

        let candidates = get_prune_candidates(&pool).await.unwrap();
        assert_eq!(
            candidates.len(),
            0,
            "files without local location should be excluded"
        );
    }

    #[tokio::test]
    async fn test_prune_candidates_allows_no_metadata() {
        let pool = test_db().await;
        // FLAC without BPM or comment — still a candidate if backed up + local
        let fid = insert_file(
            &pool,
            1,
            "/test/song.flac",
            "flac",
            1000,
            Some("ISRC-1"),
            None,
            None,
        )
        .await;
        set_file_location(&pool, fid, "local", "/test/song.flac", 1000)
            .await
            .unwrap();
        set_file_location(&pool, fid, "backup", "/backup/test/song.flac", 1000)
            .await
            .unwrap();

        let candidates = get_prune_candidates(&pool).await.unwrap();
        assert_eq!(
            candidates.len(),
            1,
            "files without bpm or comment should still be candidates if backed up + local"
        );
        assert_eq!(candidates[0].file_id, fid);
        assert!(!candidates[0].has_stem_variant);
    }

    #[tokio::test]
    async fn test_prune_candidates_wav_with_source() {
        let pool = test_db().await;
        let stem_id = insert_file(
            &pool,
            1,
            "/test/stem.stem.m4a",
            "stem.m4a",
            1000,
            Some("ISRC-1"),
            Some(120.0),
            None,
        )
        .await;
        let wav_id = insert_file(
            &pool,
            2,
            "/test/sub/vocals.wav",
            "wav",
            500,
            None,
            None,
            None,
        )
        .await;
        set_file_source_of(&pool, wav_id, stem_id).await.unwrap();
        set_file_location(&pool, wav_id, "local", "/test/sub/vocals.wav", 500)
            .await
            .unwrap();
        set_file_location(&pool, wav_id, "backup", "/backup/sub/vocals.wav", 500)
            .await
            .unwrap();

        let candidates = get_prune_candidates(&pool).await.unwrap();
        assert_eq!(
            candidates.len(),
            1,
            "backed-up WAV with source_of should be a candidate"
        );
        assert_eq!(candidates[0].reason, "wav_backed_up");
        assert!(
            candidates[0].has_stem_variant,
            "WAV always has stem variant"
        );
    }

    #[tokio::test]
    async fn test_prune_candidates_wav_without_source_allowed() {
        let pool = test_db().await;
        let wav_id = insert_file(
            &pool,
            2,
            "/test/sub/vocals.wav",
            "wav",
            500,
            None,
            None,
            None,
        )
        .await;
        set_file_location(&pool, wav_id, "local", "/test/sub/vocals.wav", 500)
            .await
            .unwrap();
        set_file_location(&pool, wav_id, "backup", "/backup/sub/vocals.wav", 500)
            .await
            .unwrap();
        // No source_of set — still a candidate if backed up + local

        let candidates = get_prune_candidates(&pool).await.unwrap();
        assert_eq!(
            candidates.len(),
            1,
            "WAV without source_of should still be a candidate if backed up + local"
        );
        assert_eq!(candidates[0].file_id, wav_id);
        assert_eq!(candidates[0].reason, "wav_backed_up");
    }

    #[tokio::test]
    async fn test_prune_candidates_has_stem_variant() {
        let pool = test_db().await;
        // FLAC with same-ISRC stem = has_stem_variant
        let flac = insert_file(
            &pool,
            1,
            "/test/song.flac",
            "flac",
            1000,
            Some("ISRC-X"),
            Some(120.0),
            None,
        )
        .await;
        let _stem = insert_file(
            &pool,
            2,
            "/test/song.stem.m4a",
            "stem.m4a",
            500,
            Some("ISRC-X"),
            Some(120.0),
            None,
        )
        .await;
        set_file_location(&pool, flac, "local", "/test/song.flac", 1000)
            .await
            .unwrap();
        set_file_location(&pool, flac, "backup", "/backup/test/song.flac", 1000)
            .await
            .unwrap();

        let candidates = get_prune_candidates(&pool).await.unwrap();
        assert_eq!(candidates.len(), 1);
        assert!(
            candidates[0].has_stem_variant,
            "FLAC with same-ISRC stem should have stem variant"
        );
    }

    // ── Storage Status ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_storage_status_basic() {
        let pool = test_db().await;

        // Insert a stem and a FLAC with local + backup
        insert_file(
            &pool,
            1,
            "/test/track.stem.m4a",
            "stem.m4a",
            5000,
            None,
            Some(120.0),
            None,
        )
        .await;
        set_file_location(&pool, 1, "local", "/test/track.stem.m4a", 5000)
            .await
            .unwrap();
        set_file_location(&pool, 1, "backup", "/backup/track.stem.m4a", 5000)
            .await
            .unwrap();

        insert_file(
            &pool,
            2,
            "/test/track.flac",
            "flac",
            10000,
            None,
            Some(120.0),
            None,
        )
        .await;
        set_file_location(&pool, 2, "local", "/test/track.flac", 10000)
            .await
            .unwrap();
        set_file_location(&pool, 2, "backup", "/backup/track.flac", 10000)
            .await
            .unwrap();

        // Insert a backup-only file (not local)
        insert_file(&pool, 3, "/test/old.flac", "flac", 2000, None, None, None).await;
        set_file_location(&pool, 3, "backup", "/backup/old.flac", 2000)
            .await
            .unwrap();

        let status = get_storage_status(&pool).await.unwrap();
        assert_eq!(status.local_file_count, 2, "2 local files");
        assert_eq!(status.tracked_file_count, 3, "3 total tracked files");
        assert_eq!(status.local_size_bytes, 15000);
        assert_eq!(status.backup_count, 3, "all 3 are backed up");
        assert_eq!(status.local_stems, 1);
        assert_eq!(status.local_flacs, 1);
        assert_eq!(status.local_stems_size, 5000);
        assert_eq!(status.local_flacs_size, 10000);
        assert!(
            status.prune_candidate_count >= 2,
            "both local tracked files should be candidates"
        );
    }

    #[tokio::test]
    async fn test_storage_status_empty() {
        let pool = test_db().await;
        let status = get_storage_status(&pool).await.unwrap();
        assert_eq!(status.local_file_count, 0);
        assert_eq!(status.tracked_file_count, 0);
        assert_eq!(status.local_size_bytes, 0);
        assert_eq!(status.backup_count, 0);
        assert_eq!(status.prune_candidate_count, 0);
    }

    // ── Unbacked-up files ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_unbacked_up_files() {
        let pool = test_db().await;

        // Insert a folder with path matching our test files
        sqlx::query(
            "INSERT INTO folders (id, folder_path, active, created_at, updated_at)
             VALUES (1, '/test', 1, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // File in the folder, not backed up
        insert_file(&pool, 1, "/test/song.flac", "flac", 1000, None, None, None).await;
        set_file_location(&pool, 1, "local", "/test/song.flac", 1000)
            .await
            .unwrap();

        // File in the folder, backed up
        insert_file(
            &pool,
            2,
            "/test/backed.flac",
            "flac",
            2000,
            None,
            None,
            None,
        )
        .await;
        set_file_location(&pool, 2, "local", "/test/backed.flac", 2000)
            .await
            .unwrap();
        set_file_location(&pool, 2, "backup", "/backup/test/backed.flac", 2000)
            .await
            .unwrap();

        // File outside the folder
        insert_file(&pool, 3, "/other/song.flac", "flac", 3000, None, None, None).await;

        let unbacked = get_unbacked_up_files(&pool, 1).await.unwrap();
        assert_eq!(
            unbacked.len(),
            1,
            "only the file without backup should appear"
        );
        assert_eq!(unbacked[0].id, 1);
    }

    #[tokio::test]
    async fn test_clear_backup_status() {
        let pool = test_db().await;

        sqlx::query(
            "INSERT INTO folders (id, folder_path, active, created_at, updated_at)
             VALUES (1, '/test', 1, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        insert_file(&pool, 1, "/test/song.flac", "flac", 1000, None, None, None).await;
        set_file_location(&pool, 1, "backup", "/backup/test/song.flac", 1000)
            .await
            .unwrap();
        set_file_location(&pool, 1, "local", "/test/song.flac", 1000)
            .await
            .unwrap();

        // Clear backup status
        clear_backup_status(&pool, 1).await.unwrap();

        let locations = get_file_locations(&pool, 1).await.unwrap();
        assert_eq!(locations.len(), 1, "only local should remain");
        assert_eq!(locations[0].location_type, "local");
    }

    // ── Service Config ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_service_config_not_configured() {
        let pool = test_db().await;
        let config = get_service_config(&pool, "spotify").await.unwrap();
        assert!(config.is_none(), "unconfigured service should return None");
    }

    #[tokio::test]
    async fn test_update_and_get_service_config() {
        let pool = test_db().await;

        update_service_config(&pool, "soundcloud", Some("user-123"), Some("pl-main"))
            .await
            .unwrap();

        let config = get_service_config(&pool, "soundcloud").await.unwrap();
        assert!(config.is_some());
        let cfg = config.unwrap();
        assert_eq!(cfg.service, "soundcloud");
        assert_eq!(cfg.user_id.as_deref(), Some("user-123"));
        assert_eq!(cfg.playlist_id.as_deref(), Some("pl-main"));
    }

    #[tokio::test]
    async fn test_update_service_connection_status() {
        let pool = test_db().await;

        // Create config row first
        update_service_config(&pool, "spotify", None, None)
            .await
            .unwrap();

        update_service_connection_status(&pool, "spotify", true)
            .await
            .unwrap();

        let (connected,): (bool,) =
            sqlx::query_as("SELECT is_connected FROM service_config WHERE service = 'spotify'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(connected);

        // Toggle off
        update_service_connection_status(&pool, "spotify", false)
            .await
            .unwrap();
        let (connected,): (bool,) =
            sqlx::query_as("SELECT is_connected FROM service_config WHERE service = 'spotify'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!connected);
    }

    #[tokio::test]
    async fn test_update_service_sync_timestamp() {
        let pool = test_db().await;

        update_service_config(&pool, "youtube", None, Some("yt-main"))
            .await
            .unwrap();

        update_service_sync_timestamp(&pool, "youtube")
            .await
            .unwrap();

        let (synced,): (Option<i64>,) =
            sqlx::query_as("SELECT last_synced FROM service_config WHERE service = 'youtube'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(synced.is_some(), "last_synced should be set");
    }

    #[tokio::test]
    async fn test_update_storage_setting() {
        let pool = test_db().await;

        update_storage_setting(&pool, r#"{"stem_preferred": true}"#)
            .await
            .unwrap();

        let (meta,): (String,) =
            sqlx::query_as("SELECT metadata_json FROM service_config WHERE service = 'storage'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(meta, r#"{"stem_preferred": true}"#);

        // Update again — should overwrite
        update_storage_setting(&pool, r#"{"stem_preferred": false}"#)
            .await
            .unwrap();
        let (meta2,): (String,) =
            sqlx::query_as("SELECT metadata_json FROM service_config WHERE service = 'storage'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(meta2, r#"{"stem_preferred": false}"#);
    }

    #[tokio::test]
    async fn test_update_service_tokens() {
        let pool = test_db().await;

        update_service_config(&pool, "spotify", None, None)
            .await
            .unwrap();

        update_service_tokens(
            &pool,
            "spotify",
            Some("refresh-abc"),
            Some("access-xyz"),
            Some(9999999999),
        )
        .await
        .unwrap();

        let (refresh, access, expiry): (Option<String>, Option<String>, Option<i64>) =
            sqlx::query_as(
                "SELECT refresh_token, access_token, token_expiry FROM service_config WHERE service = 'spotify'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(refresh.as_deref(), Some("refresh-abc"));
        assert_eq!(access.as_deref(), Some("access-xyz"));
        assert_eq!(expiry, Some(9999999999));
    }

    #[tokio::test]
    async fn test_discover_backup_files_new_files() {
        let pool = test_db().await;

        sqlx::query(
            "INSERT INTO folders (id, folder_path, active, created_at, updated_at)
             VALUES (1, '/music/stems', 1, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let remote_files = vec!["track1.flac".to_string(), "track2.stem.m4a".to_string()];

        let result = discover_backup_files(&pool, 1, &remote_files, "/backup")
            .await
            .unwrap();
        assert_eq!(result.files_on_backup, 2);
        assert_eq!(
            result.newly_discovered, 2,
            "both files should be newly discovered"
        );
        assert_eq!(result.already_tracked, 0);

        // Verify files were created
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_discover_backup_files_already_tracked() {
        let pool = test_db().await;

        sqlx::query(
            "INSERT INTO folders (id, folder_path, active, created_at, updated_at)
             VALUES (1, '/music', 1, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Existing file in DB
        insert_file(&pool, 1, "/music/song.flac", "flac", 500, None, None, None).await;

        let remote_files = vec!["song.flac".to_string()];

        let result = discover_backup_files(&pool, 1, &remote_files, "/backup")
            .await
            .unwrap();
        assert_eq!(
            result.already_tracked, 1,
            "file already in DB should be tracked"
        );
        assert_eq!(result.newly_discovered, 0);

        // Should have created a backup file location
        let locations = get_file_locations(&pool, 1).await.unwrap();
        assert_eq!(
            locations.len(),
            1,
            "backup location should have been created"
        );
        assert_eq!(locations[0].location_type, "backup");
    }

    // ── File Deletion ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_local_file_by_id_not_found() {
        let pool = test_db().await;
        // File ID that doesn't exist in DB
        let result = delete_local_file_by_id(&pool, 9999).await.unwrap();
        assert!(!result, "non-existent file should return false");
    }

    #[tokio::test]
    async fn test_delete_local_file_by_id_not_on_disk() {
        let pool = test_db().await;
        insert_file(
            &pool,
            1,
            "/nonexistent/path/file.flac",
            "flac",
            1000,
            None,
            None,
            None,
        )
        .await;
        set_file_location(&pool, 1, "local", "/nonexistent/path/file.flac", 1000)
            .await
            .unwrap();

        let result = delete_local_file_by_id(&pool, 1).await.unwrap();
        assert!(result, "should return true even if file wasn't on disk");

        // Local entry should be removed
        let locations = get_file_locations(&pool, 1).await.unwrap();
        assert!(locations.is_empty(), "local location should be removed");
    }

    // ── Backup Verification ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_verify_backup_candidates_returns_oldest_first() {
        let pool = test_db().await;

        sqlx::query(
            "INSERT INTO folders (id, folder_path, active, created_at, updated_at)
             VALUES (1, '/test', 1, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        insert_file(&pool, 1, "/test/song1.flac", "flac", 1000, None, None, None).await;
        insert_file(&pool, 2, "/test/song2.flac", "flac", 2000, None, None, None).await;

        set_file_location(&pool, 1, "backup", "/backup/test/song1.flac", 1000)
            .await
            .unwrap();
        set_file_location(&pool, 2, "backup", "/backup/test/song2.flac", 2000)
            .await
            .unwrap();

        let candidates = get_verify_backup_candidates(&pool, 1, 10).await.unwrap();
        assert_eq!(candidates.len(), 2, "should find both backup records");
        // First entry should be file 1 (lower file_id / created earlier)
        assert_eq!(candidates[0].0, 1, "file_id should match");
        assert_eq!(
            candidates[0].3, "/backup/test/song1.flac",
            "remote path should match"
        );
    }

    #[tokio::test]
    async fn test_get_verify_backup_candidates_respects_sample_size() {
        let pool = test_db().await;

        sqlx::query(
            "INSERT INTO folders (id, folder_path, active, created_at, updated_at)
             VALUES (1, '/test', 1, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        insert_file(&pool, 1, "/test/song1.flac", "flac", 1000, None, None, None).await;
        insert_file(&pool, 2, "/test/song2.flac", "flac", 2000, None, None, None).await;

        set_file_location(&pool, 1, "backup", "/backup/test/song1.flac", 1000)
            .await
            .unwrap();
        set_file_location(&pool, 2, "backup", "/backup/test/song2.flac", 2000)
            .await
            .unwrap();

        let candidates = get_verify_backup_candidates(&pool, 1, 1).await.unwrap();
        assert_eq!(candidates.len(), 1, "sample_size=1 should return 1 record");
    }

    #[tokio::test]
    async fn test_get_verify_backup_candidates_excludes_wrong_folder() {
        let pool = test_db().await;

        sqlx::query(
            "INSERT INTO folders (id, folder_path, active, created_at, updated_at)
             VALUES (1, '/test', 1, 0, 0), (2, '/other', 1, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        insert_file(&pool, 1, "/test/song.flac", "flac", 1000, None, None, None).await;
        insert_file(
            &pool,
            2,
            "/other/track.flac",
            "flac",
            2000,
            None,
            None,
            None,
        )
        .await;

        set_file_location(&pool, 1, "backup", "/backup/test/song.flac", 1000)
            .await
            .unwrap();
        set_file_location(&pool, 2, "backup", "/backup/other/track.flac", 2000)
            .await
            .unwrap();

        // Only folder 1
        let candidates = get_verify_backup_candidates(&pool, 1, 10).await.unwrap();
        assert_eq!(candidates.len(), 1, "only file in folder 1 should appear");
        assert_eq!(candidates[0].0, 1);
    }

    // ── Backup Size Backfill ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_records_needing_backfill_finds_zero_size() {
        let pool = test_db().await;

        insert_file(&pool, 1, "/test/song.flac", "flac", 0, None, None, None).await;
        set_file_location(&pool, 1, "backup", "/backup/test/song.flac", 0)
            .await
            .unwrap();

        let records = get_records_needing_backfill(&pool).await.unwrap();
        assert_eq!(records.len(), 1, "should find the zero-size record");
        assert_eq!(records[0].file_id, 1);
        assert_eq!(records[0].remote_path, "/backup/test/song.flac");
    }

    #[tokio::test]
    async fn test_get_records_needing_backfill_skips_nonzero_size() {
        let pool = test_db().await;

        insert_file(&pool, 1, "/test/song.flac", "flac", 1000, None, None, None).await;
        insert_file(&pool, 2, "/test/zero.flac", "flac", 0, None, None, None).await;

        set_file_location(&pool, 1, "backup", "/backup/test/song.flac", 1000)
            .await
            .unwrap();
        set_file_location(&pool, 2, "backup", "/backup/test/zero.flac", 0)
            .await
            .unwrap();

        let records = get_records_needing_backfill(&pool).await.unwrap();
        assert_eq!(records.len(), 1, "only the zero-size record");
        assert_eq!(records[0].file_id, 2);
    }

    #[tokio::test]
    async fn test_get_records_needing_backfill_only_backup_type() {
        let pool = test_db().await;

        insert_file(&pool, 1, "/test/song.flac", "flac", 0, None, None, None).await;
        // local with size 0 should NOT appear
        set_file_location(&pool, 1, "local", "/test/song.flac", 0)
            .await
            .unwrap();
        // backup with size > 0 should NOT appear
        set_file_location(&pool, 1, "backup", "/backup/test/song.flac", 500)
            .await
            .unwrap();

        let records = get_records_needing_backfill(&pool).await.unwrap();
        assert!(
            records.is_empty(),
            "backup record has non-zero size, local excluded"
        );
    }
}
