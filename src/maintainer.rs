use sqlx::{FromRow, SqlitePool};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Lightweight folder info for maintainer queries (avoids importing the full Folder struct).
#[derive(Debug, FromRow)]
struct FolderRow {
    id: i64,
    folder_path: String,
    backup_path: Option<String>,
    last_scanned: Option<i64>,
    auto_backup: bool,
}

/// Background maintainer task.
///
/// Periodically checks database health and triggers corrective actions.
/// Doesn't do the work itself — only triggers existing task workers
/// (folder scans, backup reconciliation, backup discovery).
pub async fn start_maintainer(
    db: SqlitePool,
    interval_secs: u64,
    full_scan_max_age: u64,
    backup_discovery_interval: u64,
    auto_prune: bool,
    auto_cleanup_dirs: bool,
    cancel_token: CancellationToken,
) {
    info!(
        "Maintainer started (interval={}s, full_scan_max_age={}s, backup_discovery_interval={}s, \
         auto_prune={}, auto_cleanup_dirs={})",
        interval_secs, full_scan_max_age, backup_discovery_interval, auto_prune, auto_cleanup_dirs,
    );

    // Track when we last ran backup discovery (start at 0 so it runs on first cycle)
    let mut last_backup_discovery: i64 = 0;

    loop {
        // Sleep for the interval (or until cancelled)
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!("Maintainer stopped");
                return;
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(interval_secs)) => {}
        }

        if cancel_token.is_cancelled() {
            return;
        }

        let now = chrono::Utc::now().timestamp();

        // ── Check 1: Full scan needed for active folders ──────────────
        //
        // For each active folder, check if `last_scanned` is older than
        // `full_scan_max_age`. If so, trigger a full scan. This ensures
        // file_locations.local stays in sync with the filesystem.
        let folders: Vec<FolderRow> = match sqlx::query_as(
            "SELECT id, folder_path, backup_path, last_scanned, auto_backup \
             FROM folders WHERE active = 1",
        )
        .fetch_all(&db)
        .await
        {
            Ok(folders) => folders,
            Err(e) => {
                warn!("Maintainer: failed to fetch active folders: {}", e);
                // Continue to next checks rather than aborting the cycle
                continue;
            }
        };

        for folder in &folders {
            // Determine if a full scan is needed
            let needs_scan = match folder.last_scanned {
                Some(ts) => (now - ts) as u64 > full_scan_max_age,
                None => true, // Never scanned — full scan needed
            };

            if needs_scan {
                info!(
                    "Maintainer: folder #{} needs full scan (last_scanned={:?})",
                    folder.id, folder.last_scanned
                );
                match crate::db::scan_folder(&db, folder.id, crate::db::ScanMode::Full).await {
                    Ok(count) => {
                        info!(
                            "Maintainer: full scan completed for folder #{} ({} files)",
                            folder.id, count
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Maintainer: full scan failed for folder #{}: {}",
                            folder.id, e
                        );
                    }
                }
            }

            // ── Check 2: Unbacked-up files (auto-backup folders) ──────
            if folder.auto_backup && folder.backup_path.is_some() {
                let unbacked: i64 = match sqlx::query_scalar(
                    "SELECT COUNT(*) FROM files f \
                     WHERE f.id NOT IN (SELECT file_id FROM file_locations WHERE location_type = 'backup') \
                     AND instr(f.file_path, ?) = 1",
                )
                .bind(&folder.folder_path)
                .fetch_one(&db)
                .await
                {
                    Ok(count) => count,
                    Err(e) => {
                        warn!(
                            "Maintainer: failed to count unbacked files for folder #{}: {}",
                            folder.id, e
                        );
                        0
                    }
                };

                if unbacked > 0 {
                    warn!(
                        "Maintainer: folder #{} has {} unbacked-up files — manual backup may be needed",
                        folder.id, unbacked
                    );
                }
            }
        }

        // ── Check 3: Backup discovery (weekly) ────────────────────────
        //
        // For folders with a backup_path configured, scan the NAS for files
        // that exist on backup but not in the local DB. Creates bare records
        // with sentinel hashes so they show up in the files index.
        if now - last_backup_discovery > backup_discovery_interval as i64 {
            #[derive(Debug, FromRow)]
            struct FolderBackupRow {
                id: i64,
                backup_path: String,
            }

            let folders_with_backup: Vec<FolderBackupRow> = match sqlx::query_as(
                "SELECT id, backup_path FROM folders WHERE backup_path IS NOT NULL AND backup_path != ''",
            )
            .fetch_all(&db)
            .await
            {
                Ok(folders) => folders,
                Err(e) => {
                    warn!("Maintainer: failed to fetch folders with backup_path: {}", e);
                    continue;
                }
            };

            for folder in &folders_with_backup {
                info!(
                    "Maintainer: triggering backup discovery for folder #{}",
                    folder.id
                );

                // Parse backup_path: format is "host:/remote/path"
                if let Some((ssh_host, remote_base)) = folder.backup_path.split_once(':') {
                    let engine = crate::backup::BackupEngine::new(ssh_host.to_string());
                    let max_depth: u32 = 2; // match typical folder config depth

                    match engine.list_remote_files_full(remote_base, max_depth).await {
                        Ok(remote_files) => {
                            if remote_files.is_empty() {
                                info!(
                                    "Maintainer: no files found on backup for folder #{}",
                                    folder.id
                                );
                            } else {
                                match crate::db::discover_backup_files(
                                    &db,
                                    folder.id,
                                    &remote_files,
                                    remote_base,
                                )
                                .await
                                {
                                    Ok(result) => {
                                        info!(
                                            "Maintainer: backup discovery for folder #{}: \
                                             {} on backup, {} already tracked, {} newly discovered, \
                                             {} missing from backup",
                                            folder.id,
                                            result.files_on_backup,
                                            result.already_tracked,
                                            result.newly_discovered,
                                            result.missing_from_backup.len(),
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Maintainer: backup discovery failed for folder #{}: {}",
                                            folder.id, e
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Maintainer: failed to list remote files for folder #{}: {}",
                                folder.id, e
                            );
                        }
                    }
                } else {
                    warn!(
                        "Maintainer: invalid backup_path format for folder #{}: {} \
                         (expected 'host:/remote/path')",
                        folder.id, folder.backup_path
                    );
                }
            }

            last_backup_discovery = now;
        }

        // ── Check 4: Backpack sync ──────────────────────────────────
        //
        // Periodically check if any files in backpack tags are missing
        // locally and pull them from backup. This ensures files become
        // available even when the initial auto-pull from the tag toggle
        // didn't complete (e.g. server was restarted).
        {
            match crate::db::get_backpack_pull_candidates(&db).await {
                Ok(candidates) if !candidates.is_empty() => {
                    info!(
                        "Maintainer: {} files in backpack tags need pulling from backup",
                        candidates.len()
                    );
                    let mut pulled = 0usize;
                    for c in &candidates {
                        let (ssh_host, remote_path) = match c.backup_path.split_once(':') {
                            Some((h, p)) => (h, p),
                            None => {
                                warn!("Maintainer: invalid backup path for file #{}", c.file_id);
                                continue;
                            }
                        };
                        let engine = crate::backup::BackupEngine::new(ssh_host.to_string());
                        let local = std::path::Path::new(&c.local_path);
                        match engine.pull_file(remote_path, local).await {
                            Ok((true, size)) => {
                                pulled += 1;
                                let _ = crate::db::set_file_location(
                                    &db,
                                    c.file_id,
                                    "local",
                                    &c.local_path,
                                    size,
                                )
                                .await;
                                let _ = sqlx::query(
                                    "UPDATE files SET last_verified_local = unixepoch() WHERE id = ?",
                                )
                                .bind(c.file_id)
                                .execute(&db)
                                .await;
                            }
                            Ok((false, _)) => {
                                warn!("Maintainer: failed to pull {} from backup", c.local_path);
                            }
                            Err(e) => {
                                warn!("Maintainer: error pulling {}: {}", c.local_path, e);
                            }
                        }
                    }
                    info!(
                        "Maintainer: backpack sync pulled {}/{} files",
                        pulled,
                        candidates.len()
                    );
                }
                Ok(_) => {
                    // No candidates — all backpack files are local
                }
                Err(e) => {
                    warn!("Maintainer: failed to get backpack pull candidates: {}", e);
                }
            }
        }

        // ── Check 5: Auto-prune non-backpack files ────────────────────
        //
        // When auto_prune is enabled, delete all local files that are backed
        // up and not protected by any backpack tag. This keeps the local disk
        // a pure cache of only backpack files.
        if auto_prune {
            match crate::db::get_prune_candidates(&db).await {
                Ok(candidates) if !candidates.is_empty() => {
                    let mut deleted = 0usize;
                    let mut freed: i64 = 0;
                    for c in &candidates {
                        let path = std::path::Path::new(&c.file_path);
                        match tokio::fs::remove_file(path).await {
                            Ok(()) => {
                                deleted += 1;
                                freed += c.file_size;
                                let _ = sqlx::query(
                                    "DELETE FROM file_locations WHERE file_id = ? \
                                     AND location_type = 'local'",
                                )
                                .bind(c.file_id)
                                .execute(&db)
                                .await;
                            }
                            Err(e) => {
                                warn!(
                                    "Maintainer auto-prune: failed to delete {}: {}",
                                    c.file_path, e
                                );
                            }
                        }
                    }
                    if deleted > 0 {
                        info!(
                            "Maintainer auto-prune: deleted {} files, freed {} bytes",
                            deleted, freed
                        );
                    }
                }
                Ok(_) => {} // no candidates
                Err(e) => warn!("Maintainer: prune candidate query failed: {}", e),
            }
        }

        // ── Check 6: Remove empty sub-folders in music dirs ───────────
        //
        // After pruning files, some WAV source directories may become empty.
        // Remove them to keep the filesystem clean.
        if auto_cleanup_dirs {
            for folder in &folders {
                let root = std::path::Path::new(&folder.folder_path);
                if !root.exists() || !root.is_dir() {
                    continue;
                }
                match std::fs::read_dir(root) {
                    Ok(entries) => {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() {
                                if let Ok(mut dir_entries) = std::fs::read_dir(&path) {
                                    if dir_entries.next().is_none() {
                                        match std::fs::remove_dir(&path) {
                                            Ok(()) => debug!(
                                                "Maintainer: removed empty directory: {}",
                                                path.display()
                                            ),
                                            Err(e) => warn!(
                                                "Maintainer: failed to remove empty \
                                                 directory {}: {}",
                                                path.display(),
                                                e
                                            ),
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Maintainer: failed to list directory {}: {}",
                            root.display(),
                            e
                        );
                    }
                }
            }
        }
    }
}

/// Determine whether a full scan is needed for a folder.
///
/// Returns `true` when:
/// - The folder was never scanned (`last_scanned` is `None`), or
/// - The time since the last scan exceeds `max_age_secs`.
pub fn needs_full_scan(last_scanned: Option<i64>, now: i64, max_age_secs: u64) -> bool {
    match last_scanned {
        Some(ts) => (now - ts) as u64 > max_age_secs,
        None => true,
    }
}

/// Parse a backup path in the format `host:/remote/path` into its components.
pub fn parse_backup_path(backup_path: &str) -> Option<(&str, &str)> {
    backup_path.split_once(':')
}

/// Determine whether backup discovery should run, based on the last run timestamp
/// and the configured interval.
pub fn should_run_backup_discovery(now: i64, last_run: i64, interval_secs: u64) -> bool {
    now - last_run > interval_secs as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_full_scan_never_scanned() {
        assert!(needs_full_scan(None, 1000, 3600));
    }

    #[test]
    fn test_needs_full_scan_expired() {
        assert!(needs_full_scan(Some(0), 10000, 3600));
    }

    #[test]
    fn test_needs_full_scan_within_window() {
        assert!(!needs_full_scan(Some(5000), 6000, 3600));
    }

    #[test]
    fn test_needs_full_scan_exactly_at_boundary() {
        assert!(!needs_full_scan(Some(0), 3600, 3600));
    }

    #[test]
    fn test_parse_backup_path_valid() {
        let result = parse_backup_path("backup:/volume1/media/stems");
        assert_eq!(result, Some(("backup", "/volume1/media/stems")));
    }

    #[test]
    fn test_parse_backup_path_no_colon() {
        let result = parse_backup_path("invalidpath");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_backup_path_multiple_colons() {
        let result = parse_backup_path("host:/path/to/dir:extra");
        assert_eq!(result, Some(("host", "/path/to/dir:extra")));
    }

    #[test]
    fn test_parse_backup_path_empty_string() {
        // Split on empty string returns None (no colon found)
        let result = parse_backup_path("");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_backup_path_empty_host() {
        // Colon at start: host is empty, path is present
        let result = parse_backup_path(":/remote/path");
        assert_eq!(result, Some(("", "/remote/path")));
    }

    #[test]
    fn test_parse_backup_path_no_path() {
        // Colon at end: host is present, path is empty
        let result = parse_backup_path("host:");
        assert_eq!(result, Some(("host", "")));
    }

    #[test]
    fn test_should_run_backup_discovery_never_run() {
        assert!(!should_run_backup_discovery(100000, 0, 604800));
    }

    #[test]
    fn test_should_run_backup_discovery_after_interval() {
        assert!(should_run_backup_discovery(700000, 0, 604800));
    }

    #[test]
    fn test_should_run_backup_discovery_within_interval() {
        assert!(!should_run_backup_discovery(600000, 500000, 604800));
    }

    #[test]
    fn test_folder_row_construction() {
        let row = FolderRow {
            id: 1,
            folder_path: "/music/stems".to_string(),
            backup_path: Some("backup:/volume1/stems".to_string()),
            last_scanned: Some(1000000),
            auto_backup: true,
        };

        assert_eq!(row.id, 1);
        assert_eq!(row.folder_path, "/music/stems");
        assert!(row.auto_backup);
        assert!(row.backup_path.is_some());
    }

    // ── Zero / large interval edge cases ────────────────────────────

    #[test]
    fn test_needs_full_scan_zero_interval_just_scanned() {
        // interval=0, last_scanned matches now → not expired
        assert!(!needs_full_scan(Some(1000), 1000, 0));
    }

    #[test]
    fn test_needs_full_scan_zero_interval_expired() {
        // interval=0, any positive elapsed time means expired
        assert!(needs_full_scan(Some(1000), 1001, 0));
    }

    #[test]
    fn test_needs_full_scan_very_large_interval() {
        // A 31-year interval means nothing triggers within reasonable time
        assert!(!needs_full_scan(Some(1_000_000), 2_000_000, 1_000_000_000));
    }

    #[test]
    fn test_needs_full_scan_large_age_just_below_boundary() {
        // elapsed = max_age - 1 → not expired
        assert!(!needs_full_scan(Some(0), 59, 60));
    }

    #[test]
    fn test_needs_full_scan_large_age_at_boundary() {
        // elapsed = max_age → not expired (strictly greater)
        assert!(!needs_full_scan(Some(0), 60, 60));
    }

    #[test]
    fn test_needs_full_scan_large_age_just_over_boundary() {
        // elapsed = max_age + 1 → expired
        assert!(needs_full_scan(Some(0), 61, 60));
    }

    #[test]
    fn test_should_run_backup_discovery_exact_boundary() {
        // now - last_run == interval → not yet (strictly greater)
        assert!(!should_run_backup_discovery(604800, 0, 604800));
    }

    #[test]
    fn test_should_run_backup_discovery_just_over_boundary() {
        // now - last_run == interval + 1 → should run
        assert!(should_run_backup_discovery(604801, 0, 604800));
    }

    #[test]
    fn test_should_run_backup_discovery_zero_interval() {
        // interval=0, any positive elapsed time should trigger
        assert!(should_run_backup_discovery(1, 0, 0));
    }
}
