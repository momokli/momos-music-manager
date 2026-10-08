//! Generic task management module
//!
//! Provides in-memory task tracking for background operations.
//! Supports multiple task types with unified progress tracking.
//! Sync state is tracked in memory, not in the database, to avoid locking issues
//! and provide real-time progress updates.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sqlx::{Pool, Sqlite};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::config::ServiceCredentials;
use crate::embeddings::serialize_embedding;
use crate::spotify::{client::SpotifyClient, sync_worker::SpotifySyncWorker};
use crate::store::{StoreClient, canonicalise_to};

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
    /// Scan a folder for nuo-stems WAV source subdirectories
    ScanWavSources { folder_id: i64 },
    /// PruneFiles: Delete selected local files (must be backed up)
    PruneFiles { file_ids: Vec<i64> },
    /// BackpackSync: Ensure files in backpack tags are available locally
    BackpackSync,
    /// StoreSync: Canonicalise + upload local files to the remote object store
    /// and verify the store's records (SHA-256 facts).
    StoreSync,
    /// Subscription poller: single subscription poll cycle
    PollSubscription {
        subscription_id: i64,
        playlist_name: String,
    },
    /// Global poller: one full cycle checking all Spotify playlists
    GlobalPollCycle,
    /// Maintainer: one housekeeping cycle
    MaintainerCycle,
    /// Folder watcher: one scan-all-active-folders cycle
    FolderWatch,
    /// Telemetry: push a DB snapshot + metadata to the collector
    TelemetryPush,
    /// Sync the BPM//key system playlists to Spotify (one reconcile pass).
    /// `strict` also removes playlists for buckets that vanished.
    SyncBpmKeyPlaylists { strict: bool },
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    pub logs: Vec<String>,
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
            logs: Vec::new(),
            started_at: Instant::now(),
            estimated_remaining: None,
        }
    }

    /// Add a log message
    pub fn add_log(&mut self, message: String) {
        self.logs.push(message);
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
    /// Log messages
    pub logs: Arc<std::sync::Mutex<Vec<String>>>,
    /// Cancellation token for this task
    pub cancel_token: CancellationToken,
    /// When the task was created
    pub created_at: Instant,
    /// When the task most recently transitioned into `Running` (None until then).
    /// Used for telemetry duration_ms.
    started_running_at: Option<Instant>,
    /// Join handle for the background task
    pub join_handle: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    /// Summary set on successful completion (e.g. "Synced 150 tracks across 3 playlists")
    pub result_summary: Arc<std::sync::Mutex<Option<String>>>,
    /// Error message set on failure
    pub error_message: Arc<std::sync::Mutex<Option<String>>>,
    /// Structured result data (e.g. Traktor ImportStats) for frontend rendering
    pub result_data: Arc<std::sync::Mutex<Option<serde_json::Value>>>,
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
        TaskType::ScanWavSources { folder_id } => Some(format!("scan_wavs:{}", folder_id)),
        TaskType::PruneFiles { .. } => None, // prunes don't conflict — multiple can run
        TaskType::BackpackSync => Some("backpack_sync".to_string()),
        TaskType::StoreSync => Some("store_sync".to_string()),
        TaskType::PollSubscription { .. } => None,
        TaskType::GlobalPollCycle => None,
        TaskType::MaintainerCycle => None,
        TaskType::FolderWatch => None,
        TaskType::TelemetryPush => None,
        TaskType::SyncBpmKeyPlaylists { .. } => Some("bpm_key_sync".to_string()),
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
            logs: Arc::new(std::sync::Mutex::new(Vec::new())),
            cancel_token: CancellationToken::new(),
            created_at: Instant::now(),
            started_running_at: None,
            join_handle: None,
            result_summary: Arc::new(std::sync::Mutex::new(None)),
            error_message: Arc::new(std::sync::Mutex::new(None)),
            result_data: Arc::new(std::sync::Mutex::new(None)),
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
            logs: Arc::new(std::sync::Mutex::new(Vec::new())),
            cancel_token: CancellationToken::new(),
            created_at: Instant::now(),
            started_running_at: None,
            join_handle: None,
            result_summary: Arc::new(std::sync::Mutex::new(None)),
            error_message: Arc::new(std::sync::Mutex::new(None)),
            result_data: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Add a log message
    pub fn add_log(&self, message: String) {
        let mut logs = self.logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.push(message);
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
            result_summary: self
                .result_summary
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            error_message: self
                .error_message
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            result_data: self
                .result_data
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
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
            TaskType::ScanWavSources { .. } => "scan_wav_sources".to_string(),
            TaskType::PruneFiles { .. } => "prune_files".to_string(),
            TaskType::BackpackSync => "backpack_sync".to_string(),
            TaskType::StoreSync => "store_sync".to_string(),
            TaskType::PollSubscription { .. } => "poll_subscription".to_string(),
            TaskType::GlobalPollCycle => "global_poll_cycle".to_string(),
            TaskType::MaintainerCycle => "maintainer_cycle".to_string(),
            TaskType::FolderWatch => "folder_watch".to_string(),
            TaskType::TelemetryPush => "telemetry_push".to_string(),
            TaskType::SyncBpmKeyPlaylists { .. } => "sync_bpm_key_playlists".to_string(),
        };
        (task_type_str, task_details)
    }
}

// ============================================================
// TaskProgress — serializable snapshot for API responses
// ============================================================

/// Serializable progress snapshot for API responses
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Summary set on successful completion
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    /// Error message set on failure
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Structured result data (e.g. Traktor ImportStats) for frontend rendering
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_data: Option<serde_json::Value>,
}

// ============================================================
// TaskManager — in-memory task registry
// ============================================================

/// In-memory task manager
#[derive(Clone)]
pub struct TaskManager {
    /// Map of task_id -> Task
    tasks: Arc<RwLock<HashMap<String, Task>>>,
    /// Optional DB pool for persisting completed tasks to task_history
    db: Option<sqlx::Pool<sqlx::Sqlite>>,
}

/// Error returned when a task cannot be started due to a conflict
#[derive(Debug, thiserror::Error)]
pub enum TaskConflictError {
    #[error("A task of this type is already running: {conflict_key}")]
    AlreadyRunning { conflict_key: String },
}

impl TaskManager {
    /// Create a new task manager (no DB persistence)
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            db: None,
        }
    }

    /// Create a new task manager with a DB pool for persisting
    /// completed tasks to the `task_history` table.
    pub fn new_with_pool(db: sqlx::Pool<sqlx::Sqlite>) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            db: Some(db),
        }
    }

    /// Persist a task snapshot to the DB if a pool is configured.
    async fn maybe_persist(&self, task: &Task) {
        if let Some(ref pool) = self.db {
            let progress = task.to_progress();
            if let Err(e) = persist_task_to_db(pool, &progress).await {
                tracing::warn!("Failed to persist task {} to history: {:#}", task.id, e);
            }
        }
    }

    /// Register a new task unconditionally and return its ID.
    pub async fn start_task(&self, task: Task) -> String {
        let id = task.id.clone();
        let transition = TaskTransition::started(&task);
        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(id.clone(), task);
        }
        if let Some(task) = self.tasks.read().await.get(&id) {
            self.maybe_persist(task).await;
        }
        // Telemetry: registration = lifecycle start (exactly one per task).
        transition.emit();
        id
    }

    /// Register a new task, rejecting if a task with the same conflict key is
    /// already running or pending.
    ///
    /// See [`task_type_conflict_key`] for which task types conflict.
    pub async fn start_task_unique(&self, task: Task) -> Result<String, TaskConflictError> {
        let conflict_key = task_type_conflict_key(&task.task_type);
        let id = task.id.clone();
        let transition = TaskTransition::started(&task);
        {
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
        }
        if let Some(task) = self.tasks.read().await.get(&id) {
            self.maybe_persist(task).await;
        }
        transition.emit();
        Ok(id)
    }

    /// Get a serializable snapshot of a task
    pub async fn get_task(&self, task_id: &str) -> Option<TaskProgress> {
        let tasks = self.tasks.read().await;
        tasks.get(task_id).map(|task| task.to_progress())
    }

    /// Cancel a task by ID
    pub async fn cancel_task(&self, task_id: &str) -> anyhow::Result<()> {
        {
            let mut tasks = self.tasks.write().await;
            if let Some(task) = tasks.get_mut(task_id) {
                *task.status.lock().unwrap_or_else(|e| e.into_inner()) = TaskStatus::Cancelled;
                task.add_log("Task cancelled by user".to_string());
                task.cancel_token.cancel();
            } else {
                return Err(anyhow::anyhow!("Task not found: {}", task_id));
            }
        }
        if let Some(task) = self.tasks.read().await.get(task_id) {
            self.maybe_persist(task).await;
        }
        Ok(())
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

    /// Update a task's status. Emits `task.completed` / `task.failed`
    /// telemetry on real transitions into terminal states (`task.started` is
    /// emitted at registration). Never blocking — no-op while telemetry is
    /// disabled.
    pub async fn update_task_status(&self, task_id: &str, status: TaskStatus) {
        let mut transition: Option<TaskTransition> = None;
        {
            let mut tasks = self.tasks.write().await;
            if let Some(task) = tasks.get_mut(task_id) {
                let old = task
                    .status
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if old != status {
                    if status == TaskStatus::Running && task.started_running_at.is_none() {
                        task.started_running_at = Some(Instant::now());
                    }
                    if matches!(status, TaskStatus::Completed | TaskStatus::Failed) {
                        transition = Some(TaskTransition::terminal(task, status.clone()));
                    }
                    *task.status.lock().unwrap_or_else(|e| e.into_inner()) = status;
                }
            }
        }
        // Emit outside the tasks lock (sync + non-blocking anyway).
        if let Some(t) = transition {
            t.emit();
        }
        if let Some(task) = self.tasks.read().await.get(task_id) {
            self.maybe_persist(task).await;
        }
    }

    /// Add a log message to a task
    pub async fn add_log(&self, task_id: &str, message: String) {
        {
            let tasks = self.tasks.read().await;
            if let Some(task) = tasks.get(task_id) {
                task.add_log(message);
            }
        }
        if let Some(task) = self.tasks.read().await.get(task_id) {
            self.maybe_persist(task).await;
        }
    }

    /// Update the progress text of a task (legacy, prefer `update_progress`)
    pub async fn update_progress_text(&self, task_id: &str, text: String) {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id) {
            *task.progress_text.lock().unwrap_or_else(|e| e.into_inner()) = text;
        }
    }

    /// Set structured result data on a task (e.g. Traktor ImportStats).
    /// In-memory only — not persisted to task_history.
    pub async fn set_result_data(&self, task_id: &str, data: serde_json::Value) {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id) {
            *task.result_data.lock().unwrap_or_else(|e| e.into_inner()) = Some(data);
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
// Task lifecycle telemetry
// ============================================================

/// A captured task lifecycle event, emitted as telemetry **after** the tasks
/// lock is released. Emission is synchronous + non-blocking
/// (`telemetry::emit_event` → mpsc `try_send`) and a no-op while telemetry
/// is disabled.
struct TaskTransition {
    /// Event type to emit.
    event: crate::telemetry::events::EventType,
    payload: serde_json::Value,
}

impl TaskTransition {
    /// `task.started` — fired once per task at registration.
    fn started(task: &Task) -> Self {
        let (task_type, _details) = task.task_type_display();
        let mut payload = serde_json::json!({ "task_type": task_type });
        if let Some(ref service) = task.service {
            payload["service"] = serde_json::Value::String(service.clone());
        }
        Self {
            event: crate::telemetry::events::EventType::TaskStarted,
            payload,
        }
    }

    /// `task.completed` / `task.failed` — fired when a task transitions into
    /// a terminal state. `started` is the moment the task began running
    /// (fallback: registration).
    fn terminal(task: &Task, new: TaskStatus) -> Self {
        let (task_type, _details) = task.task_type_display();
        let started = task.started_running_at.unwrap_or(task.created_at);
        let duration_ms = started.elapsed().as_millis() as u64;
        let mut payload = serde_json::json!({
            "task_type": task_type,
            "duration_ms": duration_ms,
        });
        if let Some(ref service) = task.service {
            payload["service"] = serde_json::Value::String(service.clone());
        }
        let event = match new {
            TaskStatus::Failed => {
                // Sanitized error text (home-prefix stripped, truncated).
                let error = task
                    .error_message
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if let Some(ref error) = error {
                    let clean = crate::telemetry::events::sanitize_error_message(error);
                    if !clean.is_empty() {
                        payload["error_message"] = serde_json::Value::String(clean);
                    }
                }
                crate::telemetry::events::EventType::TaskFailed
            }
            _ => crate::telemetry::events::EventType::TaskCompleted,
        };
        Self { event, payload }
    }

    fn emit(&self) {
        crate::telemetry::emit::emit_event(self.event, self.payload.clone());
    }
}

// ============================================================
// Task history persistence (SQLite)
// ============================================================

/// Persist a task snapshot to the `task_history` table.
pub async fn persist_task_to_db(
    pool: &Pool<Sqlite>,
    progress: &TaskProgress,
) -> anyhow::Result<()> {
    let sub_items_json = serde_json::to_string(&progress.sub_items).unwrap_or_default();
    let logs_json = serde_json::to_string(&progress.logs).unwrap_or_default();
    let task_details_json = progress
        .task_details
        .as_ref()
        .map(|d| serde_json::to_string(d).unwrap_or_default());

    // Compute started_at / completed_at based on status
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let started_at = if progress.status != TaskStatus::Pending {
        Some(now)
    } else {
        None
    };
    let completed_at = if matches!(
        progress.status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
    ) {
        Some(now)
    } else {
        None
    };

    sqlx::query(
        r#"
        INSERT OR REPLACE INTO task_history (
            id, task_type, task_details, status, service, progress,
            percent, sub_items, logs, result_summary, error_message,
            started_at, completed_at, created_at_secs, persisted_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch())
        "#,
    )
    .bind(&progress.id)
    .bind(&progress.task_type)
    .bind(&task_details_json)
    .bind(format!("{:?}", progress.status))
    .bind(&progress.service)
    .bind(&progress.progress)
    .bind(progress.percent)
    .bind(&sub_items_json)
    .bind(&logs_json)
    .bind(&progress.result_summary)
    .bind(&progress.error_message)
    .bind(started_at)
    .bind(completed_at)
    .bind(progress.created_at_secs)
    .execute(pool)
    .await?;

    Ok(())
}

/// Query task history from the `task_history` table with pagination and optional filters.
pub async fn get_task_history(
    pool: &Pool<Sqlite>,
    limit: usize,
    offset: usize,
    status: Option<&str>,
    task_type: Option<&str>,
) -> anyhow::Result<(Vec<serde_json::Value>, usize)> {
    // Build dynamic WHERE clauses
    let mut where_clauses = Vec::new();
    let mut count_clauses = Vec::new();

    if let Some(s) = status {
        where_clauses.push(format!("status = '{}'", s.replace('\'', "''")));
        count_clauses.push(format!("status = '{}'", s.replace('\'', "''")));
    }
    if let Some(t) = task_type {
        where_clauses.push(format!("task_type = '{}'", t.replace('\'', "''")));
        count_clauses.push(format!("task_type = '{}'", t.replace('\'', "''")));
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };
    let count_where = if count_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", count_clauses.join(" AND "))
    };

    // Get total count
    let total: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM task_history {}",
        count_where
    ))
    .fetch_one(pool)
    .await?;

    // Get paginated rows
    let rows: Vec<serde_json::Value> = sqlx::query_as::<_, (String, String, Option<String>, String, Option<String>, String, Option<f32>, String, String, Option<String>, Option<String>, Option<i64>, Option<i64>, f64)>(
        &format!(
            "SELECT id, task_type, task_details, status, service, progress, percent, sub_items, logs, result_summary, error_message, started_at, completed_at, created_at_secs FROM task_history {} ORDER BY created_at_secs DESC LIMIT ? OFFSET ?",
            where_sql
        )
    )
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(id, tt, details, st, sv, prog, pct, sub, logs, rs, em, sa, ca, cat)| {
        let sub_items: serde_json::Value =
            serde_json::from_str(&sub).unwrap_or(serde_json::Value::Array(vec![]));
        let logs_arr: serde_json::Value =
            serde_json::from_str(&logs).unwrap_or(serde_json::Value::Array(vec![]));
        let task_details: serde_json::Value = details
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or(serde_json::Value::Null);
        serde_json::json!({
            "id": id,
            "taskType": tt,
            "taskDetails": task_details,
            "status": st,
            "service": sv,
            "progress": prog,
            "percent": pct,
            "subItems": sub_items,
            "logs": logs_arr,
            "resultSummary": rs,
            "errorMessage": em,
            "startedAt": sa,
            "completedAt": ca,
            "createdAtSecs": cat,
        })
    })
    .collect();

    Ok((rows, total as usize))
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
        TaskType::ScanWavSources { folder_id } => format!("Scan WAV sources folder #{}", folder_id),
        TaskType::PruneFiles { file_ids } => format!("Prune {} files", file_ids.len()),
        TaskType::BackpackSync => "Backpack sync all tags".to_string(),
        TaskType::StoreSync => "Sync to object store".to_string(),
        TaskType::PollSubscription { playlist_name, .. } => {
            format!("Poll: {}", playlist_name)
        }
        TaskType::GlobalPollCycle => "Global poll cycle".to_string(),
        TaskType::MaintainerCycle => "Maintainer cycle".to_string(),
        TaskType::FolderWatch => "Folder watch".to_string(),
        TaskType::TelemetryPush => "Telemetry push".to_string(),
        TaskType::SyncBpmKeyPlaylists { .. } => "Sync BPM//key playlists".to_string(),
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

        // Rebuild the resolved-tag tables once per task: `compute_target_comment`
        // reads `file_resolved_tags`, so a tag/playlist deleted moments ago must
        // not leak into the comment we are about to write. This covers every
        // caller of this task, not just the Files-page handlers.
        crate::db::refresh_resolved_tags(&db_clone).await;

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

    match task_manager.start_task_unique(task).await {
        Ok(_) => {}
        Err(TaskConflictError::AlreadyRunning { .. }) => {
            info!("Embedding recompute already running, skipping");
            return String::new();
        }
    }

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
// SyncBpmKeyPlaylists worker
// ============================================================

/// Start a background task that reconciles the BPM//key system playlists with
/// Spotify. Uses `start_task_unique` (conflict key `bpm_key_sync`) so bursts of
/// scan/import events coalesce into a single run.
///
/// Returns the task id, or an empty string when a sync is already running.
pub async fn start_sync_bpm_key_playlists_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    creds: &ServiceCredentials,
    strict: bool,
) -> String {
    let task = Task::new(TaskType::SyncBpmKeyPlaylists { strict }, None);
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let cancel_token = task.cancel_token.clone();

    match task_manager.start_task_unique(task).await {
        Ok(_) => {}
        Err(TaskConflictError::AlreadyRunning { .. }) => {
            info!("BPM//key sync already running, skipping");
            return String::new();
        }
    }

    let tm = task_manager.clone();
    let db_clone = db.clone();
    let creds = creds.clone();

    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(&worker_task_id, "Deriving BPM//key buckets...".to_string())
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = "Deriving BPM//key buckets...".to_string();
        })
        .await;

        match crate::bpm_key::sync::run_sync(
            &tm,
            &db_clone,
            &creds,
            &worker_task_id,
            &cancel_token,
            strict,
        )
        .await
        {
            Ok(_outcome) => {
                // Defense in depth: `run_sync` finalizes on every `Ok` path, but
                // if it ever returns `Ok` without doing so, the task must still
                // not stick at `Running` — its `bpm_key_sync` conflict key would
                // then reject every later sync as "already running".
                tm.update_task_status(&worker_task_id, TaskStatus::Completed)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Completed;
                    if p.percent.is_none() {
                        p.percent = Some(100.0);
                    }
                    if p.message.trim().is_empty() {
                        p.message = "BPM//key sync complete".to_string();
                    }
                })
                .await;
                Ok(())
            }
            Err(e) => {
                let msg = format!("BPM//key sync failed: {e}");
                error!("{}", msg);
                tm.add_log(&worker_task_id, msg.clone()).await;
                tm.update_progress_text(&worker_task_id, format!("Failed: {e}"))
                    .await;
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Failed;
                    p.message = msg.clone();
                })
                .await;
                Err(anyhow::anyhow!(msg))
            }
        }
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    task_id
}

/// Enqueue a BPM//key sync when the feature is enabled. Best-effort — called
/// after folder scans / Traktor imports and by the optional schedule. The task's
/// unique conflict key (`bpm_key_sync`) coalesces bursts into one run.
pub async fn maybe_auto_enqueue_bpm_key_sync(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
) {
    match crate::db::bpm_key::bpm_key_sync_enabled(db).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            warn!(
                "BPM//key auto-sync: reading enabled setting failed: {:#}",
                e
            );
            return;
        }
    }
    let settings = match crate::db::bpm_key::load_bpm_key_settings(db).await {
        Ok(s) => s,
        Err(e) => {
            warn!("BPM//key auto-sync: reading settings failed: {:#}", e);
            return;
        }
    };
    let config =
        crate::bpm_key::installed_config().unwrap_or_else(ServiceCredentials::defaults_for_test);
    let _ = start_sync_bpm_key_playlists_task(task_manager, db, &config, settings.strict).await;
    let _ = crate::db::bpm_key::set_last_sync_enqueued_at(db, chrono::Utc::now().timestamp()).await;
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
        let scan_started = Instant::now();
        let mode_label = crate::db::folders::scan_mode_label(&scan_mode);
        match crate::db::scan_directory_with_config(
            &db_clone,
            path,
            scan_recursive,
            fixed_extensions,
            file_extensions,
            max_depth,
            scan_mode,
            Some(folder.id),
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

                // Refresh materialized tag tables since new files may have
                // been indexed that match existing tracks via ISRC.
                let _ = crate::db::refresh_file_resolved_tags(&db_clone).await;
                let _ = crate::db::refresh_track_resolved_tags(&db_clone).await;

                // Telemetry: folder scan completed (files + duration + mode).
                crate::telemetry::emit::emit_event(
                    crate::telemetry::events::EventType::ScanCompleted,
                    serde_json::json!({
                        "files_count": file_count,
                        "duration_ms": scan_started.elapsed().as_millis() as u64,
                        "mode": mode_label,
                    }),
                );

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

                // Auto-trigger: BPM/key metadata may now be available.
                maybe_auto_enqueue_bpm_key_sync(&tm, &db_clone).await;
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
                    "Import complete: {} entries parsed, {} matched, {} play counts, {} last played, {} BPM, {} key, {} rating. Used: {}",
                    stats.total_entries,
                    stats.matched,
                    stats.with_play_count,
                    stats.with_last_played,
                    stats.with_bpm,
                    stats.with_key,
                    stats.with_rating,
                    nml_path.display()
                );
                info!("{}", msg);
                // Expose structured stats (summary chips + unmatched list) to the UI.
                tm.set_result_data(
                    &worker_task_id,
                    serde_json::json!({
                        "totalEntries": stats.total_entries,
                        "matched": stats.matched,
                        "noPlayCount": stats.no_play_count,
                        "withPlayCount": stats.with_play_count,
                        "withLastPlayed": stats.with_last_played,
                        "withBpm": stats.with_bpm,
                        "withKey": stats.with_key,
                        "withRating": stats.with_rating,
                        "unmatched": stats.unmatched,
                    }),
                )
                .await;
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

                // Auto-trigger: Traktor import may have populated BPM/key.
                maybe_auto_enqueue_bpm_key_sync(&tm, &db_clone).await;
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
            return String::new();
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
/// 2. If no local file but a store backup exists: restore it from the object store
/// 3. If multiple formats: keep only best one, mark others as safe-to-delete
/// 4. Skip WAV source files entirely
///
/// The NAS (rsync/SSH) is retired: a `backup` location is only pullable when it
/// is a `store:<sha256>` object. A legacy location is skipped and logged.
pub async fn start_backpack_sync_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    store: &crate::store::StoreConfig,
) -> String {
    let task_type = TaskType::BackpackSync;
    let task = Task::new(task_type, None);
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let _cancel_token = task.cancel_token.clone();

    let store_client = store.is_configured().then(|| {
        StoreClient::new(
            store.base_url.as_deref().unwrap_or_default(),
            store.token.as_deref().unwrap_or_default(),
        )
    });

    match task_manager.start_task_unique(task).await {
        Ok(_) => {}
        Err(TaskConflictError::AlreadyRunning { .. }) => {
            info!("Backpack sync already running, skipping");
            return String::new();
        }
    };

    let tm = task_manager.clone();
    let db_clone = db.clone();
    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress_text(
            &worker_task_id,
            "Finding files in backpack tags...".to_string(),
        )
        .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = "Finding files in backpack tags...".to_string();
        })
        .await;

        // Step 1: Get all files in backpack tags that need pulling from backup
        let candidates = match crate::db::get_backpack_pull_candidates(&db_clone).await {
            Ok(c) => c,
            Err(e) => {
                let err_msg = format!("Failed to query backpack pull candidates: {}", e);
                error!("{}", err_msg);
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Failed;
                    p.message = err_msg.clone();
                })
                .await;
                return Err(anyhow::anyhow!(err_msg));
            }
        };

        let total = candidates.len();
        if total == 0 {
            let msg = "Backpack sync complete: all files already local".to_string();
            info!("Backpack sync: {}", msg);
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
            return Ok(());
        }

        tm.add_log(
            &worker_task_id,
            format!("Found {} files in backpack tags to pull from backup", total),
        )
        .await;
        tm.update_progress_text(
            &worker_task_id,
            format!("Pulling {}/{} files from backup...", 0, total),
        )
        .await;

        // Step 2: Pull each candidate file from backup
        let mut pulled = 0usize;
        let mut failed = 0usize;

        tm.add_log(
            &worker_task_id,
            format!("Will attempt to pull {} files from backup", total),
        )
        .await;

        for (i, c) in candidates.iter().enumerate() {
            // Check cancellation
            if let Some(ct) = tm.get_cancel_token(&worker_task_id).await
                && ct.is_cancelled()
            {
                tm.update_task_status(&worker_task_id, TaskStatus::Cancelled)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Cancelled;
                    p.message = format!("Cancelled after pulling {}/{} files", pulled, total);
                })
                .await;
                return Ok(());
            }

            // Restore from the object store. A backup location is only
            // pullable when it is a `store:<sha256>` object; the NAS
            // (rsync/SSH) is retired and a legacy location is skipped.
            let Some(hash) = crate::store::store_hash(&c.backup_path) else {
                let msg = format!(
                    "SKIPPED #{} ({}): backup location '{}' is not a store object (NAS retired)",
                    c.file_id, c.title, c.backup_path
                );
                warn!("Backpack sync: {}", msg);
                tm.add_log(&worker_task_id, msg).await;
                failed += 1;
                continue;
            };

            let local = std::path::Path::new(&c.local_path);
            let restored = match &store_client {
                Some(client) => {
                    crate::store::restore_object(client, &db_clone, c.file_id, hash, local).await
                }
                None => Err(anyhow::anyhow!("object store is not configured")),
            };

            match restored {
                Ok(size) => {
                    pulled += 1;
                    let msg = format!("PULLED #{} ({}) — {} bytes", c.file_id, c.title, size);
                    info!("Backpack sync: {}", msg);
                    tm.add_log(&worker_task_id, msg).await;
                    let _ = crate::db::set_file_location(
                        &db_clone,
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
                    .execute(&db_clone)
                    .await;
                }
                Err(e) => {
                    let msg = format!(
                        "FAILED #{} ({}): store restore error — {}",
                        c.file_id, c.title, e
                    );
                    warn!("Backpack sync: {}", msg);
                    tm.add_log(&worker_task_id, msg).await;
                    failed += 1;
                }
            }

            // Update progress every 10 files (or on last file)
            if (i + 1) % 10 == 0 || i + 1 == total {
                let pct = ((i + 1) as f32 / total as f32) * 100.0;
                tm.update_progress_text(
                    &worker_task_id,
                    format!("Pulling {}/{} files...", i + 1, total),
                )
                .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.percent = Some(pct);
                    p.message = format!(
                        "Pulled {}/{} files ({}/{} failed)",
                        i + 1,
                        total,
                        failed,
                        pulled + failed
                    );
                })
                .await;
            }
        }

        // Step 3: Clean up redundant formats (now that best format is local)
        let (cleaned, _freed_bytes) =
            match crate::db::cleanup_redundant_backpack_files(&db_clone).await {
                Ok(result) => result,
                Err(e) => {
                    let msg = format!("WARN: cleanup redundant formats failed — {}", e);
                    warn!("Backpack sync: {}", msg);
                    tm.add_log(&worker_task_id, msg).await;
                    (0usize, 0i64)
                }
            };

        let msg = if pulled == 0 && failed > 0 {
            format!(
                "Backpack sync: ⚠ ALL {} FILES FAILED — 0 pulled. Check task log above for reasons.",
                failed
            )
        } else {
            format!(
                "Backpack sync: {}/{} pulled, {} failed, {} redundant cleaned",
                pulled, total, failed, cleaned
            )
        };
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

        Ok(())
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    task_id
}

// ============================================================
// StoreSync worker
// ============================================================

/// Files per batch: hashed, checked and uploaded together.
const STORE_SYNC_BATCH: i64 = 200;

/// Temp path for a file's canonical copy, keeping the source extension so
/// `lofty` writes the same container.
fn store_temp_path(file_id: i64, file_path: &str) -> std::path::PathBuf {
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "mmm-store-{}-{}{}",
        std::process::id(),
        file_id,
        ext
    ))
}

/// Start a background task that backs up local files to the remote object store.
///
/// Three phases, each throttled and logged to the task:
/// 1. **Backfill** — canonicalise (clear the Comment tag) every local file whose
///    `content_hash` is still `NULL`, and record the resulting object hash.
/// 2. **Verify** — one `POST /objects/check` per batch: present hashes refresh
///    the `store:<hash>` backup location, missing ones drop a stale record.
/// 3. **Upload** — canonicalise + `PUT` every local file that still lacks a
///    store backup location, then record it.
///
/// A soft no-op — no task is created — when the store is not configured.
pub async fn start_store_sync_task(
    task_manager: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    creds: &ServiceCredentials,
) -> String {
    if !creds.store.is_configured() {
        info!("Store sync skipped: store not configured");
        return String::new();
    }
    let base_url = creds.store.base_url.clone().unwrap_or_default();
    let token = creds.store.token.clone().unwrap_or_default();
    let client = StoreClient::new(&base_url, &token);

    let task = Task::new(TaskType::StoreSync, None);
    let task_id = task.id.clone();
    let worker_task_id = task_id.clone();
    let _cancel_token = task.cancel_token.clone();

    match task_manager.start_task_unique(task).await {
        Ok(_) => {}
        Err(TaskConflictError::AlreadyRunning { .. }) => {
            info!("Store sync already running, skipping");
            return String::new();
        }
    };

    let tm = task_manager.clone();
    let db_clone = db.clone();
    let join_handle = tokio::spawn(async move {
        tm.update_task_status(&worker_task_id, TaskStatus::Running)
            .await;
        tm.update_progress(&worker_task_id, |p| {
            p.status = TaskStatus::Running;
            p.message = "Store sync: scanning local files...".to_string();
        })
        .await;

        match run_store_sync(&tm, &db_clone, &client, &worker_task_id).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let err_msg = format!("Store sync failed: {e}");
                error!("{}", err_msg);
                tm.add_log(&worker_task_id, err_msg.clone()).await;
                tm.update_task_status(&worker_task_id, TaskStatus::Failed)
                    .await;
                tm.update_progress(&worker_task_id, |p| {
                    p.status = TaskStatus::Failed;
                    p.message = err_msg.clone();
                })
                .await;
                Err(anyhow::anyhow!(err_msg))
            }
        }
    });

    task_manager.set_join_handle(&task_id, join_handle).await;
    task_id
}

/// The body of the StoreSync worker (extracted so errors set the task Failed).
///
/// Interleaved on purpose: each batch is hashed, checked and uploaded before the
/// next one is read. Hashing the whole library first would push the first upload
/// hours out on a large collection, hide progress, and re-canonicalise every file
/// a second time later on.
async fn run_store_sync(
    tm: &TaskManager,
    db: &sqlx::Pool<sqlx::Sqlite>,
    client: &StoreClient,
    task_id: &str,
) -> anyhow::Result<()> {
    let mut hashed = 0usize;
    let mut uploaded = 0usize;
    let mut already = 0usize;
    let mut verified = 0usize;
    let mut failed = 0usize;
    // Files that failed in this run (unreadable container, or a transient upload
    // error) are skipped for the rest of it — they stay in the candidate set, so
    // without this the loop would never move past them. The next run retries.
    let mut deferred: HashSet<i64> = HashSet::new();

    loop {
        if store_sync_cancelled(tm, task_id).await {
            store_sync_cancelled_done(tm, task_id).await;
            return Ok(());
        }

        let batch: Vec<_> = crate::db::local_files_needing_store_backup(db, STORE_SYNC_BATCH)
            .await?
            .into_iter()
            .filter(|c| !deferred.contains(&c.file_id))
            .collect();
        if batch.is_empty() {
            break;
        }

        // 1. Make sure every candidate has a content hash.
        let mut hashes = Vec::new();
        for c in &batch {
            match c.content_hash.as_deref() {
                Some(h) if !h.is_empty() => hashes.push((c, h.to_string())),
                _ => {
                    let tmp = store_temp_path(c.file_id, &c.file_path);
                    match canonicalise_to(std::path::Path::new(&c.file_path), &tmp) {
                        Ok(hash) => {
                            let _ = std::fs::remove_file(&tmp);
                            sqlx::query("UPDATE files SET content_hash = ? WHERE id = ?")
                                .bind(&hash)
                                .bind(c.file_id)
                                .execute(db)
                                .await?;
                            hashed += 1;
                            hashes.push((c, hash));
                        }
                        Err(e) => {
                            let _ = std::fs::remove_file(&tmp);
                            warn!(
                                "Store sync: canonicalise #{} ({}) failed: {}",
                                c.file_id, c.file_path, e
                            );
                            deferred.insert(c.file_id);
                            failed += 1;
                        }
                    }
                }
            }
        }

        // 2. One round trip to learn which of this batch the store already has.
        let hash_list: Vec<String> = hashes.iter().map(|(_, h)| h.clone()).collect();
        let present: HashSet<String> = if hash_list.is_empty() {
            HashSet::new()
        } else {
            client
                .check(&hash_list)
                .await?
                .present
                .into_iter()
                .collect()
        };

        // 3. Upload the missing ones; record the store location either way.
        for (c, hash) in &hashes {
            if present.contains(hash) {
                crate::db::upsert_store_backup_location(db, c.file_id, hash, c.file_size).await?;
                verified += 1;
                continue;
            }

            let tmp = store_temp_path(c.file_id, &c.file_path);
            let canonical = match canonicalise_to(std::path::Path::new(&c.file_path), &tmp) {
                Ok(h) => h,
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    warn!(
                        "Store sync: canonicalise #{} ({}) failed: {}",
                        c.file_id, c.file_path, e
                    );
                    deferred.insert(c.file_id);
                    failed += 1;
                    continue;
                }
            };

            match client
                .put(&canonical, &tmp, &c.file_path, c.isrc.as_deref())
                .await
            {
                Ok(stored) => {
                    let _ = std::fs::remove_file(&tmp);
                    crate::db::upsert_store_backup_location(db, c.file_id, &canonical, c.file_size)
                        .await?;
                    if stored {
                        uploaded += 1;
                    } else {
                        already += 1;
                    }
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    warn!(
                        "Store sync: upload #{} ({}) failed: {}",
                        c.file_id, c.file_path, e
                    );
                    deferred.insert(c.file_id);
                    failed += 1;
                }
            }
        }

        tm.update_progress_text(
            task_id,
            format!(
                "Store sync: {uploaded} uploaded, {verified} already in the store, {already} deduped, {failed} failed"
            ),
        )
        .await;
    }

    let msg = format!(
        "Store sync: {hashed} hashed, {uploaded} uploaded, {verified} verified present, \
         {already} already present, {failed} failed"
    );
    info!("{}", msg);
    tm.add_log(task_id, msg.clone()).await;
    tm.update_progress_text(task_id, msg.clone()).await;
    tm.update_task_status(task_id, TaskStatus::Completed).await;
    tm.update_progress(task_id, |p| {
        p.status = TaskStatus::Completed;
        p.percent = Some(100.0);
        p.message = msg.clone();
    })
    .await;
    Ok(())
}

/// `true` when the task's cancellation token has been triggered.
async fn store_sync_cancelled(tm: &TaskManager, task_id: &str) -> bool {
    matches!(tm.get_cancel_token(task_id).await, Some(ct) if ct.is_cancelled())
}

/// Mark the task cancelled (used on the cancel path).
async fn store_sync_cancelled_done(tm: &TaskManager, task_id: &str) {
    warn!("Store sync cancelled");
    tm.update_task_status(task_id, TaskStatus::Cancelled).await;
    tm.update_progress(task_id, |p| {
        p.status = TaskStatus::Cancelled;
        p.message = "Store sync cancelled".to_string();
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Der Anspruch eines Scan-Tasks ist deterministisch: hier wird **kein**
    /// Worker gespawnt, der erste Task bleibt also garantiert `Pending`.
    /// Frueher pruefte das ein Integrationstest ueber zwei parallele HTTP-Requests —
    /// der hing daran, ob der erste Scan schon `Completed` war (flaky, siehe PR #86).
    #[tokio::test]
    async fn start_task_unique_rejects_second_scan_wavs_while_pending() {
        let tm = TaskManager::new();

        let first = tm
            .start_task_unique(Task::new(
                TaskType::ScanWavSources { folder_id: 1 },
                Some("scan_wavs".to_string()),
            ))
            .await;
        assert!(first.is_ok(), "first claim must succeed: {first:?}");

        let second = tm
            .start_task_unique(Task::new(
                TaskType::ScanWavSources { folder_id: 1 },
                Some("scan_wavs".to_string()),
            ))
            .await;
        assert!(
            matches!(second, Err(TaskConflictError::AlreadyRunning { .. })),
            "a second scan for the same folder must be rejected while the first is pending, got {second:?}"
        );

        // Ein anderer Ordner hat einen anderen Konflikt-Key und darf nicht blockiert sein.
        let other = tm
            .start_task_unique(Task::new(
                TaskType::ScanWavSources { folder_id: 2 },
                Some("scan_wavs".to_string()),
            ))
            .await;
        assert!(
            other.is_ok(),
            "another folder must not be blocked: {other:?}"
        );
    }

    #[test]
    fn task_transition_started_payload_has_machine_label() {
        let task = Task::new(
            TaskType::ScanFolder { folder_id: 7 },
            Some("scan".to_string()),
        );
        let t = TaskTransition::started(&task);
        assert_eq!(t.event, crate::telemetry::events::EventType::TaskStarted);
        assert_eq!(t.payload["task_type"], "scan_folder");
        assert_eq!(t.payload["service"], "scan");
    }

    #[test]
    fn task_transition_terminal_payload_has_duration() {
        let task = Task::new(TaskType::DeemixSync, None);
        let t = TaskTransition::terminal(&task, TaskStatus::Completed);
        assert_eq!(t.event, crate::telemetry::events::EventType::TaskCompleted);
        assert_eq!(t.payload["task_type"], "deemix_sync");
        assert!(t.payload["duration_ms"].as_u64().unwrap_or(0) >= 0);
        assert!(t.payload.get("error_message").is_none());
    }

    #[test]
    fn task_transition_failed_payload_sanitizes_error() {
        let task = Task::new(TaskType::StoreSync, None);
        let home = dirs::home_dir().unwrap();
        let home_str = home.to_string_lossy().to_string();
        *task.error_message.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(format!("boom at {home_str}/secret/file.flac"));
        let t = TaskTransition::terminal(&task, TaskStatus::Failed);
        assert_eq!(t.event, crate::telemetry::events::EventType::TaskFailed);
        let err = t.payload["error_message"].as_str().unwrap();
        assert!(!err.contains(&home_str), "payload leaks home path: {err}");
        assert!(t.payload["duration_ms"].as_u64().is_some());
    }

    #[tokio::test]
    async fn task_add_log_no_truncation() {
        let task = Task::new(TaskType::BackpackSync, None);
        for i in 0..150 {
            task.add_log(format!("Log entry {}", i));
        }
        let logs = task.logs.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            logs.len(),
            150,
            "Should keep all 150 log entries without truncation"
        );
        assert!(logs.contains(&"Log entry 149".to_string()));
        assert!(logs.contains(&"Log entry 0".to_string()));
    }

    #[tokio::test]
    async fn sync_progress_add_log_no_truncation() {
        let mut sp = SyncProgress::new(SyncType::Full);
        for i in 0..150 {
            sp.add_log(format!("Sync log {}", i));
        }
        assert_eq!(
            sp.logs.len(),
            150,
            "Should keep all 150 sync log entries without truncation"
        );
    }

    #[tokio::test]
    async fn recompute_embeddings_rejects_duplicate() {
        let tm = TaskManager::new();
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();

        // First call should succeed
        let task_id1 = start_recompute_embeddings_task(&tm, &pool).await;
        assert!(
            !task_id1.is_empty(),
            "First call should return a valid task_id"
        );

        // Second call should be rejected (embeddings already running)
        let task_id2 = start_recompute_embeddings_task(&tm, &pool).await;
        assert!(
            task_id2.is_empty(),
            "Second call should return empty string (conflict)"
        );
    }

    #[tokio::test]
    async fn scan_wav_sources_returns_empty_on_conflict() {
        let tm = TaskManager::new();
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();

        let conflict = Task::new(
            TaskType::ScanWavSources { folder_id: 1 },
            Some("scan_wavs".to_string()),
        );
        tm.start_task_unique(conflict).await.unwrap();

        let task_id = start_scan_wav_sources_task(&tm, &pool, 1).await;
        assert!(task_id.is_empty(), "Should return empty string on conflict");
    }

    /// Minimal schema so `run_sync` gets past settings/derivation and fails
    /// specifically at Spotify client construction (Spotify unconfigured).
    async fn bpm_key_worker_pool() -> sqlx::Pool<sqlx::Sqlite> {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        for stmt in [
            r#"CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT 0
            )"#,
            r#"CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                bpm REAL,
                musical_key TEXT,
                stem_type TEXT,
                isrc TEXT,
                spotify_id TEXT
            )"#,
            r#"CREATE TABLE service_tracks (
                id INTEGER PRIMARY KEY,
                service TEXT NOT NULL,
                service_id TEXT NOT NULL,
                isrc TEXT,
                UNIQUE(service, service_id)
            )"#,
            r#"CREATE VIEW v_file_track_link AS
               SELECT f.id AS file_id, st.id AS track_id
               FROM files f
               JOIN service_tracks st ON (
                   st.isrc = f.isrc
                   OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
               )"#,
        ] {
            sqlx::query(stmt).execute(&pool).await.unwrap();
        }
        pool
    }

    /// Poll until the task reaches a terminal status (or panic after ~2 s).
    async fn wait_for_terminal(tm: &TaskManager, task_id: &str) -> TaskStatus {
        for _ in 0..200 {
            if let Some(p) = tm.get_task(task_id).await {
                if matches!(
                    p.status,
                    TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
                ) {
                    return p.status;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("task {task_id} never reached a terminal state");
    }

    /// The stuck-`Running` bug: an early-abort/cancel run returned before the
    /// finalization block, so the task never left `Running` and its
    /// `bpm_key_sync` conflict key rejected every later sync as "already
    /// running". The worker must always finalize (nothing stays `Running`).
    #[tokio::test]
    async fn bpm_key_worker_finalizes_and_does_not_wedge_the_conflict_key() {
        let tm = TaskManager::new();
        let pool = bpm_key_worker_pool().await;

        // No Spotify credentials: `run_sync` fails fast. The worker must still
        // drive the task to a terminal status.
        let creds = crate::config::ServiceCredentials::defaults_for_test();
        let task_id = start_sync_bpm_key_playlists_task(&tm, &pool, &creds, false).await;
        assert!(!task_id.is_empty(), "a fresh sync must be accepted");

        let status = wait_for_terminal(&tm, &task_id).await;
        assert!(
            matches!(status, TaskStatus::Completed | TaskStatus::Failed),
            "task must reach a terminal state, never Running, got {status:?}"
        );

        // The finished task must not block the next sync via the conflict key.
        let second = start_sync_bpm_key_playlists_task(&tm, &pool, &creds, false).await;
        assert!(
            !second.is_empty(),
            "a terminal task must not wedge the bpm_key_sync conflict key"
        );
    }
}
