//! Sync task management module
//!
//! This module provides in-memory task tracking for background sync operations.
//! Sync state is tracked in memory, not in the database, to avoid locking issues
//! and provide real-time progress updates.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sqlx::Pool;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use uuid::Uuid;

use crate::config::ServiceCredentials;
use crate::spotify::{client::SpotifyClient, sync_worker::SpotifySyncWorker};

/// Type of sync operation
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncType {
    /// Sync only playlist metadata (no tracks)
    Playlists,

    /// Sync tracks for a specific playlist
    TracksForPlaylist(String), // Spotify playlist ID

    /// Sync tracks for all playlists in the database
    TracksAll,

    /// Full sync: playlists + all tracks
    Full,
}

/// Status of a sync task
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SyncStatus {
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

/// Progress tracking for a sync task
#[derive(Clone, Debug, Serialize)]
pub struct SyncProgress {
    /// Type of sync operation
    pub sync_type: SyncType,

    /// Current status
    pub status: SyncStatus,

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

    // Timing
    #[serde(skip)]
    pub started_at: Instant,
    #[serde(skip)]
    pub estimated_remaining: Option<Duration>,
}

impl SyncProgress {
    /// Create new progress for a sync type
    pub fn new(sync_type: SyncType) -> Self {
        Self {
            sync_type,
            status: SyncStatus::Pending,
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
                {
                    if total > 0 {
                        return Some((current as f32 / total as f32) * 100.0);
                    }
                }
            }
            SyncType::TracksForPlaylist(_) => {
                if let (Some(current), Some(total)) = (self.current_track, self.total_tracks) {
                    if total > 0 {
                        return Some((current as f32 / total as f32) * 100.0);
                    }
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
    /// Number of playlists synced
    pub playlist_count: usize,

    /// Number of tracks synced
    pub track_count: usize,

    /// Names of synced playlists
    pub playlist_names: Vec<String>,

    /// Names of synced tracks
    pub track_names: Vec<String>,

    /// Error message if sync failed
    pub error: Option<String>,
}

impl SyncResult {
    /// Create a successful sync result
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

    /// Create a failed sync result
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

/// A sync task with cancellation support
pub struct SyncTask {
    /// Unique task ID
    pub id: String,

    /// Service name (spotify, soundcloud, youtube)
    pub service: String,

    /// Current progress
    pub progress: Arc<RwLock<SyncProgress>>,

    /// Cancellation token for this task
    pub cancel_token: CancellationToken,

    /// Join handle for the background task
    pub join_handle: Option<tokio::task::JoinHandle<anyhow::Result<SyncResult>>>,
}

impl SyncTask {
    /// Create a new sync task
    pub fn new(id: String, service: String, sync_type: SyncType) -> Self {
        Self {
            id,
            service,
            progress: Arc::new(RwLock::new(SyncProgress::new(sync_type))),
            cancel_token: CancellationToken::new(),
            join_handle: None,
        }
    }

    /// Get a clone of the progress
    pub async fn get_progress(&self) -> SyncProgress {
        self.progress.read().await.clone()
    }

    /// Update progress fields
    pub async fn update_progress<F>(&self, update_fn: F)
    where
        F: FnOnce(&mut SyncProgress),
    {
        let mut progress = self.progress.write().await;
        update_fn(&mut progress);
    }

    /// Add a log message
    pub async fn add_log(&self, message: String) {
        let mut progress = self.progress.write().await;
        progress.add_log(message);
    }

    /// Check if task has been cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }
}

/// Sync manager for in-memory task tracking
/// Only allows one sync per service at a time
#[derive(Clone)]
pub struct SyncManager {
    /// Map of task_id -> SyncTask
    tasks: Arc<RwLock<HashMap<String, SyncTask>>>,

    /// Database connection pool
    db: Pool<sqlx::Sqlite>,
}

impl SyncManager {
    /// Create a new sync manager
    pub fn new(db: Pool<sqlx::Sqlite>) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            db,
        }
    }

    /// Start a new sync task for Spotify
    /// Returns task_id if successful, error if sync already running for service
    pub async fn start_spotify_sync(
        &self,
        sync_type: SyncType,
        credentials: &ServiceCredentials,
    ) -> anyhow::Result<String> {
        let service = "spotify".to_string();

        // Check if Spotify sync is already running
        let tasks = self.tasks.read().await;
        if tasks.values().any(|task| task.service == service) {
            return Err(anyhow::anyhow!("Spotify sync already running"));
        }

        drop(tasks); // Release read lock

        // Generate task ID
        let task_id = Uuid::new_v4().to_string();
        let worker_task_id = task_id.clone();

        // Create task
        let task = SyncTask::new(task_id.clone(), service.clone(), sync_type.clone());

        // Clone for background task
        let manager = self.clone();
        let db = self.db.clone();
        let credentials = credentials.clone();
        let progress = task.progress.clone();
        let cancel_token = task.cancel_token.clone();

        info!("Starting Spotify sync with type: {:?}", sync_type);

        // Spawn background task
        let join_handle = tokio::spawn(async move {
            info!("Background task started for Spotify sync");

            // Create Spotify client
            let spotify_client = match SpotifyClient::from_stored_tokens(db, &credentials).await {
                Ok(client) => {
                    info!("Spotify client created successfully");
                    client
                }
                Err(e) => {
                    error!("Failed to create Spotify client: {}", e);
                    return Err(anyhow::anyhow!("Failed to create Spotify client: {}", e));
                }
            };

            info!("Creating SpotifySyncWorker with sync type: {:?}", sync_type);

            // Create and run sync worker
            let worker = SpotifySyncWorker::new(
                manager.db.clone(),
                spotify_client,
                worker_task_id.clone(),
                sync_type,
                cancel_token,
                progress,
            );

            info!("Running Spotify sync worker...");
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
                        .execute(&manager.db)
                        .await
                        {
                            error!("Failed to update remote counts: {}", e);
                        }
                    }

                    // Update task status
                    if let Some(task) = manager.tasks.write().await.get_mut(&worker_task_id) {
                        task.update_progress(|progress| {
                            progress.status = if result.error.is_some() {
                                SyncStatus::Failed
                            } else {
                                SyncStatus::Completed
                            };
                            if let Some(error) = &result.error {
                                progress.add_log(format!("Sync failed: {}", error));
                            } else {
                                progress.add_log(format!(
                                    "Sync completed: {} playlists, {} tracks",
                                    result.playlist_count, result.track_count
                                ));
                            }
                        })
                        .await;
                    }

                    Ok(result)
                }
                Err(e) => {
                    error!("Spotify sync worker failed: {}", e);

                    // Update task status to failed
                    if let Some(task) = manager.tasks.write().await.get_mut(&worker_task_id) {
                        task.update_progress(|progress| {
                            progress.status = SyncStatus::Failed;
                            progress.add_log(format!("Sync failed: {}", e));
                        })
                        .await;
                    }

                    Err(e)
                }
            }
        });

        // Store task with join handle
        let mut tasks = self.tasks.write().await;
        tasks.insert(
            task_id.clone(),
            SyncTask {
                join_handle: Some(join_handle),
                ..task
            },
        );

        Ok(task_id)
    }

    /// Get a sync task by ID
    pub async fn get_task(&self, task_id: &str) -> Option<SyncProgress> {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id) {
            // Clone the Arc and get progress asynchronously
            let progress_arc = task.progress.clone();
            let progress = progress_arc.read().await.clone();
            Some(progress)
        } else {
            None
        }
    }

    /// Cancel a sync task
    pub async fn cancel_task(&self, task_id: &str) -> anyhow::Result<()> {
        let mut tasks = self.tasks.write().await;

        if let Some(task) = tasks.get_mut(task_id) {
            // Update status to cancelled
            task.update_progress(|progress| {
                progress.status = SyncStatus::Cancelled;
            })
            .await;

            // Send cancellation signal
            task.cancel_token.cancel();

            Ok(())
        } else {
            Err(anyhow::anyhow!("Task not found: {}", task_id))
        }
    }

    /// List all active sync tasks
    pub async fn list_tasks(&self) -> Vec<(String, SyncProgress)> {
        let tasks = self.tasks.read().await;
        let mut result = Vec::new();

        for (task_id, task) in tasks.iter() {
            let progress_arc = task.progress.clone();
            let progress = progress_arc.read().await.clone();
            result.push((task_id.clone(), progress));
        }

        result
    }

    /// Remove a completed/failed/cancelled task
    pub async fn remove_task(&self, task_id: &str) {
        let mut tasks = self.tasks.write().await;
        tasks.remove(task_id);
    }
}

/// Sync error types
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("Sync already running for service: {0}")]
    AlreadyRunning(String),

    #[error("Task not found")]
    NotFound,

    #[error("Task is not running")]
    NotRunning,

    #[error("Sync cancelled")]
    Cancelled,

    #[error("Spotify API error: {0}")]
    SpotifyApi(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
}
