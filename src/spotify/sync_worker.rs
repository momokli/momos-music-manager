//! Spotify sync worker
//!
//! This module contains the SpotifySyncWorker which performs background sync operations
//! with progress tracking, cancellation support, and error resilience.

use anyhow::{Context, Result};
use rspotify::model::{PlayableItem, SimplifiedPlaylist, track::FullTrack};
use sqlx::{Pool, Sqlite};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::db::{add_track_to_playlist, upsert_service_playlist, upsert_service_track};
use crate::spotify::client::SpotifyClient;
use crate::spotify::models::{PlaylistInfo, TrackInfo};
use crate::sync::{SyncProgress, SyncResult, SyncType};

/// Spotify sync worker that performs background sync operations
pub struct SpotifySyncWorker {
    /// Database connection pool
    db: Pool<Sqlite>,

    /// Spotify client for API calls
    spotify_client: SpotifyClient,

    /// Task ID for progress tracking
    task_id: String,

    /// Type of sync to perform
    sync_type: SyncType,

    /// Cancellation token for this sync task
    cancel_token: CancellationToken,

    /// Progress tracking for this sync task
    progress: std::sync::Arc<tokio::sync::RwLock<SyncProgress>>,
}

impl SpotifySyncWorker {
    /// Create a new Spotify sync worker
    pub fn new(
        db: Pool<Sqlite>,
        spotify_client: SpotifyClient,
        task_id: String,
        sync_type: SyncType,
        cancel_token: CancellationToken,
        progress: std::sync::Arc<tokio::sync::RwLock<SyncProgress>>,
    ) -> Self {
        Self {
            db,
            spotify_client,
            task_id,
            sync_type,
            cancel_token,
            progress,
        }
    }

    /// Run the sync operation based on sync_type
    pub async fn run(mut self) -> Result<SyncResult> {
        info!("Starting Spotify sync: {:?}", self.sync_type);

        // Clone sync_type to avoid borrow issues
        let sync_type = self.sync_type.clone();
        match sync_type {
            SyncType::Playlists => self.sync_playlists_only().await,
            SyncType::TracksForPlaylist(playlist_id) => {
                self.sync_tracks_for_playlist(&playlist_id).await
            }
            SyncType::TracksAll => self.sync_all_tracks().await,
            SyncType::Full => self.sync_full().await,
        }
    }

    /// Check if the sync has been cancelled
    fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Update progress with current playlist info
    async fn update_playlist_progress(&self, current: usize, total: Option<usize>, name: &str) {
        let mut progress = self.progress.write().await;
        progress.current_playlist = Some(current);
        if let Some(total) = total {
            progress.total_playlists = Some(total);
        }
        progress.current_playlist_name = Some(name.to_string());

        // Add log entry
        let total_display = progress
            .total_playlists
            .map_or("?".to_string(), |t| t.to_string());
        progress.add_log(format!(
            "Processing playlist {}/{}: {}",
            current, total_display, name
        ));
    }

    /// Update progress with current track info
    async fn update_track_progress(
        &self,
        current: usize,
        total: Option<usize>,
        name: &str,
        playlist_name: &str,
    ) {
        let mut progress = self.progress.write().await;
        progress.current_track = Some(current);
        if let Some(total) = total {
            progress.total_tracks = Some(total);
        }
        progress.current_track_name = Some(name.to_string());
        progress.current_playlist_for_tracks = Some(playlist_name.to_string());

        // Add log entry
        let total_display = progress
            .total_tracks
            .map_or("?".to_string(), |t| t.to_string());
        progress.add_log(format!(
            "Processing track {}/{} in {}: {}",
            current, total_display, playlist_name, name
        ));
    }

    /// Sync only playlists (metadata)
    async fn sync_playlists_only(&mut self) -> Result<SyncResult> {
        info!("Starting playlist-only sync");

        // Update status to running
        {
            let mut progress = self.progress.write().await;
            progress.status = crate::sync::SyncStatus::Running;
        }

        debug!("Getting user playlists from Spotify client...");
        let mut playlists_stream = match self.spotify_client.get_user_playlists().await {
            Ok(stream) => {
                debug!("Successfully got playlists stream");
                stream
            }
            Err(e) => {
                error!("Failed to get playlists stream: {:?}", e);
                return Err(e);
            }
        };
        let mut playlist_count = 0;
        let mut playlist_names = Vec::new();

        while let Some(playlist_result) = playlists_stream.next().await {
            // Check for cancellation
            if self.is_cancelled() {
                debug!("Playlist sync cancelled - returning failed result");
                return Ok(SyncResult::failed("Sync cancelled by user".to_string()));
            }

            match playlist_result {
                Ok(playlist) => {
                    playlist_count += 1;
                    playlist_names.push(playlist.name.clone());

                    // Update progress
                    self.update_playlist_progress(playlist_count, None, &playlist.name)
                        .await;

                    debug!(
                        "Processing playlist {}/?: {}",
                        playlist_count, playlist.name
                    );

                    // Store playlist in database
                    if let Err(e) = self.store_playlist(&playlist).await {
                        error!("Failed to store playlist {}: {:?}", playlist.name, e);
                        // Continue with next playlist
                    }
                }
                Err(e) => {
                    error!("Failed to fetch playlist: {:?}", e);
                    // Continue with next playlist
                }
            }
        }

        // Update final progress
        {
            let mut progress = self.progress.write().await;
            progress.total_playlists = Some(playlist_count);
            progress.status = crate::sync::SyncStatus::Completed;
            progress.add_log(format!("Sync completed: {} playlists", playlist_count));
        }

        info!("Playlist sync completed: {} playlists", playlist_count);

        Ok(SyncResult::success(
            playlist_count,
            0,
            playlist_names,
            Vec::new(),
        ))
    }

    /// Sync tracks for a specific playlist
    async fn sync_tracks_for_playlist(&mut self, playlist_id: &str) -> Result<SyncResult> {
        info!("Starting tracks sync for playlist: {}", playlist_id);

        // Update status to running
        {
            let mut progress = self.progress.write().await;
            progress.status = crate::sync::SyncStatus::Running;
        }

        // First get playlist info
        let playlist = match self.spotify_client.get_playlist(playlist_id).await {
            Ok(playlist) => playlist,
            Err(e) => {
                error!("Failed to fetch playlist {}: {:?}", playlist_id, e);
                return Ok(SyncResult::failed(format!(
                    "Failed to fetch playlist: {:?}",
                    e
                )));
            }
        };

        let playlist_name = playlist.name.clone();

        // Get tracks for this playlist
        let mut tracks_stream = self.spotify_client.get_playlist_tracks(playlist_id).await?;
        let mut track_count = 0;
        let mut track_names = Vec::new();
        let mut position = 0;

        while let Some(track_result) = tracks_stream.next().await {
            // Check for cancellation
            if self.is_cancelled() {
                debug!("Track sync cancelled for playlist: {}", playlist_name);
                return Ok(SyncResult::failed("Sync cancelled by user".to_string()));
            }

            match track_result {
                Ok(item) => {
                    // Extract track from playlist item
                    if let Some(track) = item.track {
                        if let PlayableItem::Track(track) = track {
                            if let Some(_track_id) = &track.id {
                                track_count += 1;
                                position += 1;
                                track_names.push(track.name.clone());

                                // Update progress every 10 tracks
                                if track_count % 10 == 0 || track_count == 1 {
                                    self.update_track_progress(
                                        track_count,
                                        None,
                                        &track.name,
                                        &playlist_name,
                                    )
                                    .await;
                                }

                                debug!(
                                    "Processing track {}/?: {} - {}",
                                    track_count, track.name, playlist_name
                                );

                                // Store track and add to playlist
                                if let Err(e) = self
                                    .store_track_and_add_to_playlist(
                                        &track,
                                        playlist_id,
                                        position as i64,
                                    )
                                    .await
                                {
                                    error!("Failed to store track {}: {:?}", track.name, e);
                                    // Continue with next track
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to fetch track from playlist {}: {:?}",
                        playlist_name, e
                    );
                    // Continue with next track
                }
            }
        }

        // Update final progress
        {
            let mut progress = self.progress.write().await;
            progress.total_tracks = Some(track_count);
            progress.status = crate::sync::SyncStatus::Completed;
            progress.add_log(format!(
                "Sync completed for {}: {} tracks",
                playlist_name, track_count
            ));
        }

        info!(
            "Track sync completed for playlist {}: {} tracks",
            playlist_name, track_count
        );

        Ok(SyncResult::success(
            0,
            track_count,
            vec![playlist_name],
            track_names,
        ))
    }

    /// Sync tracks for all playlists in the database
    async fn sync_all_tracks(&mut self) -> Result<SyncResult> {
        info!("Starting sync for all playlists");

        // Update status to running
        {
            let mut progress = self.progress.write().await;
            progress.status = crate::sync::SyncStatus::Running;
        }

        debug!("All tracks sync: Querying database for existing playlists...");
        // First, get all playlists from database
        let db_playlists = sqlx::query_as::<_, (String, String)>(
            "SELECT playlist_id, name FROM service_playlists WHERE service = 'spotify'",
        )
        .fetch_all(&self.db)
        .await
        .context("Failed to fetch playlists from database")?;

        let total_playlists = db_playlists.len();
        debug!(
            "All tracks sync: Found {} playlists in database",
            total_playlists
        );
        let mut total_tracks = 0;
        let mut track_names = Vec::new();
        let mut playlist_names = Vec::new();

        for (i, (playlist_id, playlist_name)) in db_playlists.into_iter().enumerate() {
            // Check for cancellation between playlists
            if self.is_cancelled() {
                debug!("All tracks sync cancelled");
                return Ok(SyncResult::failed("Sync cancelled by user".to_string()));
            }

            debug!(
                "Syncing tracks for playlist {} of {}: {}",
                i + 1,
                total_playlists,
                playlist_name
            );
            playlist_names.push(playlist_name.clone());

            // Update playlist progress
            {
                let mut progress = self.progress.write().await;
                progress.current_playlist = Some(i + 1);
                progress.total_playlists = Some(total_playlists);
                progress.current_playlist_name = Some(playlist_name.clone());
                progress.add_log(format!(
                    "Processing playlist {}/{}: {}",
                    i + 1,
                    total_playlists,
                    playlist_name
                ));
            }

            // Sync tracks for this playlist
            match self.sync_tracks_for_playlist(&playlist_id).await {
                Ok(result) => {
                    total_tracks += result.track_count;
                    track_names.extend(result.track_names);
                }
                Err(e) => {
                    error!(
                        "Failed to sync tracks for playlist {}: {:?}",
                        playlist_id, e
                    );
                    // Continue with next playlist
                }
            }
        }

        // Update final progress
        {
            let mut progress = self.progress.write().await;
            progress.status = crate::sync::SyncStatus::Completed;
            progress.add_log(format!(
                "All tracks sync completed: {} tracks in {} playlists",
                total_tracks, total_playlists
            ));
        }

        info!(
            "All tracks sync completed: {} tracks in {} playlists",
            total_tracks, total_playlists
        );

        Ok(SyncResult::success(
            total_playlists,
            total_tracks,
            playlist_names,
            track_names,
        ))
    }

    /// Full sync: playlists + all tracks
    async fn sync_full(&mut self) -> Result<SyncResult> {
        info!("Starting full sync");

        // Update status to running
        {
            let mut progress = self.progress.write().await;
            progress.status = crate::sync::SyncStatus::Running;
            progress.add_log("Starting full sync (playlists + tracks)".to_string());
        }

        info!("Full sync: Starting playlist sync first...");
        // First sync playlists
        let playlists_result = self.sync_playlists_only().await?;
        info!(
            "Full sync: Playlist sync completed with {} playlists",
            playlists_result.playlist_count
        );

        // Check for cancellation
        if self.is_cancelled() {
            info!("Full sync cancelled after playlists");
            return Ok(SyncResult::failed("Sync cancelled by user".to_string()));
        }

        {
            let mut progress = self.progress.write().await;
            progress.add_log(format!(
                "Playlists synced: {}, now syncing tracks",
                playlists_result.playlist_count
            ));
        }

        // Then sync all tracks
        let tracks_result = self.sync_all_tracks().await?;

        // Combine results and update final status
        {
            let mut progress = self.progress.write().await;
            progress.status = crate::sync::SyncStatus::Completed;
            progress.add_log(format!(
                "Full sync completed: {} playlists, {} tracks",
                playlists_result.playlist_count, tracks_result.track_count
            ));
        }

        Ok(SyncResult::success(
            playlists_result.playlist_count,
            tracks_result.track_count,
            playlists_result.playlist_names,
            tracks_result.track_names,
        ))
    }

    /// Store a playlist in the database
    async fn store_playlist(&self, playlist: &SimplifiedPlaylist) -> Result<()> {
        let playlist_info = PlaylistInfo::from(playlist);

        // Serialize metadata to JSON
        let metadata_json = serde_json::to_string(&playlist_info)
            .context("Failed to serialize playlist metadata")?;

        // Start a transaction
        let mut tx = self.db.begin().await?;

        // Upsert playlist
        upsert_service_playlist(
            &mut tx,
            "spotify",
            &playlist_info.id,
            &playlist_info.name,
            playlist_info.description.as_deref(),
            Some(&metadata_json),
        )
        .await
        .context("Failed to upsert playlist")?;

        tx.commit().await?;

        Ok(())
    }

    /// Store a track and add it to a playlist
    async fn store_track_and_add_to_playlist(
        &self,
        track: &FullTrack,
        playlist_id: &str,
        position: i64,
    ) -> Result<()> {
        let track_info = TrackInfo::from(track);

        // Skip if no track ID (shouldn't happen)
        if track_info.id.is_empty() {
            warn!("Track has no ID, skipping: {}", track_info.name);
            return Ok(());
        }

        // Serialize metadata to JSON
        let metadata_json =
            serde_json::to_string(&track_info).context("Failed to serialize track metadata")?;

        // Start a transaction
        let mut tx = self.db.begin().await?;

        // Upsert track
        let db_track = upsert_service_track(
            &mut tx,
            "spotify",
            &track_info.id,
            &track_info.name,
            &track_info.artists,
            track_info.album.as_deref(),
            track_info.isrc.as_deref(),
            Some(track_info.duration_ms),
            Some(&metadata_json),
        )
        .await
        .context("Failed to upsert track")?;

        // Find the playlist in database
        let db_playlist = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM service_playlists WHERE service = 'spotify' AND playlist_id = ?",
        )
        .bind(playlist_id)
        .fetch_one(&mut *tx)
        .await
        .context("Failed to find playlist in database")?;

        // Add track to playlist
        add_track_to_playlist(&mut tx, db_playlist.0, db_track.id, Some(position as i32))
            .await
            .context("Failed to add track to playlist")?;

        tx.commit().await?;

        Ok(())
    }
}
