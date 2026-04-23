//! Generic task management module
//!
//! This module provides in-memory task tracking for background operations.
//! Supports multiple task types: SpotifySync, WriteComment, and future types.
//! Sync state is tracked in memory, not in the database, to avoid locking issues
//! and provide real-time progress updates.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use uuid::Uuid;

// Re-export old sync types that SpotifySyncWorker needs (will be removed after full migration)
pub use crate::sync::{SyncProgress, SyncType};

use crate::config::ServiceCredentials;
use crate::spotify::{client::SpotifyClient, sync_worker::SpotifySyncWorker};

/// Type of task operation
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TaskType {
    /// Spotify sync operation (playlists, tracks, full)
    SpotifySync(SyncConfig),
    /// Write comment to file(s)
    WriteComment { file_ids: Vec<i64> },
}

/// Configuration for Spotify sync (generic serializable version of SyncType)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncConfig {
    /// Sync only playlist metadata
    Playlists,
    /// Sync tracks for a specific playlist
    TracksForPlaylist(String),
    /// Sync tracks for all playlists
    TracksAll,
    /// Full sync: playlists + all tracks
    Full,
}

impl From<SyncType> for SyncConfig {
    fn from(sync_type: SyncType) -> Self {
        match sync_type {
            SyncType::Playlists => SyncConfig::Playlists,
            SyncType::TracksForPlaylist(id) => SyncConfig::TracksForPlaylist(id),
            SyncType::TracksAll => SyncConfig::TracksAll,
            SyncType::Full => SyncConfig::Full,
        }
    }
}

impl SyncConfig {
    /// Convert back to SyncType for compatibility with SpotifySyncWorker
    pub fn to_sync_type(&self) -> SyncType {
        match self {
            SyncConfig::Playlists => SyncType::Playlists,
            SyncConfig::TracksForPlaylist(id) => SyncType::TracksForPlaylist(id.clone()),
            SyncConfig::TracksAll => SyncType::TracksAll,
            SyncConfig::Full => SyncType::Full,
        }
    }
}

/// Status of a task
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    /// Task is queued but not yet started
    Pending,
    /// Task is currently running
    Running,
    /// Task completed successfully
    Completed,
    /// Task failed with an error
    Failed,
    /// Task was cancelled by the user
    Cancelled,
}

/// A background task with progress tracking
pub struct Task {
    /// Unique task ID
    pub id: String,
    /// Type of task operation
    pub task_type: TaskType,
    /// Current status (use std::sync::Mutex for synchronous reads in TaskProgress conversion)
    pub status: Arc<std::sync::Mutex<TaskStatus>>,
    /// Service name (spotify, soundcloud, youtube) if applicable
    pub service: Option<String>,
    /// Human-readable progress text
    pub progress_text: Arc<std::sync::Mutex<String>>,
    /// Detailed sync progress (only for Spotify sync tasks, uses tokio::RwLock for worker)
    pub sync_progress: Option<Arc<RwLock<SyncProgress>>>,
    /// Log messages (last 100)
    pub logs: Arc<std::sync::Mutex<VecDeque<String>>>,
    /// Cancellation token for this task
    pub cancel_token: CancellationToken,
    /// When the task was created
    pub created_at: Instant,
    /// Join handle for the background task
    pub join_handle: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl Task {
    /// Create a new generic task
    pub fn new(task_type: TaskType, service: Option<String>) -> Self {
        let id = Uuid::new_v4().to_string();
        Self {
            id,
            task_type,
            status: Arc::new(std::sync::Mutex::new(TaskStatus::Pending)),
            service,
            progress_text: Arc::new(std::sync::Mutex::new("Pending".to_string())),
            sync_progress: None,
            logs: Arc::new(std::sync::Mutex::new(VecDeque::new())),
            cancel_token: CancellationToken::new(),
            created_at: Instant::now(),
            join_handle: None,
        }
    }

    /// Create a new sync task with detailed progress tracking
    pub fn new_sync(service: String, sync_type: SyncType) -> Self {
        let id = Uuid::new_v4().to_string();
        let config = SyncConfig::from(sync_type.clone());
        Self {
            id,
            task_type: TaskType::SpotifySync(config),
            status: Arc::new(std::sync::Mutex::new(TaskStatus::Pending)),
            service: Some(service),
            progress_text: Arc::new(std::sync::Mutex::new("Starting...".to_string())),
            sync_progress: Some(Arc::new(RwLock::new(SyncProgress::new(sync_type)))),
            logs: Arc::new(std::sync::Mutex::new(VecDeque::new())),
            cancel_token: CancellationToken::new(),
            created_at: Instant::now(),
            join_handle: None,
        }
    }

    /// Add a log message (keeps last 100 entries)
    pub fn add_log(&self, message: String) {
        let mut logs = self.logs.lock().unwrap();
        logs.push_back(message);
        if logs.len() > 100 {
            logs.pop_front();
        }
    }

    /// Check if task has been cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Convert to serializable TaskProgress
    pub fn to_progress(&self) -> TaskProgress {
        let task_type_str = match &self.task_type {
            TaskType::SpotifySync(_) => "spotify_sync".to_string(),
            TaskType::WriteComment { .. } => "write_comment".to_string(),
        };

        let status = self.status.lock().unwrap().clone();
        let progress = self.progress_text.lock().unwrap().clone();
        let logs: Vec<String> = self.logs.lock().unwrap().iter().cloned().collect();
        // Use current timestamp minus elapsed as the creation time (relative)
        let created_at_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() - self.created_at.elapsed().as_secs_f64())
            .unwrap_or(0.0);

        TaskProgress {
            id: self.id.clone(),
            task_type: task_type_str,
            task_details: Some(self.task_type.clone()),
            status,
            service: self.service.clone(),
            progress,
            logs,
            created_at_secs,
        }
    }
}

/// Serializable progress snapshot for API responses
#[derive(Clone, Debug, Serialize)]
pub struct TaskProgress {
    pub id: String,
    pub task_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_details: Option<TaskType>,
    pub status: TaskStatus,
    pub service: Option<String>,
    pub progress: String,
    pub logs: Vec<String>,
    pub created_at_secs: f64,
}

/// In-memory task manager
#[derive(Clone)]
pub struct TaskManager {
    /// Map of task_id -> Task
    tasks: Arc<RwLock<HashMap<String, Task>>>,
}

impl TaskManager {
    /// Create a new task manager
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new task and return its ID
    pub async fn start_task(&self, task: Task) -> String {
        let id = task.id.clone();
        let mut tasks = self.tasks.write().await;
        tasks.insert(id.clone(), task);
        id
    }

    /// Get a serializable snapshot of a task
    pub async fn get_task(&self, task_id: &str) -> Option<TaskProgress> {
        let tasks = self.tasks.read().await;
        tasks.get(task_id).map(|task| task.to_progress())
    }

    /// Cancel a task
    pub async fn cancel_task(&self, task_id: &str) -> anyhow::Result<()> {
        let mut tasks = self.tasks.write().await;

        if let Some(task) = tasks.get_mut(task_id) {
            // Update status to cancelled
            *task.status.lock().unwrap() = TaskStatus::Cancelled;
            task.add_log("Task cancelled by user".to_string());

            // Send cancellation signal
            task.cancel_token.cancel();

            Ok(())
        } else {
            Err(anyhow::anyhow!("Task not found: {}", task_id))
        }
    }

    /// List all tasks (returns serializable snapshots)
    pub async fn list_tasks(&self) -> Vec<TaskProgress> {
        let tasks = self.tasks.read().await;
        let mut result: Vec<TaskProgress> = tasks.values().map(|t| t.to_progress()).collect();
        // Sort by creation time (most recent first)
        result.sort_by(|a, b| {
            b.created_at_secs
                .partial_cmp(&a.created_at_secs)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        result
    }

    /// List tasks with pagination and optional status filter
    pub async fn list_tasks_paginated(
        &self,
        limit: usize,
        offset: usize,
        status_filter: Option<TaskStatus>,
    ) -> (Vec<TaskProgress>, usize) {
        let tasks = self.tasks.read().await;
        let mut all: Vec<TaskProgress> = tasks.values().map(|t| t.to_progress()).collect();
        // Sort by creation time (most recent first)
        all.sort_by(|a, b| {
            b.created_at_secs
                .partial_cmp(&a.created_at_secs)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let filtered: Vec<TaskProgress> = if let Some(ref filter) = status_filter {
            all.into_iter().filter(|t| t.status == *filter).collect()
        } else {
            all
        };

        let total = filtered.len();
        let paginated: Vec<TaskProgress> = filtered.into_iter().skip(offset).take(limit).collect();

        (paginated, total)
    }

    /// Update a task's status
    pub async fn update_task_status(&self, task_id: &str, status: TaskStatus) {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id) {
            *task.status.lock().unwrap() = status;
        }
    }

    /// Add a log message to a task
    pub async fn add_log(&self, task_id: &str, message: String) {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id) {
            task.add_log(message);
        }
    }

    /// Update the progress text of a task
    pub async fn update_progress_text(&self, task_id: &str, text: String) {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id) {
            *task.progress_text.lock().unwrap() = text;
        }
    }

    // ---- Sync-specific methods (for backward compatibility) ----

    /// Get detailed sync progress for a task (returns old SyncProgress format)
    pub async fn get_sync_progress(&self, task_id: &str) -> Option<SyncProgress> {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id) {
            if let Some(ref sync_progress) = task.sync_progress {
                return Some(sync_progress.read().await.clone());
            }
        }
        None
    }

    /// Get the cancellation token for a task
    pub async fn get_cancel_token(&self, task_id: &str) -> Option<CancellationToken> {
        let tasks = self.tasks.read().await;
        tasks.get(task_id).map(|t| t.cancel_token.clone())
    }

    /// Set the join handle for a task
    pub async fn set_join_handle(
        &self,
        task_id: &str,
        handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    ) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(task_id) {
            task.join_handle = Some(handle);
        }
    }

    /// Remove a completed/failed/cancelled task
    #[allow(dead_code)]
    pub async fn remove_task(&self, task_id: &str) {
        let mut tasks = self.tasks.write().await;
        tasks.remove(task_id);
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Helper functions for spawning Spotify sync tasks
// ============================================================

/// Start a Spotify sync task using the TaskManager.
/// This function replicates the old SyncManager::start_spotify_sync logic.
pub async fn start_spotify_sync_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    credentials: &ServiceCredentials,
    sync_type: SyncType,
) -> anyhow::Result<String> {
    let service = "spotify".to_string();

    // Check if a Spotify sync is already running
    let tasks = task_manager.tasks.read().await;
    for (_id, task) in tasks.iter() {
        if task.service.as_deref() == Some(&service) {
            let status = task.status.lock().unwrap().clone();
            if status == TaskStatus::Running || status == TaskStatus::Pending {
                return Err(anyhow::anyhow!("Spotify sync already running"));
            }
        }
    }
    drop(tasks); // Release read lock

    // Create task with detailed sync progress
    let task = Task::new_sync(service.clone(), sync_type.clone());
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let cancel_token = task.cancel_token.clone();
    let sync_progress = task.sync_progress.as_ref().unwrap().clone();

    // Register task
    task_manager.start_task(task).await;

    info!("Starting Spotify sync with type: {:?}", sync_type);

    // Clone everything needed for background task
    let tm = task_manager.clone();
    let db_clone = db.clone();
    let creds = credentials.clone();
    let sync_type_clone = sync_type.clone();

    // Spawn background task
    let join_handle = tokio::spawn(async move {
        // Update status to Running
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(
            &worker_task_id,
            format!("Starting {:?} sync...", sync_type_clone),
        )
        .await;

        // Create Spotify client
        let spotify_client = match SpotifyClient::from_stored_tokens(db_clone.clone(), &creds).await
        {
            Ok(client) => {
                info!("Spotify client created successfully");
                client
            }
            Err(e) => {
                error!("Failed to create Spotify client: {}", e);
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                tm.add_log(
                    &worker_task_id,
                    format!("Failed to create Spotify client: {}", e),
                )
                .await;
                tm.update_progress_text(&worker_task_id, format!("Failed: {}", e))
                    .await;
                return Err(anyhow::anyhow!("Failed to create Spotify client: {}", e));
            }
        };

        info!(
            "Creating SpotifySyncWorker with sync type: {:?}",
            sync_type_clone
        );

        // Create and run sync worker
        let worker = SpotifySyncWorker::new(
            db_clone.clone(),
            spotify_client,
            worker_task_id.clone(),
            sync_type_clone,
            cancel_token,
            sync_progress,
        );

        match worker.run().await {
            Ok(result) => {
                info!(
                    "Spotify sync worker completed: {} playlists, {} tracks",
                    result.playlist_count, result.track_count
                );
                // Update remote counts in database if sync was successful
                if result.error.is_none() {
                    let now = chrono::Utc::now().timestamp();
                    if let Err(e) = sqlx::query(
                        r#"
                        UPDATE service_config
                        SET remote_playlists_count = ?,
                            remote_tracks_count = ?,
                            last_synced = ?,
                            updated_at = ?
                        WHERE service = 'spotify'
                        "#,
                    )
                    .bind(result.playlist_count as i64)
                    .bind(result.track_count as i64)
                    .bind(now)
                    .bind(now)
                    .execute(&db_clone)
                    .await
                    {
                        error!("Failed to update remote counts: {}", e);
                    }
                }

                let status = if result.error.is_some() {
                    TaskStatus::Failed
                } else {
                    TaskStatus::Completed
                };
                tm.update_task_status(&worker_task_id, status).await;
                if let Some(ref error) = result.error {
                    tm.add_log(&worker_task_id, format!("Sync failed: {}", error))
                        .await;
                    tm.update_progress_text(&worker_task_id, format!("Failed: {}", error))
                        .await;
                } else {
                    tm.add_log(
                        &worker_task_id,
                        format!(
                            "Sync completed: {} playlists, {} tracks",
                            result.playlist_count, result.track_count
                        ),
                    )
                    .await;
                    tm.update_progress_text(
                        &worker_task_id,
                        format!(
                            "Completed: {} playlists, {} tracks",
                            result.playlist_count, result.track_count
                        ),
                    )
                    .await;
                }

                Ok(())
            }
            Err(e) => {
                error!("Spotify sync worker failed: {}", e);
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                tm.add_log(&worker_task_id, format!("Sync failed: {}", e))
                    .await;
                tm.update_progress_text(&worker_task_id, format!("Failed: {}", e))
                    .await;
                Err(e)
            }
        }
    });

    // Set join handle
    task_manager.set_join_handle(&task_id, join_handle).await;

    Ok(task_id)
}

// ============================================================
// WriteComment worker
// ============================================================

/// Start a WriteComment task for one or more files.
/// Each file is processed: compute_target → exiftool write → DB update.
/// Continues on individual file errors; logs warnings for DB failures after successful write.
pub async fn start_write_comment_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    file_ids: Vec<i64>,
) -> String {
    let task_type = TaskType::WriteComment {
        file_ids: file_ids.clone(),
    };
    let task = Task::new(task_type, None);
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let cancel_token = task.cancel_token.clone();

    task_manager.start_task(task).await;

    let tm = task_manager.clone();
    let db_clone = db.clone();

    let join_handle = tokio::spawn(async move {
        // Update status to Running
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(
            &worker_task_id,
            format!("Writing comment for {} file(s)...", file_ids.len()),
        )
        .await;

        let total = file_ids.len();
        let mut written = 0usize;
        let mut skipped = 0usize;
        let mut errors = 0usize;
        let mut warnings: Vec<String> = Vec::new();

        for (i, file_id) in file_ids.iter().enumerate() {
            // Check cancellation
            if let Some(ct) = tm.get_cancel_token(&worker_task_id).await {
                if ct.is_cancelled() {
                    tm.add_log(&worker_task_id, "Task cancelled".to_string())
                        .await;
                    tm.update_task_status(&worker_task_id, TaskStatus::Cancelled)
                        .await;
                    tm.update_progress_text(&worker_task_id, "Cancelled".to_string())
                        .await;
                    return Ok(());
                }
            }

            // 1. Fetch file from database
            let file =
                match sqlx::query_as::<_, crate::db::File>("SELECT * FROM files WHERE id = ?")
                    .bind(file_id)
                    .fetch_optional(&db_clone)
                    .await
                {
                    Ok(Some(f)) => f,
                    Ok(None) => {
                        tm.add_log(
                            &worker_task_id,
                            format!("File #{} not found, skipping", file_id),
                        )
                        .await;
                        errors += 1;
                        continue;
                    }
                    Err(e) => {
                        tm.add_log(
                            &worker_task_id,
                            format!("Error fetching file #{}: {}", file_id, e),
                        )
                        .await;
                        errors += 1;
                        continue;
                    }
                };

            let title_display = file
                .title
                .clone()
                .unwrap_or_else(|| "(untitled)".to_string());
            let file_path = file.file_path.clone();

            // 2. Compute target comment
            let target = match crate::db::compute_target_comment(&db_clone, *file_id).await {
                Ok(t) => t,
                Err(e) => {
                    tm.add_log(
                        &worker_task_id,
                        format!(
                            "Error computing target comment for '{}': {}",
                            title_display, e
                        ),
                    )
                    .await;
                    errors += 1;
                    continue;
                }
            };

            // 3. Check if already up to date
            if file.comment.as_deref() == Some(&target) {
                skipped += 1;
                continue;
            }

            // 4. Check file exists on disk
            if !std::path::Path::new(&file_path).exists() {
                tm.add_log(
                    &worker_task_id,
                    format!(
                        "File not found on disk: '{}' ({})",
                        title_display, file_path
                    ),
                )
                .await;
                errors += 1;
                continue;
            }

            // Update progress
            tm.update_progress_text(
                &worker_task_id,
                format!("Writing file {}/{}: '{}'", i + 1, total, title_display),
            )
            .await;
            tm.add_log(
                &worker_task_id,
                format!("Writing comment to '{}'...", title_display),
            )
            .await;

            // 5. Write comment to file via exiftool
            if let Err(e) = crate::db::write_comment_to_file(&file_path, &target).await {
                tm.add_log(
                    &worker_task_id,
                    format!("Failed to write comment to '{}': {}", title_display, e),
                )
                .await;
                errors += 1;
                continue;
            }

            // 6. Update database
            if let Err(e) = crate::db::update_file_comment(&db_clone, *file_id, &target).await {
                let warn_msg = format!(
                    "Comment written to file but DB update failed for '{}': {}",
                    title_display, e
                );
                tm.add_log(&worker_task_id, format!("WARNING: {}", warn_msg))
                    .await;
                warnings.push(warn_msg);
                // File was written, DB is stale – still count as written
            }

            written += 1;
        }

        // Summary
        let summary = format!(
            "Written: {}, Skipped (already up-to-date): {}, Errors: {}",
            written, skipped, errors
        );
        tm.add_log(&worker_task_id, summary.clone()).await;

        // Determine final status
        if errors > 0 && written == 0 {
            tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                .await;
            tm.update_progress_text(&worker_task_id, format!("Failed: {}", summary))
                .await;
        } else if errors > 0 || !warnings.is_empty() {
            tm.update_task_status(&worker_task_id, TaskStatus::Completed)
                .await;
            tm.update_progress_text(
                &worker_task_id,
                format!("Completed with issues: {}", summary),
            )
            .await;
            if !warnings.is_empty() {
                tm.add_log(
                    &worker_task_id,
                    format!("Warnings: {}", warnings.join("; ")),
                )
                .await;
            }
        } else {
            tm.update_task_status(&worker_task_id, TaskStatus::Completed)
                .await;
            tm.update_progress_text(&worker_task_id, summary).await;
        }

        Ok(())
    });

    task_manager.set_join_handle(&task_id, join_handle).await;

    task_id
}
