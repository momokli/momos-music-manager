//! Folder watcher module for monitoring and scanning directories
//!
//! This module provides a simple polling-based folder watcher that scans
//! active folders at regular intervals (default: 5 minutes).

use anyhow::Result;
use sqlx::{Pool, Sqlite};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time;
use tracing::{error, info, warn};

use crate::db;

/// Configuration for the folder watcher
#[derive(Debug, Clone)]
pub struct FolderWatcherConfig {
    /// Interval between scans in seconds (default: 300 = 5 minutes)
    pub scan_interval_seconds: u64,
    /// Whether to start the watcher automatically
    #[allow(dead_code)]
    pub auto_start: bool,
}

impl Default for FolderWatcherConfig {
    fn default() -> Self {
        Self {
            scan_interval_seconds: 300, // 5 minutes
            auto_start: true,
        }
    }
}

/// Folder watcher that polls active folders at regular intervals
pub struct FolderWatcher {
    db_pool: Pool<Sqlite>,
    task_manager: crate::tasks::TaskManager,
    config: FolderWatcherConfig,
    is_running: Arc<std::sync::atomic::AtomicBool>,
    shutdown_sender: Option<oneshot::Sender<()>>,
}

#[allow(dead_code)]
impl FolderWatcher {
    /// Create a new folder watcher
    pub fn new(
        db_pool: Pool<Sqlite>,
        task_manager: crate::tasks::TaskManager,
        config: FolderWatcherConfig,
    ) -> Self {
        Self {
            db_pool,
            task_manager,
            config,
            is_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            shutdown_sender: None,
        }
    }

    /// Start the folder watcher in the background
    /// Returns a handle that can be used to stop the watcher
    pub fn start(&mut self) -> Result<()> {
        if self.is_running.load(std::sync::atomic::Ordering::Relaxed) {
            warn!("Folder watcher is already running");
            return Ok(());
        }

        info!(
            "Starting folder watcher with {} second interval",
            self.config.scan_interval_seconds
        );

        let db_pool = self.db_pool.clone();
        let tm = self.task_manager.clone();
        let interval_seconds = self.config.scan_interval_seconds;
        let is_running = self.is_running.clone();

        // Set running flag
        is_running.store(true, std::sync::atomic::Ordering::Relaxed);

        // Create shutdown channel
        let (shutdown_sender, mut shutdown_receiver) = oneshot::channel();

        // Spawn the watcher task
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(interval_seconds));

            // Run initial scan immediately
            info!("Running initial folder scan...");
            if let Err(e) = Self::scan_active_folders(&db_pool, &tm).await {
                error!("Initial folder scan failed: {}", e);
            }

            info!(
                "Folder watcher started, next scan in {} seconds",
                interval_seconds
            );

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        info!("Starting scheduled folder scan...");
                        if let Err(e) = Self::scan_active_folders(&db_pool, &tm).await {
                            error!("Scheduled folder scan failed: {}", e);
                        } else {
                            info!("Scheduled folder scan completed");
                        }
                    }
                    _ = &mut shutdown_receiver => {
                        info!("Folder watcher shutdown signal received");
                        break;
                    }
                }
            }

            // Clear running flag
            is_running.store(false, std::sync::atomic::Ordering::Relaxed);
            info!("Folder watcher stopped");
        });

        self.shutdown_sender = Some(shutdown_sender);
        Ok(())
    }

    /// Stop the folder watcher
    pub async fn stop(&mut self) -> Result<()> {
        if !self.is_running.load(std::sync::atomic::Ordering::Relaxed) {
            warn!("Folder watcher is not running");
            return Ok(());
        }

        info!("Stopping folder watcher...");

        if let Some(shutdown_sender) = self.shutdown_sender.take() {
            let _ = shutdown_sender.send(());
        }

        // Wait a bit for the task to clean up
        time::sleep(Duration::from_millis(100)).await;
        Ok(())
    }

    /// Check if the watcher is running
    pub fn is_running(&self) -> bool {
        self.is_running.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Trigger an immediate scan of all active folders
    pub async fn scan_now(&self) -> Result<usize> {
        info!("Manual folder scan requested");
        Self::scan_active_folders(&self.db_pool, &self.task_manager).await
    }

    /// Scan all active folders in the database via background tasks
    async fn scan_active_folders(
        pool: &Pool<Sqlite>,
        tm: &crate::tasks::TaskManager,
    ) -> Result<usize> {
        // Get all active folders
        let folders = db::get_folders(pool).await?;
        let active_folders: Vec<_> = folders.into_iter().filter(|f| f.active).collect();

        if active_folders.is_empty() {
            info!("No active folders to scan");
            return Ok(0);
        }

        info!("Scanning {} active folder(s)...", active_folders.len());

        let mut scanned_folders = 0;

        for folder in active_folders {
            info!("Starting scan for folder: {}", folder.folder_path);

            match crate::tasks::start_scan_folder_task(
                tm,
                pool,
                folder.id,
                crate::db::ScanMode::Incremental { since: None },
            )
            .await
            {
                Ok(task_id) => {
                    info!(
                        "Started scan task {} for folder: {}",
                        task_id, folder.folder_path
                    );
                    scanned_folders += 1;
                }
                Err(e) => {
                    error!(
                        "Failed to start scan task for folder {}: {}",
                        folder.folder_path, e
                    );
                }
            }
        }

        info!(
            "Folder scan completed: {} folder(s) scanned via background tasks",
            scanned_folders
        );

        Ok(scanned_folders)
    }

    /// Get the watcher configuration
    pub fn config(&self) -> &FolderWatcherConfig {
        &self.config
    }

    /// Update the watcher configuration
    /// Note: Changes to scan_interval_seconds will only take effect after restart
    pub fn update_config(&mut self, config: FolderWatcherConfig) {
        self.config = config;
        info!("Folder watcher configuration updated");
    }
}
