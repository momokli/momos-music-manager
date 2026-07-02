//! Background sync worker for Tidal.
//!
//! Follows the same pattern as SoundCloud and Spotify sync workers — integrates
//! with the TaskManager for progress tracking, cancellation, and result reporting.
//!
//! Sync modes:
//!   - `SyncType::Playlists` — fetch and store playlist metadata only
//!   - `SyncType::Full` — fetch all playlists + tracks
//!   - `SyncType::TracksForPlaylist(String)` — fetch one playlist + its tracks

use anyhow::{Context, Result};
use sqlx::{Pool, Sqlite};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::tasks::{SyncProgress, SyncResult, SyncType, TaskStatus};
use crate::tidal::client::TidalClient;
use crate::tidal::models::TidalPlaylist;

/// Background worker that syncs Tidal playlists and tracks into the local DB.
pub struct TidalSyncWorker {
    db: Pool<Sqlite>,
    tidal_client: TidalClient,
    task_id: String,
    sync_type: SyncType,
    cancel_token: CancellationToken,
    progress: Arc<RwLock<SyncProgress>>,
}

impl TidalSyncWorker {
    pub fn new(
        db: Pool<Sqlite>,
        tidal_client: TidalClient,
        task_id: String,
        sync_type: SyncType,
        cancel_token: CancellationToken,
        progress: Arc<RwLock<SyncProgress>>,
    ) -> Self {
        Self {
            db,
            tidal_client,
            task_id,
            sync_type,
            cancel_token,
            progress,
        }
    }

    pub async fn run(&self) -> Result<SyncResult> {
        info!("Starting Tidal sync: {:?}", self.sync_type);

        match &self.sync_type {
            SyncType::Playlists => self.sync_playlists_only().await,
            SyncType::Full => self.sync_full().await,
            SyncType::TracksForPlaylist(playlist_id) => {
                self.sync_single_playlist(playlist_id).await
            }
            _ => {
                warn!(
                    "Unsupported Tidal sync type: {:?}, treating as full sync",
                    self.sync_type
                );
                self.sync_full().await
            }
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    async fn update_playlist_progress(&self, current: usize, total: Option<usize>, name: &str) {
        let mut progress = self.progress.write().await;
        progress.current_playlist = Some(current);
        if let Some(total) = total {
            progress.total_playlists = Some(total);
        }
        progress.current_playlist_name = Some(name.to_string());
        let total_display = progress
            .total_playlists
            .map_or("?".to_string(), |t| t.to_string());
        progress.add_log(format!(
            "Processing playlist {}/{}: {}",
            current, total_display, name
        ));
    }

    // ── Sync operations ─────────────────────────────────────────────────

    async fn sync_playlists_only(&self) -> Result<SyncResult> {
        info!("Starting Tidal playlist-only sync");

        {
            let mut progress = self.progress.write().await;
            progress.status = TaskStatus::Running;
            progress.add_log("Fetching Tidal playlists...".to_string());
        }

        let playlists = self
            .tidal_client
            .get_user_playlists()
            .await
            .context("Failed to fetch Tidal playlists")?;

        let total = playlists.len();
        let mut playlist_count = 0;
        let mut playlist_names = Vec::new();

        {
            let mut progress = self.progress.write().await;
            progress.total_playlists = Some(total);
            progress.add_log(format!("Found {} Tidal playlists", total));
        }

        for (i, playlist) in playlists.iter().enumerate() {
            if self.is_cancelled() {
                return Ok(SyncResult::failed("Sync cancelled by user".to_string()));
            }

            playlist_count += 1;
            playlist_names.push(playlist.name.clone());
            self.update_playlist_progress(i + 1, Some(total), &playlist.name)
                .await;

            if let Err(e) = self.store_playlist(playlist).await {
                error!(
                    "Failed to store Tidal playlist '{}': {:?}",
                    playlist.name, e
                );
            }
        }

        {
            let mut progress = self.progress.write().await;
            progress.status = TaskStatus::Completed;
            progress.add_log(format!(
                "Tidal playlist sync completed: {} playlists",
                playlist_count
            ));
        }

        Ok(SyncResult::success(
            playlist_count,
            0,
            playlist_names,
            Vec::new(),
        ))
    }

    /// Full sync: playlists + tracks.
    async fn sync_full(&self) -> Result<SyncResult> {
        let result = self.sync_playlists_only().await?;
        // For now, full sync = playlists only (tracks synced in single playlist mode)
        Ok(result)
    }

    /// Sync a single playlist and all its tracks.
    async fn sync_single_playlist(&self, playlist_id: &str) -> Result<SyncResult> {
        info!("Starting Tidal tracks sync for playlist: {}", playlist_id);

        {
            let mut progress = self.progress.write().await;
            progress.status = TaskStatus::Running;
            progress.add_log(format!(
                "Fetching Tidal playlist {} with tracks...",
                playlist_id
            ));
        }

        let (playlist_info, tracks) = self
            .tidal_client
            .get_playlist(playlist_id)
            .await
            .context(format!("Failed to fetch Tidal playlist {}", playlist_id))?;

        let playlist_name = playlist_info.name.clone();
        let track_count = tracks.len();

        // Store the playlist
        if let Err(e) = self.store_playlist(&playlist_info).await {
            error!(
                "Failed to store Tidal playlist '{}': {:?}",
                playlist_name, e
            );
        }

        // Get the DB playlist ID
        let db_playlist_id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM service_playlists WHERE service = 'tidal' AND playlist_id = ?",
        )
        .bind(&playlist_info.id)
        .fetch_optional(&self.db)
        .await?;

        let db_playlist_id = match db_playlist_id {
            Some(id) => id,
            None => {
                return Ok(SyncResult::failed(format!(
                    "Failed to find stored playlist {}",
                    playlist_id
                )));
            }
        };

        // Store tracks
        let mut track_names = Vec::new();
        for (i, track) in tracks.iter().enumerate() {
            if self.is_cancelled() {
                return Ok(SyncResult::failed("Sync cancelled by user".to_string()));
            }

            track_names.push(format!("{} - {}", track.artist, track.title));

            // Upsert the service track
            let isrc = track.isrc.clone();
            let now = chrono::Utc::now().timestamp();
            sqlx::query(
                "INSERT INTO service_tracks (service, service_id, title, artist, isrc, duration_ms, imported_at, updated_at) VALUES ('tidal', ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(service, service_id) DO UPDATE SET title = excluded.title, artist = excluded.artist, isrc = excluded.isrc, duration_ms = excluded.duration_ms, updated_at = excluded.updated_at",
            )
            .bind(&track.id)
            .bind(&track.title)
            .bind(&track.artist)
            .bind(isrc.as_deref())
            .bind(track.duration_ms)
            .bind(now)
            .bind(now)
            .execute(&self.db)
            .await?;

            // Get the DB track ID
            let db_track_id: i64 = sqlx::query_scalar(
                "SELECT id FROM service_tracks WHERE service = 'tidal' AND service_id = ?",
            )
            .bind(&track.id)
            .fetch_one(&self.db)
            .await?;

            // Add track to playlist
            let now2 = chrono::Utc::now().timestamp();
            sqlx::query(
                "INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at) VALUES (?, ?, ?, ?)",
            )
            .bind(db_playlist_id)
            .bind(db_track_id)
            .bind(i as i32)
            .bind(now2)
            .execute(&self.db)
            .await?;
        }

        // Update playlist track count
        let now = chrono::Utc::now().timestamp();
        let _ = sqlx::query(
            "UPDATE service_playlists SET remote_track_count = ?, last_fetched_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(track_count as i64)
        .bind(now)
        .bind(now)
        .bind(db_playlist_id)
        .execute(&self.db)
        .await;

        {
            let mut progress = self.progress.write().await;
            progress.status = TaskStatus::Completed;
            progress.add_log(format!(
                "Tidal playlist sync completed: '{}' with {} tracks",
                playlist_name, track_count
            ));
        }

        info!(
            "Tidal playlist sync completed: '{}' with {} tracks",
            playlist_name, track_count
        );

        Ok(SyncResult::success(
            1,
            track_count,
            vec![playlist_name],
            track_names,
        ))
    }

    // ── Storage helpers ────────────────────────────────────────────────

    async fn store_playlist(&self, playlist: &TidalPlaylist) -> Result<()> {
        let name = &playlist.name;
        let desc = playlist.description.as_deref().unwrap_or("");
        let now = chrono::Utc::now().timestamp();

        sqlx::query(
            "INSERT INTO service_playlists (service, playlist_id, name, description, imported_at, updated_at) VALUES ('tidal', ?, ?, ?, ?, ?) ON CONFLICT(service, playlist_id) DO UPDATE SET name = excluded.name, description = excluded.description, updated_at = excluded.updated_at",
        )
        .bind(&playlist.id)
        .bind(name)
        .bind(desc)
        .bind(now)
        .bind(now)
        .execute(&self.db)
        .await?;

        Ok(())
    }
}
