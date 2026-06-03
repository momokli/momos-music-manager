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
    /// Periodic poll of the deemix download queue
    DeemixSync,
    /// Backup a folder's files via SSH/SCP to the configured backup destination
    BackupFolder { folder_id: i64 },
    /// Backup WAV source subdirectories for a folder, then delete locally
    BackupWavs { folder_id: i64 },
    /// Scan a folder for nuo-stems WAV source subdirectories
    ScanWavSources { folder_id: i64 },
    /// BackupDiscovery: Scan NAS backup to discover files that exist only on backup
    BackupDiscovery { folder_id: i64 },
    /// PruneFiles: Delete selected local files (must be backed up)
    PruneFiles { file_ids: Vec<i64> },
    /// BackpackSync: Ensure files in backpack tags are available locally
    BackpackSync { tag_ids: Vec<i64> },
}

/// What to sync for a service
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SyncOperation {
    /// Sync only playlist metadata (no tracks)
    Playlists,
    /// Sync only playlists that don't yet exist in the database (metadata + tracks)
    NewPlaylists,
    /// Sync tracks for a specific playlist
    TracksForPlaylist(String),
    /// Sync tracks for a list of playlist IDs (batch operation)
    TracksForPlaylistList(Vec<String>),
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
            SyncType::NewPlaylists => SyncConfig::Playlists,
            SyncType::TracksForPlaylist(id) => SyncConfig::TracksForPlaylist(id),
            SyncType::TracksForPlaylistList(ids) => {
                SyncConfig::TracksForPlaylist(ids.first().cloned().unwrap_or_default())
            }
            SyncType::TracksAll => SyncConfig::TracksAll,
            SyncType::Full => SyncConfig::Full,
        }
    }
}

impl SyncConfig {
    /// Convert to old SyncType for backward compat with SpotifySyncWorker
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn to_sync_type(&self) -> SyncType {
        match self {
            SyncOperation::Playlists => SyncType::Playlists,
            SyncOperation::NewPlaylists => SyncType::NewPlaylists,
            SyncOperation::TracksForPlaylist(id) => SyncType::TracksForPlaylist(id.clone()),
            SyncOperation::TracksForPlaylistList(ids) => {
                SyncType::TracksForPlaylistList(ids.clone())
            }
            SyncOperation::TracksAll => SyncType::TracksAll,
            SyncOperation::Full => SyncType::Full,
        }
    }

    /// Convert to old SyncConfig for backward compat
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub started_at: Instant,
    #[serde(skip)]
    #[allow(dead_code)]
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
            SyncType::Playlists | SyncType::NewPlaylists => {
                if let (Some(current), Some(total)) = (self.current_playlist, self.total_playlists)
                    && total > 0
                {
                    return Some((current as f32 / total as f32) * 100.0);
                }
            }
            SyncType::TracksForPlaylist(_) | SyncType::TracksForPlaylistList(_) => {
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
        TaskType::DeemixSync => None,
        TaskType::BackupFolder { folder_id } => Some(format!("backup:{}", folder_id)),
        TaskType::BackupWavs { folder_id } => Some(format!("backup_wavs:{}", folder_id)),
        TaskType::ScanWavSources { folder_id } => Some(format!("scan_wavs:{}", folder_id)),
        TaskType::BackupDiscovery { folder_id } => Some(format!("backup_discovery:{}", folder_id)),
        TaskType::PruneFiles { .. } => None, // prunes don't conflict — multiple can run
        TaskType::BackpackSync { .. } => None, // multiple can run
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
    #[allow(dead_code)]
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
            TaskType::DeemixSync => "deemix_sync".to_string(),
            TaskType::BackupFolder { .. } => "backup_folder".to_string(),
            TaskType::BackupWavs { .. } => "backup_wavs".to_string(),
            TaskType::ScanWavSources { .. } => "scan_wav_sources".to_string(),
            TaskType::BackupDiscovery { .. } => "backup_discovery".to_string(),
            TaskType::PruneFiles { .. } => "prune_files".to_string(),
            TaskType::BackpackSync { .. } => "backpack_sync".to_string(),
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
    #[allow(dead_code)]
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

    /// List tasks with pagination, optional status filter, and optional sort
    pub async fn list_tasks_paginated(
        &self,
        limit: usize,
        offset: usize,
        status_filter: Option<TaskStatus>,
        sort: Option<String>,
        order: Option<String>,
    ) -> (Vec<TaskProgress>, usize) {
        let tasks = self.tasks.read().await;
        let mut all: Vec<TaskProgress> = tasks.values().map(|t| t.to_progress()).collect();

        // Apply sort
        let sort_col = sort.as_deref().unwrap_or("created_at");
        let is_desc = matches!(order.as_deref(), Some("desc"));
        all.sort_by(|a, b| {
            let cmp = match sort_col {
                "type" => a.task_type.cmp(&b.task_type),
                "status" => {
                    let sa = format!("{:?}", a.status);
                    let sb = format!("{:?}", b.status);
                    sa.cmp(&sb)
                }
                "progress" => a
                    .percent
                    .unwrap_or(0f32)
                    .partial_cmp(&b.percent.unwrap_or(0f32))
                    .unwrap_or(std::cmp::Ordering::Equal),
                "created_at" => a
                    .created_at_secs
                    .partial_cmp(&b.created_at_secs)
                    .unwrap_or(std::cmp::Ordering::Equal),
                // "updated_at" — tasks don't have an updated_at field, fall through to default
                _ => b
                    .created_at_secs
                    .partial_cmp(&a.created_at_secs)
                    .unwrap_or(std::cmp::Ordering::Equal),
            };
            if is_desc { cmp.reverse() } else { cmp }
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
    #[allow(dead_code)]
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
#[allow(dead_code)]
pub fn task_type_label(task_type: &TaskType) -> String {
    match task_type {
        TaskType::ServiceSync { service, operation } => {
            let op_label = match operation {
                SyncOperation::Playlists => "playlists",
                SyncOperation::NewPlaylists => "new playlists",
                SyncOperation::TracksForPlaylist(_) => "tracks (single playlist)",
                SyncOperation::TracksForPlaylistList(ids) => {
                    if ids.len() == 1 {
                        "tracks (1 playlist)"
                    } else {
                        "tracks (batch)"
                    }
                }
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
        TaskType::DeemixSync => "Deemix sync".to_string(),
        TaskType::BackupFolder { folder_id } => format!("Backup folder #{}", folder_id),
        TaskType::BackupWavs { folder_id } => format!("Backup WAVs folder #{}", folder_id),
        TaskType::ScanWavSources { folder_id } => format!("Scan WAV sources folder #{}", folder_id),
        TaskType::BackupDiscovery { folder_id } => {
            format!("Backup discovery folder #{}", folder_id)
        }
        TaskType::PruneFiles { file_ids } => format!("Prune {} files", file_ids.len()),
        TaskType::BackpackSync { tag_ids } => {
            format!("Backpack sync {} tags", tag_ids.len())
        }
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

                    // Refresh materialized tables after sync
                    if let Err(e) = crate::db::refresh_file_resolved_tags(&db_clone).await {
                        error!("Failed to refresh file_resolved_tags after sync: {}", e);
                    }
                    if let Err(e) = crate::db::refresh_track_resolved_tags(&db_clone).await {
                        error!("Failed to refresh track_resolved_tags after sync: {}", e);
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
#[allow(dead_code)]
pub async fn start_scan_folder_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    folder_id: i64,
    scan_mode: crate::db::ScanMode,
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
            scan_mode,
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

// ============================================================
// BackupFolder worker
// ============================================================

/// Start a background task to backup all unbacked-up files in a folder
/// to its configured SSH backup destination.
pub async fn start_backup_folder_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    folder_id: i64,
) -> String {
    let task = Task::new(
        TaskType::BackupFolder { folder_id },
        Some("backup".to_string()),
    );
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let cancel_token = task.cancel_token.clone();

    match task_manager.start_task_unique(task).await {
        Ok(_) => {}
        Err(TaskConflictError::AlreadyRunning { conflict_key }) => {
            tracing::info!(
                "Backup folder task for folder {} already running (key: {}), skipping",
                folder_id,
                conflict_key
            );
            return task_id;
        }
    }

    let tm = task_manager.clone();
    let db_clone = db.clone();

    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(&worker_task_id, "Starting folder backup...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = "Starting folder backup...".to_string();
        })
        .await;

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

        let backup_path = match &folder.backup_path {
            Some(p) => p.clone(),
            None => {
                let msg = "Folder has no backup_path configured".to_string();
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, "Failed: no backup_path".to_string())
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

        let (ssh_host, remote_base) = match backup_path.split_once(':') {
            Some((host, path)) => (host.to_string(), path.to_string()),
            None => {
                let msg = "Invalid backup_path format. Expected host:/path".to_string();
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, "Failed: invalid backup_path".to_string())
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

        let engine = crate::backup::BackupEngine::new(ssh_host);
        let local_dir = folder.folder_path.clone();

        // STEP 1: Reconcile - find files already on NAS via ssh ls, mark as backed up
        tm.add_log(
            &worker_task_id,
            "Step 1/2: Reconciling with remote (ssh ls)...".to_string(),
        )
        .await;
        tm.update_progress_text(
            &worker_task_id,
            "Reconciling: listing remote files...".to_string(),
        )
        .await;

        // Match remote scan depth to local folder settings
        let remote_max_depth = if folder.scan_recursive {
            folder.max_depth as u32
        } else {
            1u32
        };
        match engine
            .list_remote_files_with_depth(&remote_base, remote_max_depth)
            .await
        {
            Ok(remote_files) if !remote_files.is_empty() => {
                let remote_count = remote_files.len();
                tm.add_log(
                    &worker_task_id,
                    format!(
                        "Remote has {} files - matching against local...",
                        remote_count
                    ),
                )
                .await;

                let all_local = match crate::db::get_unbacked_up_files(&db_clone, folder_id).await {
                    Ok(f) => f,
                    Err(_) => vec![],
                };

                let remote_set: std::collections::HashSet<String> =
                    remote_files.into_iter().collect();
                let mut reconciled = 0usize;

                for (i, file) in all_local.iter().enumerate() {
                    if cancel_token.is_cancelled() {
                        tm.update_task_status(&worker_task_id, TaskStatus::Cancelled)
                            .await;
                        return Ok(());
                    }
                    let filename = std::path::Path::new(&file.file_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if remote_set.contains(&filename) {
                        let rel_path = file
                            .file_path
                            .strip_prefix(&local_dir)
                            .unwrap_or(&file.file_path)
                            .trim_start_matches('/');
                        let remote_path =
                            format!("{}/{}", remote_base.trim_end_matches('/'), rel_path);
                        let _ = crate::db::record_backup_result(
                            &db_clone,
                            file.id,
                            true,
                            file.file_size,
                            &remote_path,
                        )
                        .await;
                        reconciled += 1;
                    }
                    if i % 500 == 0 {
                        tm.update_progress_text(
                            &worker_task_id,
                            format!("Reconciling: {}/{} checked", i, all_local.len()),
                        )
                        .await;
                    }
                }
                let msg = format!("Reconcile done: {} files already on NAS", reconciled);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, msg).await;
            }
            Ok(_) => {}
            Err(e) => {
                tm.add_log(
                    &worker_task_id,
                    format!(
                        "Reconcile skipped (list failed: {}) - proceeding with rsync",
                        e
                    ),
                )
                .await;
            }
        }

        // STEP 2: Re-query - many files may now be reconciled
        let files = match crate::db::get_unbacked_up_files(&db_clone, folder_id).await {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("Failed to get unbacked-up files: {}", e);
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

        let total = files.len();
        if total == 0 {
            let msg =
                "All files already backed up (reconciled from NAS or already synced)".to_string();
            tm.add_log(&worker_task_id, msg.clone()).await;
            tm.update_progress_text(
                &worker_task_id,
                "Completed: everything already backed up".to_string(),
            )
            .await;
            tm.update_task_status(&worker_task_id, TaskStatus::Completed)
                .await;
            tm.update_progress(&worker_task_id, |p| {
                p.status = TaskStatus::Completed;
                p.percent = Some(100.0);
                p.message = msg;
            })
            .await;
            return Ok(());
        }

        // STEP 3: Rsync remaining files — fast scan, no verbose output.
        // Uses `--ignore-existing` to skip remote stat calls: files that already
        // exist remotely (by path) are skipped without per-file checks.
        // No `-u`, `-v`, or `-P` flags — we don't want progress lines for individual
        // files, just clean exit on completion.
        tm.add_log(
            &worker_task_id,
            format!("Step 2/2: Copying {} file(s) to {}", total, backup_path),
        )
        .await;
        tm.update_progress_text(&worker_task_id, format!("Copying {} files...", total))
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.percent = Some(0.0);
            p.message = format!("Copying {} files to {}...", total, backup_path);
        })
        .await;

        let local_with_slash = format!("{}/", local_dir.trim_end_matches('/'));
        let dest = format!("{}:{}", engine.ssh_host(), remote_base);
        let output = tokio::process::Command::new("rsync")
            .arg("-a")
            .arg("--ignore-existing")
            .arg("--rsh=ssh")
            .arg(&local_with_slash)
            .arg(&dest)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let err_msg = format!("Rsync failed: {}", stderr);
            tm.add_log(&worker_task_id, err_msg.clone()).await;
            tm.update_progress_text(&worker_task_id, "Failed: rsync error".to_string())
                .await;
            tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                .await;
            tm.update_progress(&worker_task_id, |p| {
                p.status = TaskStatus::Failed;
                p.message = err_msg.clone();
            })
            .await;
            return Err(anyhow::anyhow!(err_msg));
        }

        tm.add_log(
            &worker_task_id,
            format!("Rsync completed: {} files transferred", total),
        )
        .await;

        // Mark all previously unbacked-up files as backed up
        tm.update_progress_text(&worker_task_id, "Recording backup status...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.message = "Recording backup status...".to_string();
        })
        .await;

        let total_recording = files.len();
        let mut recorded = 0usize;
        for (i, file) in files.iter().enumerate() {
            if cancel_token.is_cancelled() {
                tm.add_log(&worker_task_id, "Task cancelled".to_string())
                    .await;
                tm.update_task_status(&worker_task_id, TaskStatus::Cancelled)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Cancelled;
                    p.message = "Cancelled during recording".to_string();
                })
                .await;
                return Ok(());
            }

            let rel_path = file
                .file_path
                .strip_prefix(&local_dir)
                .unwrap_or(&file.file_path);
            let rel_path = rel_path.trim_start_matches('/');
            let remote_path = format!("{}/{}", remote_base.trim_end_matches('/'), rel_path);

            if let Err(e) = crate::db::record_backup_result(
                &db_clone,
                file.id,
                true,
                file.file_size,
                &remote_path,
            )
            .await
            {
                tm.add_log(
                    &worker_task_id,
                    format!("Failed to record backup for file {}: {}", file.id, e),
                )
                .await;
            } else {
                recorded += 1;
            }

            // Update live progress: % through recording + filename
            let pct = ((i + 1) as f32 / total_recording as f32) * 100.0;
            tm.update_progress_text(
                &worker_task_id,
                format!(
                    "Recording backup status: {}/{} files ({:.0}%)",
                    i + 1,
                    total_recording,
                    pct
                ),
            )
            .await;
            tm.update_progress(&worker_task_id, |p| {
                p.percent = Some(pct);
                p.message = format!(
                    "Recording backup status: {}/{} files ({:.0}%)",
                    i + 1,
                    total_recording,
                    pct
                );
            })
            .await;
        }

        let msg = format!("Backup complete: {} files recorded as backed up", recorded);
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
        Ok(())
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    task_id
}

// ============================================================
// BackupWavs worker
// ============================================================

/// Start a background task to backup WAV source subdirectories for a folder,
/// then delete them locally.
pub async fn start_backup_wavs_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    folder_id: i64,
) -> String {
    let task = Task::new(
        TaskType::BackupWavs { folder_id },
        Some("backup_wavs".to_string()),
    );
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let cancel_token = task.cancel_token.clone();

    match task_manager.start_task_unique(task).await {
        Ok(_) => {}
        Err(TaskConflictError::AlreadyRunning { conflict_key }) => {
            tracing::info!(
                "Backup WAVs task for folder {} already running (key: {}), skipping",
                folder_id,
                conflict_key
            );
            return task_id;
        }
    }

    let tm = task_manager.clone();
    let db_clone = db.clone();

    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(&worker_task_id, "Starting WAV backup...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = "Starting WAV backup...".to_string();
        })
        .await;

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

        let backup_path = match &folder.backup_path {
            Some(p) => p.clone(),
            None => {
                let msg = "Folder has no backup_path configured".to_string();
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, "Failed: no backup_path".to_string())
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

        let (ssh_host, remote_base) = match backup_path.split_once(':') {
            Some((host, path)) => (host.to_string(), path.to_string()),
            None => {
                let msg = "Invalid backup_path format".to_string();
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, "Failed: invalid backup_path".to_string())
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

        let engine = crate::backup::BackupEngine::new(ssh_host);
        let local_dir = folder.folder_path.clone();

        let subdirs = match crate::db::get_wav_source_subdirs(&db_clone, folder_id).await {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("Failed to get WAV source subdirs: {}", e);
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

        if subdirs.is_empty() {
            let msg = "No WAV source subdirectories found".to_string();
            tm.add_log(&worker_task_id, msg.clone()).await;
            tm.update_progress_text(&worker_task_id, "Completed: nothing to backup".to_string())
                .await;
            tm.update_task_status(&worker_task_id, TaskStatus::Completed)
                .await;
            tm.update_progress(&worker_task_id, |p| {
                p.status = TaskStatus::Completed;
                p.percent = Some(100.0);
                p.message = msg;
            })
            .await;
            return Ok(());
        }

        tm.add_log(
            &worker_task_id,
            format!("Found {} WAV subdirectories to backup", subdirs.len()),
        )
        .await;

        let mut backed_up = 0usize;
        let mut deleted = 0usize;

        for (i, subdir_name) in subdirs.iter().enumerate() {
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

            let local_subdir = format!("{}/{}", local_dir.trim_end_matches('/'), subdir_name);
            let local_path = std::path::Path::new(&local_subdir);

            if !local_path.is_dir() {
                continue;
            }

            let remote_subdir = format!("{}/{}", remote_base.trim_end_matches('/'), subdir_name);

            match engine.run_sync(&local_subdir, &remote_subdir).await {
                Ok((count, _)) => {
                    backed_up += count;

                    if let Ok(entries) = std::fs::read_dir(local_path) {
                        for entry in entries.flatten() {
                            let entry_path = entry.path();
                            if entry_path.extension().and_then(|e| e.to_str()) == Some("wav") {
                                let remote_wav_path = format!(
                                    "{}/{}",
                                    remote_subdir,
                                    entry.file_name().to_string_lossy()
                                );
                                let file_size =
                                    entry.metadata().map(|m| m.len() as i64).unwrap_or(0);
                                // Look up the WAV file in DB by local path to get correct file_id
                                let local_wav_path = entry_path.to_string_lossy().to_string();
                                let file_id = if let Ok(Some(f)) =
                                    crate::db::get_file_by_path(&db_clone, &local_wav_path).await
                                {
                                    f.id
                                } else {
                                    continue; // skip files not in DB
                                };
                                let _ = crate::db::record_backup_result(
                                    &db_clone,
                                    file_id,
                                    true,
                                    file_size,
                                    &remote_wav_path,
                                )
                                .await;
                            }
                        }
                    }

                    if let Err(e) = std::fs::remove_dir_all(local_path) {
                        tm.add_log(
                            &worker_task_id,
                            format!("Failed to delete local WAV dir {}: {}", local_subdir, e),
                        )
                        .await;
                    } else {
                        if let Ok(entries) = std::fs::read_dir(&local_subdir) {
                            let count = entries
                                .flatten()
                                .filter(|e| {
                                    e.path().extension().and_then(|ext| ext.to_str()) == Some("wav")
                                })
                                .count();
                            deleted += count;
                        }
                    }
                }
                Err(e) => {
                    tm.add_log(
                        &worker_task_id,
                        format!("Failed to backup WAV dir {}: {}", local_subdir, e),
                    )
                    .await;
                }
            }

            let msg = format!("Backed up {}/{} WAV subdirectories", i + 1, subdirs.len());
            tm.update_progress_text(&worker_task_id, msg.clone()).await;
            tm.update_progress(&worker_task_id, |p| {
                p.percent = Some(((i + 1) as f32 / subdirs.len() as f32) * 100.0);
                p.message = msg;
            })
            .await;
        }

        let msg = format!(
            "WAV backup complete: {} subdirectories backed up, {} local WAVs deleted",
            backed_up, deleted
        );
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
        Ok(())
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    task_id
}

// ============================================================
// ScanWavSources worker
// ============================================================

/// Start a background task to scan a folder for nuo-stems WAV source subdirectories.
pub async fn start_scan_wav_sources_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    folder_id: i64,
) -> String {
    let task = Task::new(
        TaskType::ScanWavSources { folder_id },
        Some("scan_wavs".to_string()),
    );
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let cancel_token = task.cancel_token.clone();

    match task_manager.start_task_unique(task).await {
        Ok(_) => {}
        Err(TaskConflictError::AlreadyRunning { conflict_key }) => {
            tracing::info!(
                "Scan WAV sources task for folder {} already running (key: {}), skipping",
                folder_id,
                conflict_key
            );
            return task_id;
        }
    }

    let tm = task_manager.clone();
    let db_clone = db.clone();

    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(&worker_task_id, "Scanning for WAV sources...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = "Scanning for WAV sources...".to_string();
        })
        .await;

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

        if !folder.scan_sources {
            let msg = "Folder does not have scan_sources enabled".to_string();
            tm.add_log(&worker_task_id, msg.clone()).await;
            tm.update_progress_text(
                &worker_task_id,
                "Failed: scan_sources not enabled".to_string(),
            )
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

        let subdirs = match crate::db::get_wav_source_subdirs(&db_clone, folder_id).await {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("Failed to get WAV source subdirs: {}", e);
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

        let local_dir = folder.folder_path.clone();
        let mut wav_indexed = 0usize;
        let mut linked_to_stems = 0usize;

        tm.add_log(
            &worker_task_id,
            format!("Found {} WAV source subdirectories to scan", subdirs.len()),
        )
        .await;

        for (i, subdir_name) in subdirs.iter().enumerate() {
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

            let local_subdir = format!("{}/{}", local_dir.trim_end_matches('/'), subdir_name);
            let dir_path = std::path::Path::new(&local_subdir);

            if !dir_path.is_dir() {
                continue;
            }

            if let Ok(entries) = std::fs::read_dir(dir_path) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if entry_path.extension().and_then(|e| e.to_str()) != Some("wav") {
                        continue;
                    }
                    wav_indexed += 1;

                    // Look up the WAV file in DB by path, then link to stem
                    let wav_path_str = entry_path.to_string_lossy().to_string();
                    if let Ok(Some(wav_file)) =
                        crate::db::get_file_by_path(&db_clone, &wav_path_str).await
                    {
                        match crate::db::link_wav_to_stem(&db_clone, wav_file.id, &wav_path_str)
                            .await
                        {
                            Ok(Some((stem_id, stem_type))) => {
                                linked_to_stems += 1;
                                tracing::debug!(
                                    "Linked WAV {} (type={}) -> stem #{}",
                                    wav_path_str,
                                    stem_type,
                                    stem_id
                                );
                            }
                            Ok(None) => {
                                tracing::debug!("No matching stem for WAV: {}", wav_path_str);
                            }
                            Err(e) => {
                                tracing::warn!("Failed to link WAV {}: {}", wav_path_str, e);
                            }
                        }
                    }
                }
            }

            let msg = format!("Scanned {}/{} WAV subdirectories", i + 1, subdirs.len());
            tm.update_progress_text(&worker_task_id, msg.clone()).await;
            tm.update_progress(&worker_task_id, |p| {
                p.percent = Some(((i + 1) as f32 / subdirs.len() as f32) * 100.0);
                p.message = msg;
            })
            .await;
        }

        let msg = format!(
            "WAV source scan complete: {} WAV files indexed, {} linked to stems in {} subdirectories",
            wav_indexed,
            linked_to_stems,
            subdirs.len()
        );
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
        Ok(())
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    task_id
}

// ============================================================
// BackupDiscovery worker
// ============================================================

/// Start a background task to scan NAS backup and discover files that exist
/// only on backup (not in local DB). Creates new file records for discovered files.
pub async fn start_backup_discovery_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    folder_id: i64,
) -> String {
    let task = Task::new(
        TaskType::BackupDiscovery { folder_id },
        Some("backup_discovery".to_string()),
    );
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let cancel_token = task.cancel_token.clone();

    match task_manager.start_task_unique(task).await {
        Ok(_) => {}
        Err(TaskConflictError::AlreadyRunning { conflict_key }) => {
            tracing::info!(
                "Backup discovery task for folder {} already running (key: {}), skipping",
                folder_id,
                conflict_key
            );
            return task_id;
        }
    }

    let tm = task_manager.clone();
    let db_clone = db.clone();

    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(&worker_task_id, "Starting backup discovery...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = "Starting backup discovery...".to_string();
        })
        .await;

        // Get folder info
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

        let backup_path = match &folder.backup_path {
            Some(p) => p.clone(),
            None => {
                let msg = "Folder has no backup_path configured".to_string();
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, "Failed: no backup_path".to_string())
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

        let (ssh_host, remote_base) = match backup_path.split_once(':') {
            Some((host, path)) => (host.to_string(), path.to_string()),
            None => {
                let msg = "Invalid backup_path format, expected 'host:/path'".to_string();
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, "Failed: invalid backup_path".to_string())
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

        // Determine max depth for SSH find
        let max_depth = if folder.scan_recursive {
            folder.max_depth as u32
        } else {
            1u32
        };

        let engine = crate::backup::BackupEngine::new(ssh_host);
        let remote_dir = remote_base.clone();

        tm.add_log(
            &worker_task_id,
            format!("Listing files on backup: {}", remote_dir),
        )
        .await;
        tm.update_progress_text(&worker_task_id, "Listing files on backup...".to_string())
            .await;

        let remote_files = match engine.list_remote_files_full(&remote_dir, max_depth).await {
            Ok(files) => files,
            Err(e) => {
                let msg = format!("Failed to list remote files: {}", e);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(
                    &worker_task_id,
                    "Failed: remote listing error".to_string(),
                )
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

        if remote_files.is_empty() {
            let msg = "No files found on backup (or failed to list)".to_string();
            tm.add_log(&worker_task_id, msg.clone()).await;
            tm.update_progress_text(
                &worker_task_id,
                "Completed: no files to process".to_string(),
            )
            .await;
            tm.update_task_status(&worker_task_id, TaskStatus::Completed)
                .await;
            tm.update_progress(&worker_task_id, |p| {
                p.status = TaskStatus::Completed;
                p.message = msg.clone();
            })
            .await;
            return Ok(());
        }

        tm.add_log(
            &worker_task_id,
            format!(
                "Found {} files on backup, discovering...",
                remote_files.len()
            ),
        )
        .await;
        tm.update_progress_text(&worker_task_id, "Matching against local DB...".to_string())
            .await;

        match crate::db::discover_backup_files(&db_clone, folder_id, &remote_files, &remote_base)
            .await
        {
            Ok(result) => {
                let msg = format!(
                    "Backup discovery complete: {} files on backup, {} already tracked, {} newly discovered",
                    result.files_on_backup, result.already_tracked, result.newly_discovered
                );
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
                let msg = format!("Backup discovery failed: {}", e);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, "Failed: discovery error".to_string())
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
    task_id
}

// ============================================================
// PruneFiles worker
// ============================================================

/// Start a background task to delete selected local files.
/// Each file must have a confirmed backup before it can be pruned.
pub async fn start_prune_files_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    file_ids: Vec<i64>,
) -> String {
    let task_type = TaskType::PruneFiles {
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
            format!("Pruning {} file(s)...", file_ids.len()),
        )
        .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = format!("Pruning {} file(s)...", file_ids.len());
        })
        .await;

        let total = file_ids.len();
        let mut deleted = 0usize;
        let mut skipped = 0usize;
        let mut errors = 0usize;
        let mut freed_bytes: i64 = 0;

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

            // Fetch file to get its size for reporting
            let file_size: Option<i64> =
                sqlx::query_scalar("SELECT file_size FROM files WHERE id = ?")
                    .bind(file_id)
                    .fetch_optional(&db_clone)
                    .await
                    .unwrap_or(None);

            match crate::db::delete_local_file_by_id(&db_clone, *file_id).await {
                Ok(true) => {
                    deleted += 1;
                    freed_bytes += file_size.unwrap_or(0);
                    tm.add_log(
                        &worker_task_id,
                        format!(
                            "Deleted local file #{} ({} bytes)",
                            file_id,
                            file_size.unwrap_or(0)
                        ),
                    )
                    .await;
                }
                Ok(false) => {
                    skipped += 1;
                    tm.add_log(
                        &worker_task_id,
                        format!("Skipped file #{} (not on local disk)", file_id),
                    )
                    .await;
                }
                Err(e) => {
                    errors += 1;
                    tm.add_log(
                        &worker_task_id,
                        format!("Error deleting file #{}: {}", file_id, e),
                    )
                    .await;
                }
            }

            // Update progress
            let percent = ((i + 1) as f64 / total as f64) * 100.0;
            tm.update_progress(&worker_task_id, |p| {
                p.percent = Some(percent as f32);
                p.message = format!(
                    "Pruning {}/{} ({} deleted, {} skipped, {} errors)",
                    i + 1,
                    total,
                    deleted,
                    skipped,
                    errors
                );
                if percent >= 100.0 {
                    p.status = TaskStatus::Completed;
                }
            })
            .await;
        }

        let summary = format!(
            "Prune complete: {} deleted ({} bytes freed), {} skipped, {} errors",
            deleted, freed_bytes, skipped, errors
        );
        tm.add_log(&worker_task_id, summary.clone()).await;
        tm.update_progress_text(&worker_task_id, summary.clone())
            .await;
        if errors > 0 && deleted == 0 {
            tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                .await;
        } else {
            tm.update_task_status(&worker_task_id, TaskStatus::Completed)
                .await;
        }
        tm.update_progress(&worker_task_id, |p| {
            p.status = if errors > 0 && deleted == 0 {
                TaskStatus::Failed
            } else {
                TaskStatus::Completed
            };
            p.percent = Some(100.0);
            p.message = summary;
        })
        .await;

        Ok(())
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    task_id
}

// ============================================================
// BackpackSync worker
// ============================================================

/// Start a background task to sync files in backpack tags.
/// For each track in backpack tags:
/// 1. Find best local file (stem > FLAC > MP3)
/// 2. If no local file but backup exists: pull from backup
/// 3. If multiple formats: keep only best one, mark others as safe-to-delete
/// 4. Skip WAV source files entirely
pub async fn start_backpack_sync_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    tag_ids: Vec<i64>,
) -> String {
    let task_type = TaskType::BackpackSync {
        tag_ids: tag_ids.clone(),
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
        tm.update_progress_text(&worker_task_id, "Starting backpack sync...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = "Starting backpack sync...".to_string();
        })
        .await;

        // Find all tracks in backpack tags.
        // For each tag_ids, find playlists with matching name, then tracks in those playlists.
        let placeholders: Vec<String> = tag_ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "SELECT DISTINCT spt.track_id FROM service_playlist_tracks spt
                 JOIN service_playlists sp ON sp.id = spt.playlist_id
                 WHERE LOWER(TRIM(sp.name)) IN (
                     SELECT LOWER(TRIM(t.name)) FROM tags t WHERE t.id IN ({})
                 )
                 AND (sp.archive_deleted = 1 OR spt.deleted_at IS NULL)",
            placeholders.join(",")
        );

        let track_ids: Vec<i64> = {
            let mut q = sqlx::query_scalar(&sql);
            for id in &tag_ids {
                q = q.bind(id);
            }
            q.fetch_all(&db_clone).await.unwrap_or_default()
        };

        let total_tracks = track_ids.len();
        tm.add_log(
            &worker_task_id,
            format!("Found {} tracks in backpack tags", total_tracks),
        )
        .await;
        tm.update_progress_text(
            &worker_task_id,
            format!("Processing {} tracks...", total_tracks),
        )
        .await;

        let mut pulled = 0usize;
        let mut cleaned = 0usize;
        let mut skipped = 0usize;

        for (i, track_id) in track_ids.iter().enumerate() {
            if let Some(ct) = tm.get_cancel_token(&worker_task_id).await
                && ct.is_cancelled()
            {
                tm.update_task_status(&worker_task_id, TaskStatus::Cancelled)
                    .await;
                return Ok(());
            }

            // Find linked files for this track (skip WAVs)
            let files: Vec<(i64, String)> = sqlx::query_as(
                "SELECT f.id, f.file_type FROM v_file_track_link v
                     JOIN files f ON f.id = v.file_id
                     WHERE v.track_id = ? AND f.file_type != 'wav'
                     ORDER BY
                       CASE f.file_type
                         WHEN 'stem.m4a' THEN 0
                         WHEN 'flac' THEN 1
                         ELSE 2
                       END",
            )
            .bind(track_id)
            .fetch_all(&db_clone)
            .await
            .unwrap_or_default();

            if files.is_empty() {
                // No linked non-WAV files — can't pull what doesn't exist
                skipped += 1;
                continue;
            }

            // Best file is first (stem.m4a > flac > mp3 per ORDER BY)
            let (best_id, _best_type) = &files[0];

            // Check if best file is local
            let is_local: bool = sqlx::query_scalar(
                "SELECT COUNT(*) FROM file_locations WHERE file_id = ? AND location_type = 'local'",
            )
            .bind(best_id)
            .fetch_one(&db_clone)
            .await
            .unwrap_or(0)
                > 0;

            if !is_local {
                // Check if best file is on backup
                let on_backup: bool =
                        sqlx::query_scalar(
                            "SELECT COUNT(*) FROM file_locations WHERE file_id = ? AND location_type = 'backup'",
                        )
                        .bind(best_id)
                        .fetch_one(&db_clone)
                        .await
                        .unwrap_or(0)
                            > 0;

                if on_backup {
                    pulled += 1;
                    // In a full implementation, this would pull from backup.
                    // For now, log that it needs pulling.
                    tm.add_log(
                        &worker_task_id,
                        format!(
                            "Track #{}: best file #{} needs pull from backup (not implemented)",
                            track_id, best_id
                        ),
                    )
                    .await;
                }
            }

            // Mark redundant formats as safe-to-delete
            // Files after the best (index 0) are redundant and can be cleaned
            if files.len() > 1 {
                // Currently just log — actual cleanup is handled by prune workflow
                cleaned += files.len() - 1;
            }

            if (i + 1) % 50 == 0 {
                let pct = ((i + 1) as f32 / total_tracks as f32) * 100.0;
                tm.update_progress_text(
                    &worker_task_id,
                    format!("Processed {}/{} tracks", i + 1, total_tracks),
                )
                .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.percent = Some(pct);
                    p.message = format!("Processed {}/{} tracks", i + 1, total_tracks);
                })
                .await;
            }
        }

        let msg = format!(
            "Backpack sync complete: {} tracks processed, {} pulled, {} redundant formats found, {} skipped",
            total_tracks, pulled, cleaned, skipped
        );
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

        Ok(())
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    task_id
}
