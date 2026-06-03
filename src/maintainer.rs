use sqlx::{FromRow, SqlitePool};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

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
    cancel_token: CancellationToken,
) {
    info!(
        "Maintainer started (interval={}s, full_scan_max_age={}s, backup_discovery_interval={}s)",
        interval_secs, full_scan_max_age, backup_discovery_interval
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
    }
}
