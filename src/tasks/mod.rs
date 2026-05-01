//! Generic task management module
//!
//! Provides in-memory task tracking for background operations.
//! Supports multiple task types with unified progress tracking.
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

use crate::config::ServiceCredentials;
use crate::embeddings::serialize_embedding;
use crate::spotify::{client::SpotifyClient, sync_worker::SpotifySyncWorker};

// ============================================================
// TaskType — unified enum for all background operations
// ============================================================

/// Type of task operation
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum TaskType {
    /// Sync from a service (spotify, soundcloud, youtube)
    ServiceSync {
        service: String,
        operation: SyncOperation,
    },
    /// Write target comments to one or more files
    WriteComment { file_ids: Vec<i64> },
    /// Recompute ML embeddings for all tags
    RecomputeEmbeddings,
    /// Scan a monitored folder for new/changed files
    ScanFolder { folder_id: i64 },
    /// Import play stats (play_count, last_played, rating) from Traktor collection.nml
    TraktorImport {
        /// Optional custom path to collection.nml
        custom_path: Option<String>,
    },
}

/// What to sync for a service
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SyncOperation {
    /// Sync only playlist metadata (no tracks)
    Playlists,
    /// Sync tracks for a specific playlist
    TracksForPlaylist(String),
    /// Sync tracks for all playlists in the database
    TracksAll,
    /// Full sync: playlists + all tracks
    Full,
}

/// Backward compatibility alias — SyncType is now SyncOperation
pub type SyncType = SyncOperation;

// ============================================================
// Backward compatibility: SyncConfig bridges old SyncType → new SyncOperation
// ============================================================

/// Configuration for Spotify sync (kept for backward compatibility with old TaskType::SpotifySync)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncConfig {
    Playlists,
    TracksForPlaylist(String),
    TracksAll,
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
    /// Convert to old SyncType for backward compat with SpotifySyncWorker
    pub fn to_sync_type(&self) -> SyncType {
        match self {
            SyncConfig::Playlists => SyncType::Playlists,
            SyncConfig::TracksForPlaylist(id) => SyncType::TracksForPlaylist(id.clone()),
            SyncConfig::TracksAll => SyncType::TracksAll,
            SyncConfig::Full => SyncType::Full,
        }
    }

    /// Convert to the new SyncOperation
    pub fn to_sync_operation(&self) -> SyncOperation {
        match self {
            SyncConfig::Playlists => SyncOperation::Playlists,
            SyncConfig::TracksForPlaylist(id) => SyncOperation::TracksForPlaylist(id.clone()),
            SyncConfig::TracksAll => SyncOperation::TracksAll,
            SyncConfig::Full => SyncOperation::Full,
        }
    }
}

impl SyncOperation {
    /// Convert to old SyncType for backward compat
    pub fn to_sync_type(&self) -> SyncType {
        match self {
            SyncOperation::Playlists => SyncType::Playlists,
            SyncOperation::TracksForPlaylist(id) => SyncType::TracksForPlaylist(id.clone()),
            SyncOperation::TracksAll => SyncType::TracksAll,
            SyncOperation::Full => SyncType::Full,
        }
    }

    /// Convert to old SyncConfig for backward compat
    pub fn to_sync_config(&self) -> SyncConfig {
        SyncConfig::from(self.clone())
    }
}

// ============================================================
// TaskStatus — lifecycle states for all tasks
// ============================================================

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

// ============================================================
// Progress — unified progress tracking for all task types
// ============================================================

/// A sub-item within a task's progress (e.g. a single file in a batch write,
/// a single playlist in a sync operation)
#[derive(Clone, Debug, Serialize)]
pub struct ProgressItem {
    /// Human-readable label for this sub-item
    pub label: String,
    /// Status of this sub-item
    pub status: TaskStatus,
    /// Optional percentage (0–100)
    pub percent: Option<f32>,
    /// Human-readable message for this sub-item
    pub message: String,
}

/// Unified progress tracking for all task types.
///
/// Every task reports via this struct. Spotify sync tasks additionally have
/// the old `SyncProgress` for backward compatibility with `SpotifySyncWorker`,
/// which is converted to this format when serializing via `to_progress()`.
#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    /// Overall status
    pub status: TaskStatus,
    /// Optional overall percentage (0–100)
    pub percent: Option<f32>,
    /// Human-readable overall progress message
    pub message: String,
    /// Sub-items for granular progress (e.g. files, playlists)
    pub sub_items: Vec<ProgressItem>,
}

impl Progress {
    /// Create a new Progress in Pending state
    pub fn new(message: &str) -> Self {
        Self {
            status: TaskStatus::Pending,
            percent: None,
            message: message.to_string(),
            sub_items: Vec::new(),
        }
    }
}

// ============================================================
// SyncProgress — legacy detailed progress for Spotify sync worker
// ============================================================

/// Backward-compatible detailed sync progress for Spotify sync tasks.
/// Used by `SpotifySyncWorker` internally. Converted to unified `Progress`
/// when serializing via `to_progress()`.
#[derive(Clone, Debug, Serialize)]
pub struct SyncProgress {
    /// Type of sync operation
    pub sync_type: SyncType,
    /// Current status
    pub status: TaskStatus,
    // Playlist sync progress
    pub current_playlist: Option<usize>,
    pub total_playlists: Option<usize>,
    pub current_playlist_name: Option<String>,
    // Track sync progress
    pub current_track: Option<usize>,
    pub total_tracks: Option<usize>,
    pub current_track_name: Option<String>,
    pub current_playlist_for_tracks: Option<String>,
    // Log messages
    pub logs: VecDeque<String>,
    // Timing (not serialized)
    #[serde(skip)]
    pub started_at: Instant,
    #[serde(skip)]
    pub estimated_remaining: Option<std::time::Duration>,
}

impl SyncProgress {
    /// Create new progress for a sync type
    pub fn new(sync_type: SyncType) -> Self {
        Self {
            sync_type,
            status: TaskStatus::Pending,
            current_playlist: None,
            total_playlists: None,
            current_playlist_name: None,
            current_track: None,
            total_tracks: None,
            current_track_name: None,
            current_playlist_for_tracks: None,
            logs: VecDeque::new(),
            started_at: Instant::now(),
            estimated_remaining: None,
        }
    }

    /// Add a log message (keeps last 100 entries)
    pub fn add_log(&mut self, message: String) {
        self.logs.push_back(message);
        if self.logs.len() > 100 {
            self.logs.pop_front();
        }
    }

    /// Calculate progress percentage (0-100)
    pub fn percentage(&self) -> Option<f32> {
        match self.sync_type {
            SyncType::Playlists => {
                if let (Some(current), Some(total)) = (self.current_playlist, self.total_playlists)
                    && total > 0
                {
                    return Some((current as f32 / total as f32) * 100.0);
                }
            }
            SyncType::TracksForPlaylist(_) => {
                if let (Some(current), Some(total)) = (self.current_track, self.total_tracks)
                    && total > 0
                {
                    return Some((current as f32 / total as f32) * 100.0);
                }
            }
            SyncType::TracksAll | SyncType::Full => {
                // Combined progress for multi-stage syncs
                let playlist_progress = if let (Some(current), Some(total)) =
                    (self.current_playlist, self.total_playlists)
                {
                    if total > 0 {
                        (current as f32 / total as f32) * 0.3
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };

                let track_progress =
                    if let (Some(current), Some(total)) = (self.current_track, self.total_tracks) {
                        if total > 0 {
                            (current as f32 / total as f32) * 0.7
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };

                return Some((playlist_progress + track_progress) * 100.0);
            }
        }
        None
    }
}

/// Result of a sync operation
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncResult {
    pub playlist_count: usize,
    pub track_count: usize,
    pub playlist_names: Vec<String>,
    pub track_names: Vec<String>,
    pub error: Option<String>,
}

impl SyncResult {
    pub fn success(
        playlist_count: usize,
        track_count: usize,
        playlist_names: Vec<String>,
        track_names: Vec<String>,
    ) -> Self {
        Self {
            playlist_count,
            track_count,
            playlist_names,
            track_names,
            error: None,
        }
    }

    pub fn failed(error: String) -> Self {
        Self {
            playlist_count: 0,
            track_count: 0,
            playlist_names: Vec::new(),
            track_names: Vec::new(),
            error: Some(error),
        }
    }
}

// ============================================================
// Task — a single background task
// ============================================================

/// A background task with progress tracking
pub struct Task {
    /// Unique task ID
    pub id: String,
    /// Type of task operation
    pub task_type: TaskType,
    /// Current status
    pub status: Arc<std::sync::Mutex<TaskStatus>>,
    /// Service name (spotify, soundcloud, youtube) if applicable
    pub service: Option<String>,
    /// Human-readable progress text (kept for backward compat with existing callers)
    pub progress_text: Arc<std::sync::Mutex<String>>,
    /// Unified progress with percent + sub-items (used by all tasks)
    pub progress: Arc<RwLock<Progress>>,
    /// Detailed sync progress (only for Spotify sync tasks, kept for backward compat)
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

/// Derive a conflict key from a TaskType.
///
/// Tasks with the same conflict key cannot run concurrently.
/// Returns `None` for task types that have no uniqueness constraint.
///
/// | TaskType | Conflict Key | Constraint |
/// |---|---|---|
/// | `ServiceSync { service }` | `sync:{service}` | One sync per service at a time |
/// | `ScanFolder { folder_id }` | `scan:{folder_id}` | One scan per folder at a time |
/// | `RecomputeEmbeddings` | `embeddings` | Only one at a time |
/// | `WriteComment` | None | No constraint (can run concurrently) |
pub fn task_type_conflict_key(task_type: &TaskType) -> Option<String> {
    match task_type {
        TaskType::ServiceSync { service, .. } => Some(format!("sync:{}", service)),
        TaskType::ScanFolder { folder_id } => Some(format!("scan:{}", folder_id)),
        TaskType::RecomputeEmbeddings => Some("embeddings".to_string()),
        TaskType::WriteComment { .. } => None,
        TaskType::TraktorImport { .. } => Some("traktor_import".to_string()),
    }
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
            progress: Arc::new(RwLock::new(Progress::new("Pending"))),
            sync_progress: None,
            logs: Arc::new(std::sync::Mutex::new(VecDeque::new())),
            cancel_token: CancellationToken::new(),
            created_at: Instant::now(),
            join_handle: None,
        }
    }

    /// Create a new sync task with detailed SyncProgress (for Spotify sync backward compat)
    pub fn new_sync(service: String, sync_type: SyncType) -> Self {
        let id = Uuid::new_v4().to_string();
        let config = SyncConfig::from(sync_type.clone());
        let task_type = TaskType::ServiceSync {
            service: service.clone(),
            operation: config.to_sync_operation(),
        };
        let sync_progress = SyncProgress::new(sync_type);
        Self {
            id,
            task_type,
            status: Arc::new(std::sync::Mutex::new(TaskStatus::Pending)),
            service: Some(service),
            progress_text: Arc::new(std::sync::Mutex::new("Starting...".to_string())),
            progress: Arc::new(RwLock::new(Progress::new("Starting..."))),
            sync_progress: Some(Arc::new(RwLock::new(sync_progress))),
            logs: Arc::new(std::sync::Mutex::new(VecDeque::new())),
            cancel_token: CancellationToken::new(),
            created_at: Instant::now(),
            join_handle: None,
        }
    }

    /// Add a log message (keeps last 100 entries)
    pub fn add_log(&self, message: String) {
        let mut logs = self.logs.lock().unwrap_or_else(|e| e.into_inner());
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
        let (task_type_str, task_details) = self.task_type_display();
        let status = self
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let progress_text = self
            .progress_text
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let logs: Vec<String> = self
            .logs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect();

        // Derive percent and sub_items from unified progress or sync_progress (backward compat)
        let (percent, sub_items) = if let Some(ref sp) = self.sync_progress {
            // Convert old SyncProgress to unified format
            let sp = sp
                .try_read()
                .map(|p| p.clone())
                .unwrap_or(SyncProgress::new(SyncType::Playlists));
            let pct = sp.percentage();
            let mut items = Vec::new();
            if let Some(ref name) = sp.current_playlist_name {
                let item_pct = sp.current_playlist.zip(sp.total_playlists).map(|(c, t)| {
                    if t > 0 {
                        (c as f32 / t as f32) * 100.0
                    } else {
                        0.0
                    }
                });
                items.push(ProgressItem {
                    label: format!("Playlist: {}", name),
                    status: TaskStatus::Running,
                    percent: item_pct,
                    message: format!(
                        "{}/{} playlists",
                        sp.current_playlist.unwrap_or(0),
                        sp.total_playlists.unwrap_or(0)
                    ),
                });
            }
            (pct, items)
        } else {
            self.progress
                .try_read()
                .map(|p| (p.percent, p.sub_items.clone()))
                .unwrap_or((None, vec![]))
        };

        let created_at_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() - self.created_at.elapsed().as_secs_f64())
            .unwrap_or(0.0);

        TaskProgress {
            id: self.id.clone(),
            task_type: task_type_str,
            task_details,
            status,
            service: self.service.clone(),
            progress: progress_text,
            percent,
            sub_items,
            logs,
            created_at_secs,
        }
    }

    fn task_type_display(&self) -> (String, Option<TaskType>) {
        let task_details = Some(self.task_type.clone());
        let task_type_str = match &self.task_type {
            TaskType::ServiceSync { service, .. } => format!("{}_sync", service),
            TaskType::WriteComment { .. } => "write_comment".to_string(),
            TaskType::RecomputeEmbeddings => "recompute_embeddings".to_string(),
            TaskType::ScanFolder { .. } => "scan_folder".to_string(),
            TaskType::TraktorImport { .. } => "traktor_import".to_string(),
        };
        (task_type_str, task_details)
    }
}

// ============================================================
// TaskProgress — serializable snapshot for API responses
// ============================================================

/// Serializable progress snapshot for API responses
#[derive(Clone, Debug, Serialize)]
pub struct TaskProgress {
    pub id: String,
    /// Machine-readable task type string (e.g. "spotify_sync", "write_comment", "scan_folder")
    pub task_type: String,
    /// Full TaskType variant with details (serialized for frontend context)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_details: Option<TaskType>,
    pub status: TaskStatus,
    pub service: Option<String>,
    /// Human-readable progress text (legacy field, all tasks populate this)
    pub progress: String,
    /// Optional percentage (0–100) for progress bars
    pub percent: Option<f32>,
    /// Granular sub-items for detailed progress display
    pub sub_items: Vec<ProgressItem>,
    pub logs: Vec<String>,
    pub created_at_secs: f64,
}

// ============================================================
// TaskManager — in-memory task registry
// ============================================================

/// In-memory task manager
#[derive(Clone)]
pub struct TaskManager {
    /// Map of task_id -> Task
    tasks: Arc<RwLock<HashMap<String, Task>>>,
}

/// Error returned when a task cannot be started due to a conflict
#[derive(Debug, thiserror::Error)]
pub enum TaskConflictError {
    #[error("A task of this type is already running: {conflict_key}")]
    AlreadyRunning { conflict_key: String },
}

impl TaskManager {
    /// Create a new task manager
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new task unconditionally and return its ID.
    pub async fn start_task(&self, task: Task) -> String {
        let id = task.id.clone();
        let mut tasks = self.tasks.write().await;
        tasks.insert(id.clone(), task);
        id
    }

    /// Register a new task, rejecting if a task with the same conflict key is
    /// already running or pending.
    ///
    /// See [`task_type_conflict_key`] for which task types conflict.
    pub async fn start_task_unique(&self, task: Task) -> Result<String, TaskConflictError> {
        let conflict_key = task_type_conflict_key(&task.task_type);
        let id = task.id.clone();
        let mut tasks = self.tasks.write().await;
        if let Some(ref key) = conflict_key {
            for existing in tasks.values() {
                if let Some(existing_key) = task_type_conflict_key(&existing.task_type)
                    && &existing_key == key
                {
                    let status = existing
                        .status
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    if status == TaskStatus::Running || status == TaskStatus::Pending {
                        return Err(TaskConflictError::AlreadyRunning {
                            conflict_key: key.clone(),
                        });
                    }
                }
            }
        }
        tasks.insert(id.clone(), task);
        Ok(id)
    }

    /// Get a serializable snapshot of a task
    pub async fn get_task(&self, task_id: &str) -> Option<TaskProgress> {
        let tasks = self.tasks.read().await;
        tasks.get(task_id).map(|task| task.to_progress())
    }

    /// Cancel a task by ID
    pub async fn cancel_task(&self, task_id: &str) -> anyhow::Result<()> {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(task_id) {
            *task.status.lock().unwrap_or_else(|e| e.into_inner()) = TaskStatus::Cancelled;
            task.add_log("Task cancelled by user".to_string());
            task.cancel_token.cancel();
            Ok(())
        } else {
            Err(anyhow::anyhow!("Task not found: {}", task_id))
        }
    }

    /// List all tasks (returns serializable snapshots, most recent first)
    pub async fn list_tasks(&self) -> Vec<TaskProgress> {
        let tasks = self.tasks.read().await;
        let mut result: Vec<TaskProgress> = tasks.values().map(|t| t.to_progress()).collect();
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
            *task.status.lock().unwrap_or_else(|e| e.into_inner()) = status;
        }
    }

    /// Add a log message to a task
    pub async fn add_log(&self, task_id: &str, message: String) {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id) {
            task.add_log(message);
        }
    }

    /// Update the progress text of a task (legacy, prefer `update_progress`)
    pub async fn update_progress_text(&self, task_id: &str, text: String) {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id) {
            *task.progress_text.lock().unwrap_or_else(|e| e.into_inner()) = text;
        }
    }

    /// Update the unified Progress for a task.
    /// The closure receives a mutable reference to the task's Progress struct.
    pub async fn update_progress<F>(&self, task_id: &str, update_fn: F)
    where
        F: FnOnce(&mut Progress),
    {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id) {
            let mut progress = task.progress.write().await;
            update_fn(&mut progress);
        }
    }

    // ---- Sync-specific methods (for backward compatibility) ----

    /// Get detailed sync progress for a task (returns old SyncProgress format)
    pub async fn get_sync_progress(&self, task_id: &str) -> Option<SyncProgress> {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id)
            && let Some(ref sync_progress) = task.sync_progress
        {
            return Some(sync_progress.read().await.clone());
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

    /// Remove a task by ID
    pub async fn remove_task(&self, task_id: &str) {
        let mut tasks = self.tasks.write().await;
        tasks.remove(task_id);
    }

    /// Prune completed/failed/cancelled tasks older than the given duration.
    /// Call this periodically to prevent unbounded memory growth.
    pub async fn prune_old_tasks(&self, max_age: std::time::Duration) {
        let mut tasks = self.tasks.write().await;
        let now = Instant::now();
        tasks.retain(|_id, task| {
            let status = task
                .status
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let is_terminal = matches!(
                status,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
            );
            if is_terminal {
                let age = now - task.created_at;
                age <= max_age
            } else {
                true // keep running/pending tasks regardless of age
            }
        });
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Utility
// ============================================================

/// Derive a human-readable display label from a TaskType
pub fn task_type_label(task_type: &TaskType) -> String {
    match task_type {
        TaskType::ServiceSync { service, operation } => {
            let op_label = match operation {
                SyncOperation::Playlists => "playlists",
                SyncOperation::TracksForPlaylist(_) => "tracks (single playlist)",
                SyncOperation::TracksAll => "all tracks",
                SyncOperation::Full => "full sync",
            };
            format!("{} {}", service, op_label)
        }
        TaskType::WriteComment { file_ids } => {
            if file_ids.len() == 1 {
                "Write comment (1 file)".to_string()
            } else {
                format!("Write comment ({} files)", file_ids.len())
            }
        }
        TaskType::RecomputeEmbeddings => "Recompute embeddings".to_string(),
        TaskType::ScanFolder { folder_id } => format!("Scan folder #{}", folder_id),
        TaskType::TraktorImport { custom_path: _ } => "Import from Traktor".to_string(),
    }
}

// ============================================================
// Spotify sync task worker
// ============================================================

/// Start a Spotify sync task using the TaskManager.
/// Uses `start_task_unique` to prevent duplicate Spotify syncs.
pub async fn start_spotify_sync_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    credentials: &ServiceCredentials,
    sync_type: SyncType,
) -> anyhow::Result<String> {
    let service = "spotify".to_string();

    // Try to start uniquely — reject if a Spotify sync is already running/pending
    let task = Task::new_sync(service.clone(), sync_type.clone());
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let cancel_token = task.cancel_token.clone();
    let sync_progress = task.sync_progress.as_ref().unwrap().clone();

    task_manager
        .start_task_unique(task)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    info!("Starting Spotify sync with type: {:?}", sync_type);

    let tm = task_manager.clone();
    let db_clone = db.clone();
    let creds = credentials.clone();
    let sync_type_clone = sync_type.clone();

    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(
            &worker_task_id,
            format!("Starting {:?} sync...", sync_type_clone),
        )
        .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = format!("Starting {:?} sync...", sync_type_clone);
        })
        .await;

        let spotify_client = match SpotifyClient::from_stored_tokens(db_clone.clone(), &creds).await
        {
            Ok(client) => client,
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
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Failed;
                    p.message = format!("Failed: {}", e);
                })
                .await;
                return Err(anyhow::anyhow!("Failed to create Spotify client: {}", e));
            }
        };

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

                let (status, summary) = if result.error.is_some() {
                    (
                        TaskStatus::Failed,
                        format!("Sync failed: {}", result.error.unwrap()),
                    )
                } else {
                    (
                        TaskStatus::Completed,
                        format!(
                            "Sync completed: {} playlists, {} tracks",
                            result.playlist_count, result.track_count
                        ),
                    )
                };
                tm.update_task_status(&worker_task_id, status.clone()).await;
                tm.add_log(&worker_task_id, summary.clone()).await;
                tm.update_progress_text(&worker_task_id, summary.clone())
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = status;
                    p.message = summary;
                })
                .await;
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
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Failed;
                    p.message = format!("Failed: {}", e);
                })
                .await;
                Err(e)
            }
        }
    });

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
    let _cancel_token = task.cancel_token.clone();

    task_manager.start_task(task).await;

    let tm = task_manager.clone();
    let db_clone = db.clone();

    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(
            &worker_task_id,
            format!("Writing comment for {} file(s)...", file_ids.len()),
        )
        .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = format!("Writing comment for {} file(s)...", file_ids.len());
        })
        .await;

        let total = file_ids.len();
        let mut written = 0usize;
        let mut skipped = 0usize;
        let mut errors = 0usize;
        let mut warnings: Vec<String> = Vec::new();

        for (i, file_id) in file_ids.iter().enumerate() {
            // Check cancellation
            if let Some(ct) = tm.get_cancel_token(&worker_task_id).await
                && ct.is_cancelled()
            {
                tm.add_log(&worker_task_id, "Task cancelled".to_string())
                    .await;
                tm.update_task_status(&worker_task_id, TaskStatus::Cancelled)
                    .await;
                tm.update_progress_text(&worker_task_id, "Cancelled".to_string())
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Cancelled;
                    p.message = "Cancelled".to_string();
                })
                .await;
                return Ok(());
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
            let msg = format!("Writing file {}/{}: '{}'", i + 1, total, title_display);
            tm.update_progress_text(&worker_task_id, msg.clone()).await;
            tm.update_progress(&worker_task_id, |p| {
                p.percent = Some((i as f32 / total as f32) * 100.0);
                p.message = msg;
                p.sub_items.push(ProgressItem {
                    label: title_display.clone(),
                    status: TaskStatus::Running,
                    percent: None,
                    message: "Writing...".to_string(),
                });
            })
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
        let (final_status, final_msg) = if errors > 0 && written == 0 {
            (TaskStatus::Failed, format!("Failed: {}", summary))
        } else if errors > 0 || !warnings.is_empty() {
            (
                TaskStatus::Completed,
                format!("Completed with issues: {}", summary),
            )
        } else {
            (TaskStatus::Completed, summary)
        };

        tm.update_task_status(&worker_task_id, final_status.clone())
            .await;
        tm.update_progress_text(&worker_task_id, final_msg.clone())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = final_status;
            p.percent = Some(100.0);
            p.message = final_msg;
        })
        .await;

        if !warnings.is_empty() {
            tm.add_log(
                &worker_task_id,
                format!("Warnings: {}", warnings.join("; ")),
            )
            .await;
        }

        Ok(())
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    task_id
}

// ============================================================
// RecomputeEmbeddings worker
// ============================================================

/// Start a background task to recompute embeddings for all tags.
/// Loads the ML model, iterates over all tags, computes and stores embeddings.
/// Reports progress via the task system (visible in tasks UI).
pub async fn start_recompute_embeddings_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
) -> String {
    let task = Task::new(TaskType::RecomputeEmbeddings, None);
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let cancel_token = task.cancel_token.clone();

    task_manager.start_task(task).await;

    let tm = task_manager.clone();
    let db_clone = db.clone();

    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(&worker_task_id, "Loading ML model...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = "Loading ML model...".to_string();
        })
        .await;
        tm.add_log(
            &worker_task_id,
            "Starting embedding recompute...".to_string(),
        )
        .await;

        // Load embedding model
        let model = match crate::embeddings::EmbeddingModel::new() {
            Ok(m) => m,
            Err(e) => {
                let msg = format!("Failed to load model: {}", e);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, "Failed".to_string())
                    .await;
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Failed;
                    p.message = msg.clone();
                })
                .await;
                return Err(anyhow::anyhow!(msg));
            }
        };

        // Get all tags
        let tags = match sqlx::query_as::<_, crate::db::Tag>("SELECT * FROM tags ORDER BY name")
            .fetch_all(&db_clone)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                let msg = format!("Failed to fetch tags: {}", e);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, "Failed".to_string())
                    .await;
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Failed;
                    p.message = msg.clone();
                })
                .await;
                return Err(anyhow::anyhow!(msg));
            }
        };

        let total = tags.len();
        tm.add_log(
            &worker_task_id,
            format!("Found {} tags, computing embeddings...", total),
        )
        .await;

        // Clear old embeddings
        let _ = sqlx::query("DELETE FROM tag_embeddings")
            .execute(&db_clone)
            .await;

        let mut count = 0usize;
        for (i, tag) in tags.iter().enumerate() {
            // Check cancellation
            if cancel_token.is_cancelled() {
                tm.add_log(&worker_task_id, "Task cancelled by user".to_string())
                    .await;
                tm.update_progress_text(&worker_task_id, "Cancelled".to_string())
                    .await;
                tm.update_task_status(&worker_task_id, TaskStatus::Cancelled)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Cancelled;
                    p.message = "Cancelled".to_string();
                })
                .await;
                return Ok(());
            }

            match model.embed_text(&tag.name) {
                Ok(vec) => {
                    let blob = serialize_embedding(&vec);
                    let now = chrono::Utc::now().timestamp();
                    let _ = sqlx::query(
                        r#"
                        INSERT INTO tag_embeddings (tag_id, embedding, model_version, updated_at)
                        VALUES (?, ?, ?, ?)
                        ON CONFLICT(tag_id) DO UPDATE SET
                            embedding = excluded.embedding,
                            model_version = excluded.model_version,
                            updated_at = excluded.updated_at
                        "#,
                    )
                    .bind(tag.id)
                    .bind(&blob)
                    .bind("all-MiniLM-L6-v2")
                    .bind(now)
                    .execute(&db_clone)
                    .await;
                    count += 1;
                }
                Err(e) => {
                    tm.add_log(
                        &worker_task_id,
                        format!("Failed to embed tag '{}': {}", tag.name, e),
                    )
                    .await;
                }
            }

            // Update progress every 10 tags
            if i % 10 == 0 {
                let msg = format!("{}/{} tags embedded", i, total);
                tm.update_progress_text(&worker_task_id, msg.clone()).await;
                tm.update_progress(&worker_task_id, |p| {
                    p.percent = Some((i as f32 / total as f32) * 100.0);
                    p.message = msg;
                })
                .await;
            }
        }

        let msg = format!("Done: {}/{} embeddings computed", count, total);
        tm.add_log(&worker_task_id, msg.clone()).await;
        tm.update_progress_text(&worker_task_id, format!("Completed ({} embeddings)", count))
            .await;
        tm.update_task_status(&worker_task_id, TaskStatus::Completed)
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Completed;
            p.percent = Some(100.0);
            p.message = msg;
        })
        .await;

        Ok(())
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    task_id
}

// ============================================================
// ScanFolder worker
// ============================================================

/// Start a task to scan a monitored folder for new/changed files.
///
/// Uses the folder's configured scan settings (recursive, extensions, max_depth).
/// Reports progress via the task system and supports cancellation.
///
/// Uses `start_task` (not `start_task_unique`) so duplicate scans per folder are
/// prevented by the caller (`api.rs`) via the conflict key check.
pub async fn start_scan_folder_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    folder_id: i64,
) -> anyhow::Result<String> {
    let task = Task::new(TaskType::ScanFolder { folder_id }, Some("scan".to_string()));
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let cancel_token = task.cancel_token.clone();

    task_manager
        .start_task_unique(task)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let tm = task_manager.clone();
    let db_clone = db.clone();

    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(&worker_task_id, "Loading folder config...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = "Loading folder config...".to_string();
        })
        .await;

        // Fetch folder config from DB
        let folder = match crate::db::get_folder_by_id(&db_clone, folder_id).await {
            Ok(Some(f)) => f,
            Ok(None) => {
                let msg = format!("Folder #{} not found", folder_id);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, "Failed: folder not found".to_string())
                    .await;
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Failed;
                    p.message = msg.clone();
                })
                .await;
                return Err(anyhow::anyhow!(msg));
            }
            Err(e) => {
                let msg = format!("Error fetching folder #{}: {}", folder_id, e);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, "Failed: DB error".to_string())
                    .await;
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Failed;
                    p.message = msg.clone();
                })
                .await;
                return Err(anyhow::anyhow!(msg));
            }
        };

        let folder_path = folder.folder_path.clone();
        let scan_recursive = folder.scan_recursive;
        let fixed_extensions = folder.fixed_extensions;
        let file_extensions = folder.file_extensions;
        let max_depth = folder.max_depth;

        tm.add_log(&worker_task_id, format!("Scanning folder: {}", folder_path))
            .await;
        tm.update_progress_text(&worker_task_id, "Scanning...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.message = format!("Scanning: {}", folder_path);
        })
        .await;

        // Check cancellation before starting the scan
        if cancel_token.is_cancelled() {
            tm.add_log(&worker_task_id, "Task cancelled".to_string())
                .await;
            tm.update_task_status(&worker_task_id, TaskStatus::Cancelled)
                .await;
            tm.update_progress_text(&worker_task_id, "Cancelled".to_string())
                .await;
            tm.update_progress(&worker_task_id, |p| {
                p.status = TaskStatus::Cancelled;
                p.message = "Cancelled".to_string();
            })
            .await;
            return Ok(());
        }

        // Perform the actual scan
        let path = std::path::Path::new(&folder_path);
        match crate::db::scan_directory_with_config(
            &db_clone,
            path,
            scan_recursive,
            fixed_extensions,
            file_extensions,
            max_depth,
        )
        .await
        {
            Ok(file_count) => {
                // Update last_scanned timestamp
                let now = chrono::Utc::now().timestamp();
                let _ =
                    sqlx::query("UPDATE folders SET last_scanned = ?, updated_at = ? WHERE id = ?")
                        .bind(now)
                        .bind(now)
                        .bind(folder_id)
                        .execute(&db_clone)
                        .await;

                let msg = format!(
                    "Scan complete: {} files found in folder #{}",
                    file_count, folder_id
                );
                info!("{}", msg);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, msg.clone()).await;
                tm.update_task_status(&worker_task_id, TaskStatus::Completed)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Completed;
                    p.percent = Some(100.0);
                    p.message = msg;
                })
                .await;
            }
            Err(e) => {
                let msg = format!("Scan failed for folder #{}: {}", folder_id, e);
                error!("{}", msg);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, format!("Failed: {}", e))
                    .await;
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Failed;
                    p.message = msg.clone();
                })
                .await;
                return Err(anyhow::anyhow!(msg));
            }
        }

        Ok(())
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    Ok(task_id)
}

/// Start a task to import play stats from Traktor's collection.nml.
///
/// Finds the latest `collection.nml` under `~/Documents/Native Instruments/Traktor */`,
/// parses it, matches entries against the `files` table, and updates `play_count`,
/// `last_played`, and `rating`.
///
/// Uses `start_task_unique` so only one Traktor import can run at a time.
pub async fn start_traktor_import_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    custom_path: Option<String>,
) -> anyhow::Result<String> {
    let task = Task::new(
        TaskType::TraktorImport {
            custom_path: custom_path.clone(),
        },
        Some("import".to_string()),
    );
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let cancel_token = task.cancel_token.clone();

    task_manager
        .start_task_unique(task)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let tm = task_manager.clone();
    let db_clone = db.clone();

    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(&worker_task_id, "Starting Traktor import...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = "Starting Traktor import...".to_string();
        })
        .await;

        // Check cancellation before doing anything
        if cancel_token.is_cancelled() {
            tm.add_log(&worker_task_id, "Task cancelled".to_string())
                .await;
            tm.update_task_status(&worker_task_id, TaskStatus::Cancelled)
                .await;
            tm.update_progress_text(&worker_task_id, "Cancelled".to_string())
                .await;
            tm.update_progress(&worker_task_id, |p| {
                p.status = TaskStatus::Cancelled;
                p.message = "Cancelled".to_string();
            })
            .await;
            return Ok(());
        }

        // Resolve custom path
        let custom_path_ref = custom_path.as_ref().map(std::path::Path::new);

        tm.add_log(
            &worker_task_id,
            "Scanning for collection.nml...".to_string(),
        )
        .await;
        tm.update_progress_text(&worker_task_id, "Locating collection.nml...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.message = "Locating Traktor collection.nml...".to_string();
        })
        .await;

        // Run the import
        match crate::traktor::run_import(&db_clone, custom_path_ref).await {
            Ok((stats, nml_path)) => {
                let msg = format!(
                    "Import complete: {} entries parsed, {} matched, {} play counts, {} last played dates. Used: {}",
                    stats.total_entries,
                    stats.matched,
                    stats.updated_play_count,
                    stats.updated_last_played,
                    nml_path.display()
                );
                info!("{}", msg);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, msg.clone()).await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Completed;
                    p.percent = Some(100.0);
                    p.message = msg;
                })
                .await;
                tm.update_task_status(&worker_task_id, TaskStatus::Completed)
                    .await;
            }
            Err(e) => {
                let msg = format!("Traktor import failed: {}", e);
                error!("{}", msg);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, format!("Failed: {}", e))
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Failed;
                    p.message = msg.clone();
                })
                .await;
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                return Err(anyhow::anyhow!(msg));
            }
        }

        Ok(())
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    Ok(task_id)
}
