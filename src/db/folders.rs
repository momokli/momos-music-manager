//! Folder-related database queries — CRUD, scanning, stats.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Sqlite};

// Types (Folder, ScanMode, etc.) are re-exported from super via legacy.
// Importing from super::* avoids type-mismatch errors with modules that
// expect the legacy:: versions of these types.
use super::*;
use crate::db::normalize_and_validate_folder_path;

// ── Types ───────────────────────────────────────────────────────────────

/// Comprehensive stats for a single folder, including per-type file counts
/// and backup status. Shadow-imported over the simpler version in types.rs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderStats {
    pub id: i64,
    pub folder_path: String,
    pub backup_path: Option<String>,
    pub scan_sources: bool,
    /// Total files in this folder
    pub total_files: i64,
    /// Total size of all files in this folder (bytes)
    pub total_size_bytes: i64,
    /// File counts by type
    pub stems: i64,
    pub flacs: i64,
    pub wavs: i64,
    pub mp3s: i64,
    pub other: i64,
    /// Number of files backed up (have a backup file_locations entry)
    pub backed_up: i64,
    /// Total size of backed up files (bytes)
    pub backed_up_size_bytes: i64,
    /// Number of WAV source subdirectories found
    pub wav_source_dirs: i64,
    /// Number of WAV files indexed from sources
    pub wav_source_files: i64,
    /// Number of WAV files that are backed up
    pub wav_backed_up: i64,
    /// When the folder was last scanned
    pub last_scanned: Option<i64>,
    /// Whether folder watching is active
    pub watch_enabled: bool,
    /// Scan config
    pub scan_recursive: bool,
    pub max_depth: i32,
}

// ── Folder CRUD ─────────────────────────────────────────────────────────

/// Get all folders
pub async fn get_folders(pool: &Pool<Sqlite>) -> Result<Vec<Folder>> {
    let folders = sqlx::query_as::<_, Folder>("SELECT * FROM folders ORDER BY folder_path")
        .fetch_all(pool)
        .await?;
    Ok(folders)
}

/// Create a folder with default config
pub async fn create_folder(pool: &Pool<Sqlite>, folder_path: &str, active: bool) -> Result<Folder> {
    create_folder_with_config(
        pool,
        folder_path,
        active,
        false,         // scan_recursive
        false,         // fixed_extensions
        String::new(), // file_extensions
        1,             // max_depth
    )
    .await
}

/// Create a folder with full configuration
pub async fn create_folder_with_config(
    pool: &Pool<Sqlite>,
    folder_path: &str,
    active: bool,
    scan_recursive: bool,
    fixed_extensions: bool,
    file_extensions: String,
    max_depth: i32,
) -> Result<Folder> {
    // Validate file_extensions if fixed_extensions is true
    if fixed_extensions && !file_extensions.trim().is_empty() {
        crate::audio_extensions::AudioExtension::parse_list(&file_extensions)
            .map_err(|e| anyhow!("Invalid file extensions: {}", e))?;
    }

    let now = chrono::Utc::now().timestamp();
    let folder = sqlx::query_as::<_, Folder>(
        r#"
        INSERT INTO folders (
            folder_path, active, scan_recursive, fixed_extensions,
            file_extensions, max_depth, created_at, updated_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        RETURNING *
        "#,
    )
    .bind(folder_path)
    .bind(active)
    .bind(scan_recursive)
    .bind(fixed_extensions)
    .bind(file_extensions)
    .bind(max_depth)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(folder)
}

/// Get a single folder by ID
pub async fn get_folder_by_id(pool: &Pool<Sqlite>, id: i64) -> Result<Option<Folder>> {
    let folder = sqlx::query_as::<_, Folder>("SELECT * FROM folders WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(folder)
}

/// Update folder path and active status
pub async fn update_folder(
    pool: &Pool<Sqlite>,
    id: i64,
    folder_path: Option<&str>,
    active: Option<bool>,
) -> Result<Folder> {
    update_folder_with_config(
        pool,
        id,
        folder_path,
        active,
        None, // scan_recursive
        None, // fixed_extensions
        None, // file_extensions
        None, // max_depth
    )
    .await
}

/// Update folder with full configuration
#[allow(clippy::too_many_arguments)]
pub async fn update_folder_with_config(
    pool: &Pool<Sqlite>,
    id: i64,
    folder_path: Option<&str>,
    active: Option<bool>,
    scan_recursive: Option<bool>,
    fixed_extensions: Option<bool>,
    file_extensions: Option<&str>,
    max_depth: Option<i32>,
) -> Result<Folder> {
    let now = chrono::Utc::now().timestamp();

    // Validate file_extensions if fixed_extensions is true and file_extensions is provided
    if let (Some(true), Some(extensions)) = (fixed_extensions, file_extensions)
        && !extensions.trim().is_empty()
    {
        crate::audio_extensions::AudioExtension::parse_list(extensions)
            .map_err(|e| anyhow!("Invalid file extensions: {}", e))?;
    }

    // Build dynamic query based on what's being updated
    if let Some(path) = folder_path {
        // Validate new path if provided
        let normalized_path = normalize_and_validate_folder_path(path)?;

        let folder = sqlx::query_as::<_, Folder>(
            r#"
            UPDATE folders
            SET
                folder_path = ?,
                active = COALESCE(?, active),
                scan_recursive = COALESCE(?, scan_recursive),
                fixed_extensions = COALESCE(?, fixed_extensions),
                file_extensions = COALESCE(?, file_extensions),
                max_depth = COALESCE(?, max_depth),
                updated_at = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(normalized_path)
        .bind(active)
        .bind(scan_recursive)
        .bind(fixed_extensions)
        .bind(file_extensions)
        .bind(max_depth)
        .bind(now)
        .bind(id)
        .fetch_one(pool)
        .await?;

        // If scan config changed, reset last_scanned to force a full rescan
        if scan_recursive.is_some() || max_depth.is_some() || file_extensions.is_some() {
            sqlx::query("UPDATE folders SET last_scanned = NULL WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
        }

        Ok(folder)
    } else if active.is_some()
        || scan_recursive.is_some()
        || fixed_extensions.is_some()
        || file_extensions.is_some()
        || max_depth.is_some()
    {
        let folder = sqlx::query_as::<_, Folder>(
            r#"
            UPDATE folders
            SET
                active = COALESCE(?, active),
                scan_recursive = COALESCE(?, scan_recursive),
                fixed_extensions = COALESCE(?, fixed_extensions),
                file_extensions = COALESCE(?, file_extensions),
                max_depth = COALESCE(?, max_depth),
                updated_at = ?
            WHERE id = ?
            RETURNING *
            "#,
        )
        .bind(active)
        .bind(scan_recursive)
        .bind(fixed_extensions)
        .bind(file_extensions)
        .bind(max_depth)
        .bind(now)
        .bind(id)
        .fetch_one(pool)
        .await?;

        // If scan config changed, reset last_scanned to force a full rescan
        if scan_recursive.is_some() || max_depth.is_some() || file_extensions.is_some() {
            sqlx::query("UPDATE folders SET last_scanned = NULL WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
        }

        Ok(folder)
    } else {
        // Nothing to update
        if let Some(folder) = get_folder_by_id(pool, id).await? {
            Ok(folder)
        } else {
            Err(anyhow!("Folder not found with id: {}", id))
        }
    }
}

/// Update only the active status of a folder
pub async fn update_folder_active(pool: &Pool<Sqlite>, id: i64, active: bool) -> Result<Folder> {
    update_folder(pool, id, None, Some(active)).await
}

/// Delete a folder by ID
pub async fn delete_folder(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
    let result = sqlx::query("DELETE FROM folders WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Folder not found with id: {}", id));
    }

    Ok(())
}

// ── Folder Scan ─────────────────────────────────────────────────────────

/// Scan a folder by ID, discovering new and updated files.
/// Returns the number of files found during scanning.
pub async fn scan_folder(
    pool: &Pool<Sqlite>,
    folder_id: i64,
    scan_mode: ScanMode,
) -> Result<usize> {
    // Get folder path
    let folder = get_folder_by_id(pool, folder_id)
        .await?
        .ok_or_else(|| anyhow!("Folder not found with id: {}", folder_id))?;

    let path = std::path::Path::new(&folder.folder_path);

    // Capture scan start time BEFORE scanning, so we can clean up stale local entries
    // (files that existed before this scan but weren't encountered).
    let scan_start = chrono::Utc::now().timestamp();

    // Determine effective scan mode based on folder's last_scanned
    let effective_mode = match &scan_mode {
        ScanMode::Full => ScanMode::Full,
        ScanMode::Incremental { .. } => {
            if let Some(ts) = folder.last_scanned {
                ScanMode::Incremental { since: Some(ts) }
            } else {
                // Never scanned before, do full scan
                ScanMode::Full
            }
        }
    };

    let file_count = crate::db::scan_directory_with_config(
        pool,
        path,
        folder.scan_recursive,
        folder.fixed_extensions,
        folder.file_extensions,
        folder.max_depth,
        effective_mode,
    )
    .await?;

    // Update last_scanned timestamp
    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE folders SET last_scanned = ?, updated_at = ? WHERE id = ?")
        .bind(now)
        .bind(now)
        .bind(folder_id)
        .execute(pool)
        .await?;

    // Stale local entry cleanup: only on full scans.
    // Incremental scans skip unchanged files, so files not encountered
    // in an incremental scan may still exist on disk.
    let is_full_scan = matches!(scan_mode, ScanMode::Full);
    if is_full_scan {
        sqlx::query(
            "DELETE FROM file_locations WHERE location_type = 'local'
         AND file_id IN (
             SELECT f.id FROM files f
             JOIN folders fol ON fol.folder_path = substr(f.file_path, 1, length(fol.folder_path))
             WHERE fol.id = ? AND f.last_scanned < ?
         )",
        )
        .bind(folder_id)
        .bind(scan_start)
        .execute(pool)
        .await?;
    }

    Ok(file_count)
}

/// Get the count of files in a folder
pub async fn get_folder_file_count(pool: &Pool<Sqlite>, folder_id: i64) -> Result<i64> {
    // Get folder path
    let folder = get_folder_by_id(pool, folder_id)
        .await?
        .ok_or_else(|| anyhow!("Folder not found with id: {}", folder_id))?;

    // Count files where file_path starts with folder path
    // Ensure folder path ends with a slash for proper matching
    let folder_path = if folder.folder_path.ends_with('/') {
        folder.folder_path.clone()
    } else {
        format!("{}/", folder.folder_path)
    };

    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM files
        WHERE file_path LIKE ? || '%'
        "#,
    )
    .bind(folder_path)
    .fetch_one(pool)
    .await?;

    Ok(count)
}

// ── Folder Config ──────────────────────────────────────────────────────

/// Update folder backup configuration (backup_path, scan_sources)
pub async fn update_folder_backup_config(
    pool: &Pool<Sqlite>,
    folder_id: i64,
    backup_path: Option<&str>,
    scan_sources: Option<bool>,
) -> Result<()> {
    if let Some(bp) = backup_path {
        sqlx::query("UPDATE folders SET backup_path = ? WHERE id = ?")
            .bind(bp)
            .bind(folder_id)
            .execute(pool)
            .await?;
    }
    if let Some(ss) = scan_sources {
        sqlx::query("UPDATE folders SET scan_sources = ? WHERE id = ?")
            .bind(ss)
            .bind(folder_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Get comprehensive stats for a folder, including per-type file counts,
/// backup status, and WAV source information.
pub async fn get_folder_stats(pool: &Pool<Sqlite>, folder_id: i64) -> Result<FolderStats> {
    let folder = get_folder_by_id(pool, folder_id)
        .await?
        .ok_or_else(|| anyhow!("Folder not found"))?;

    let folder_path_prefix = format!("{}%", folder.folder_path);

    // Total files
    let total_files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE file_path LIKE ?")
        .bind(&folder_path_prefix)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    // Total size
    let total_size_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(file_size), 0) FROM files WHERE file_path LIKE ?")
            .bind(&folder_path_prefix)
            .fetch_one(pool)
            .await
            .unwrap_or(0);

    // By type
    let stems: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type = 'stem.m4a'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let flacs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type = 'flac'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let wavs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type = 'wav'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let mp3s: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type = 'mp3'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let other: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type NOT IN ('stem.m4a','flac','wav','mp3')"
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // Backed up files (have backup location)
    let backed_up: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT fl.file_id) FROM file_locations fl
         JOIN files f ON f.id = fl.file_id
         WHERE f.file_path LIKE ? AND fl.location_type = 'backup'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // Backed up size
    let backed_up_size_bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(f.file_size), 0) FROM files f
         JOIN file_locations fl ON fl.file_id = f.id
         WHERE f.file_path LIKE ? AND fl.location_type = 'backup'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    // WAV source dirs: count subdirs that exist on filesystem
    let wav_source_dirs = if folder.scan_sources {
        let subdirs = crate::db::get_wav_source_subdirs(pool, folder_id)
            .await
            .unwrap_or_default();
        let mut count = 0i64;
        for subdir in &subdirs {
            let full_path = format!("{}/{}", folder.folder_path, subdir);
            let path = std::path::Path::new(&full_path);
            if path.is_dir() {
                count += 1;
            }
        }
        count
    } else {
        0
    };

    let wav_source_files: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE file_path LIKE ? AND file_type = 'wav'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let wav_backed_up: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT fl.file_id) FROM file_locations fl
         JOIN files f ON f.id = fl.file_id
         WHERE f.file_path LIKE ? AND f.file_type = 'wav' AND fl.location_type = 'backup'",
    )
    .bind(&folder_path_prefix)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    Ok(FolderStats {
        id: folder.id,
        folder_path: folder.folder_path,
        backup_path: folder.backup_path,
        scan_sources: folder.scan_sources,
        total_files,
        total_size_bytes,
        stems,
        flacs,
        wavs,
        mp3s,
        other,
        backed_up,
        backed_up_size_bytes,
        wav_source_dirs,
        wav_source_files,
        wav_backed_up,
        last_scanned: folder.last_scanned,
        watch_enabled: folder.active,
        scan_recursive: folder.scan_recursive,
        max_depth: folder.max_depth,
    })
}
