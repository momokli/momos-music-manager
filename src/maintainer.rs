use sqlx::{FromRow, SqlitePool};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::tasks::{Task, TaskStatus, TaskType};

/// Lightweight folder info for maintainer queries (avoids importing the full Folder struct).
#[derive(Debug, FromRow)]
struct FolderRow {
    id: i64,
    folder_path: String,
    last_scanned: Option<i64>,
}

/// Background maintainer task.
///
/// Periodically checks database health and triggers corrective actions.
/// Doesn't do the work itself — only triggers existing task workers
/// (folder scans, backup reconciliation, backup discovery).
pub async fn start_maintainer(
    db: SqlitePool,
    task_manager: crate::tasks::TaskManager,
    store: crate::store::StoreConfig,
    interval_secs: u64,
    full_scan_max_age: u64,
    auto_prune: bool,
    auto_cleanup_dirs: bool,
    traktor_import_enabled: bool,
    cancel_token: CancellationToken,
) {
    info!(
        "Maintainer started (interval={}s, full_scan_max_age={}s, auto_prune={}, \
         auto_cleanup_dirs={}, traktor_import={})",
        interval_secs,
        full_scan_max_age,
        auto_prune,
        auto_cleanup_dirs,
        traktor_import_enabled,
    );

    // Track collection.nml mtime for Traktor auto-import detection
    let mut last_nml_mtime: Option<i64> = None;

    // Run Traktor import once at startup to seed initial state
    if traktor_import_enabled {
        if let Ok((_path, mtime)) = crate::traktor::get_collection_status(None) {
            let mtime_secs = mtime
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            info!(
                "Maintainer: initial Traktor import at startup (mtime {})",
                mtime_secs
            );
            match crate::traktor::run_import(&db, None).await {
                Ok((stats, _)) => {
                    info!(
                        "Maintainer: initial traktor import complete: {} entries, {} matched, {} BPM, {} key, {} rating",
                        stats.total_entries,
                        stats.matched,
                        stats.with_bpm,
                        stats.with_key,
                        stats.with_rating
                    );
                    last_nml_mtime = Some(mtime_secs);
                }
                Err(e) => warn!("Maintainer: initial traktor import failed: {}", e),
            }
        }
    }

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

        let task_id = task_manager
            .start_task(Task::new(TaskType::MaintainerCycle, None))
            .await;
        task_manager
            .update_task_status(&task_id, TaskStatus::Running)
            .await;
        task_manager
            .add_log(&task_id, "Maintainer cycle starting...".into())
            .await;

        let now = chrono::Utc::now().timestamp();

        // ── Check 0: Refresh materialized tag tables ─────────────────
        //
        // Ensures comment computation uses current tag resolution data
        // after any background changes (global poller, subscription poller,
        // folder watcher, etc.). Runs before all other checks so they
        // operate on fresh data.
        if let Err(e) = crate::db::refresh_file_resolved_tags(&db).await {
            warn!("Maintainer: failed to refresh file_resolved_tags: {}", e);
        }
        if let Err(e) = crate::db::refresh_track_resolved_tags(&db).await {
            warn!("Maintainer: failed to refresh track_resolved_tags: {}", e);
        }

        // ── Check 1: Full scan needed for active folders ──────────────
        //
        // For each active folder, check if `last_scanned` is older than
        // `full_scan_max_age`. If so, trigger a full scan. This ensures
        // file_locations.local stays in sync with the filesystem.
        let folders: Vec<FolderRow> = match sqlx::query_as(
            "SELECT id, folder_path, last_scanned FROM folders WHERE active = 1",
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
                match crate::tasks::start_scan_folder_task(
                    &task_manager,
                    &db,
                    folder.id,
                    crate::db::ScanMode::Full,
                )
                .await
                {
                    Ok(task_id) => {
                        info!(
                            "Maintainer: started full scan task {} for folder #{}",
                            task_id, folder.id
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Maintainer: could not start full scan for folder #{}: {}",
                            folder.id, e
                        );
                    }
                }
            }

        }

        // ── Check 4: Backpack sync ──────────────────────────────────
        //
        // Periodically ensure files in backpack tags are available locally
        // by triggering a background sync task.
        {
            // Check if any backpack tags exist before spawning
            let backpack_tag_count: i64 =
                match sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE backpack = 1")
                    .fetch_one(&db)
                    .await
                {
                    Ok(count) => count,
                    Err(e) => {
                        warn!("Maintainer: failed to count backpack tags: {}", e);
                        0
                    }
                };

            if backpack_tag_count > 0 {
                crate::tasks::start_backpack_sync_task(&task_manager, &db, &store).await;
            }
        }

        // ── Check 7: Traktor collection.nml auto-import ──────────────
        //
        // Checks if collection.nml has been modified (e.g. user analysed
        // new files in Traktor and closed the app). If changed, triggers
        // import to pull BPM, key, rating, and play stats into the DB.
        if traktor_import_enabled {
            if let Ok((_path, mtime)) = crate::traktor::get_collection_status(None) {
                let mtime_secs = mtime
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                if last_nml_mtime.map_or(true, |last| mtime_secs > last) {
                    info!(
                        "Maintainer: collection.nml changed (mtime {}), auto-importing...",
                        mtime_secs
                    );
                    match crate::traktor::run_import(&db, None).await {
                        Ok((stats, _)) => {
                            info!(
                                "Maintainer: traktor auto-import complete: {} entries, {} matched, {} BPM, {} key, {} rating",
                                stats.total_entries,
                                stats.matched,
                                stats.with_bpm,
                                stats.with_key,
                                stats.with_rating,
                            );
                            last_nml_mtime = Some(mtime_secs);
                        }
                        Err(e) => warn!("Maintainer: traktor auto-import failed: {}", e),
                    }
                }
            }
        }

        // ── Check 5: Auto-prune non-backpack files ────────────────────
        //
        // When auto_prune is enabled, delete all local files that are backed
        // up and not protected by any backpack tag. This keeps the local disk
        // a pure cache of only backpack files.
        //
        // Note: pruning is done via start_prune_files_task (background, async).
        // Check 6 (empty dir cleanup) runs in the same maintainer cycle but
        // will only see directories emptied by the *previous* prune cycle.
        // This one-cycle lag is acceptable — empty dirs are cleaned up on the
        // next maintainer run.
        if auto_prune {
            match crate::db::get_prune_candidates(&db).await {
                Ok(candidates) if !candidates.is_empty() => {
                    let file_ids: Vec<i64> = candidates.iter().map(|c| c.file_id).collect();
                    let total_bytes: i64 = candidates.iter().map(|c| c.file_size).sum();
                    info!(
                        "Maintainer: {} prune candidates ({} bytes) — dispatching prune task",
                        file_ids.len(),
                        total_bytes
                    );
                    crate::tasks::start_prune_files_task(&task_manager, &db, file_ids).await;
                }
                Ok(_) => {} // no candidates
                Err(e) => warn!("Maintainer: prune candidate query failed: {}", e),
            }
        }

        // ── Check 6: Remove empty sub-folders in music dirs ───────────
        //
        // After pruning files, some WAV source directories may become empty.
        // Since Check 5 fires a background prune task (async), the empty dirs
        // from TODAY's prune will be cleaned up in the NEXT maintainer cycle.
        // This one-cycle lag is acceptable.
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

        task_manager
            .add_log(&task_id, "Maintainer cycle complete".into())
            .await;
        task_manager
            .update_task_status(&task_id, TaskStatus::Completed)
            .await;
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
    fn test_folder_row_construction() {
        let row = FolderRow {
            id: 1,
            folder_path: "/music/stems".to_string(),
            last_scanned: Some(1000000),
        };

        assert_eq!(row.id, 1);
        assert_eq!(row.folder_path, "/music/stems");
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



}
