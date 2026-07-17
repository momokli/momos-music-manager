//! Auto-backup poller — periodically checks folders with auto_backup enabled
//! for unbacked files and triggers backup tasks.
//!
//! Runs every 10 minutes by default.

use sqlx::{Pool, Sqlite};
use tracing::{info, warn};

use crate::tasks::{Task, TaskManager, TaskStatus, TaskType};

/// Start the auto-backup poller in the background.
///
/// Every 10 minutes, queries folders with `auto_backup = 1` and a non-empty
/// `backup_path`, checks for unbacked files, and triggers backup tasks via the
/// TaskManager.
pub async fn start_auto_backup_poller(db: Pool<Sqlite>, task_manager: TaskManager) {
    let interval = std::time::Duration::from_secs(600);
    loop {
        tokio::time::sleep(interval).await;

        let task_id = task_manager
            .start_task(Task::new(TaskType::AutoBackupCheck, None))
            .await;
        task_manager
            .update_task_status(&task_id, TaskStatus::Running)
            .await;

        let folders: Vec<crate::db::Folder> = match sqlx::query_as::<_, crate::db::Folder>(
            "SELECT * FROM folders WHERE auto_backup = 1 AND backup_path IS NOT NULL AND backup_path != ''",
        )
        .fetch_all(&db)
        .await
        {
            Ok(f) => f,
            Err(e) => {
                warn!("Auto-backup: failed to query folders: {}", e);
                task_manager
                    .add_log(&task_id, format!("ERROR: failed to query folders: {}", e))
                    .await;
                task_manager
                    .update_task_status(&task_id, TaskStatus::Failed)
                    .await;
                continue;
            }
        };

        task_manager
            .add_log(
                &task_id,
                format!("Checking {} folder(s) with auto_backup...", folders.len()),
            )
            .await;

        let mut backups_triggered = 0usize;
        let mut unbacked_total = 0usize;
        for folder in &folders {
            let unbacked = match crate::db::get_unbacked_up_files(&db, folder.id).await {
                Ok(f) => f,
                Err(e) => {
                    warn!(
                        "Auto-backup: failed to check files for folder {}: {}",
                        folder.id, e,
                    );
                    task_manager
                        .add_log(
                            &task_id,
                            format!(
                                "ERROR: folder '{}': failed to check files: {}",
                                folder.folder_path, e
                            ),
                        )
                        .await;
                    continue;
                }
            };

            if !unbacked.is_empty() {
                task_manager
                    .add_log(
                        &task_id,
                        format!(
                            "Folder '{}': {} unbacked files — triggering backup",
                            folder.folder_path,
                            unbacked.len()
                        ),
                    )
                    .await;
                info!(
                    "Auto-backup: folder '{}' has {} unbacked files — starting backup task",
                    folder.folder_path,
                    unbacked.len()
                );
                crate::tasks::start_backup_folder_task(&task_manager, &db, folder.id).await;
                backups_triggered += 1;
                unbacked_total += unbacked.len();
            }
        }

        if backups_triggered > 0 {
            task_manager
                .add_log(
                    &task_id,
                    format!(
                        "Done: {} backup(s) triggered ({} files total)",
                        backups_triggered, unbacked_total
                    ),
                )
                .await;
        } else {
            task_manager
                .add_log(&task_id, "All folders up to date, no backups needed".into())
                .await;
        }

        task_manager
            .update_task_status(&task_id, TaskStatus::Completed)
            .await;
    }
}
