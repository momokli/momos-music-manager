//! Background global poller for all Spotify playlists
//!
//! This module provides a [`start_global_poller`] function that spawns a tokio
//! background task. It runs in a configurable interval (default 15 minutes)
//! and checks ALL Spotify playlists for changes using `snapshot_id` comparison.
//!
//! # How it works
//!
//! 1. Fetch ALL user playlists from Spotify via `GET /me/playlists` (paginated)
//! 2. For each playlist, compare the stored `snapshot_id` in the DB with the
//!    current value from the API
//! 3. If snapshot matches → skip (no changes, 0 API calls for that playlist)
//! 4. If snapshot differs or playlist is new → fetch all tracks (paginated)
//!    and upsert them, then update `snapshot_id`, `last_fetched_at`,
//!    `remote_track_count`
//! 5. If a playlist exists in the DB but not in the API response → log as
//!    deleted (mark as inactive)
//!
//! # API traffic estimate (200 playlists, 5 changed per cycle)
//!
//! - 4 API calls to fetch the playlist list (50/page)
//! - ~10 API calls to fetch tracks for changed playlists (2 pages each)
//! - **Total: ~14 API calls every 15 minutes**

use std::time::Duration;

use anyhow::Result;
use rspotify::prelude::Id;
use sqlx::{Pool, Sqlite};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::ServiceCredentials;
use crate::db;
use crate::spotify::client::SpotifyClient;
use crate::spotify::models::TrackInfo;
use crate::spotify::retry::extract_retry_after_secs;
use crate::spotify::retry::format_duration;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start the global playlist poller background task.
///
/// Spawns a loop that runs every `interval_secs` until the
/// provided [`CancellationToken`] is signalled.
pub async fn start_global_poller(
    db: Pool<Sqlite>,
    config: ServiceCredentials,
    interval_secs: u64,
    cancel_token: CancellationToken,
) {
    if interval_secs == 0 {
        info!("Global poller disabled (interval=0)");
        return;
    }

    info!("Global poller started (interval: {}s)", interval_secs);

    // Wait one interval before first poll so the server has time to fully start
    tokio::time::sleep(Duration::from_secs(interval_secs)).await;

    loop {
        if cancel_token.is_cancelled() {
            info!("Global poller: cancellation requested, shutting down");
            break;
        }

        if let Err(e) = run_poll_cycle(&db, &config, &cancel_token).await {
            error!("Global poller: poll cycle failed: {:#}", e);
        }

        // Sleep between cycles, cancellable
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval_secs)) => {}
            _ = cancel_token.cancelled() => {
                info!("Global poller: cancellation requested, shutting down");
                break;
            }
        }
    }

    info!("Global poller: stopped");
}

/// Run a single poll cycle: fetch all playlists, compare snapshots, sync changes.
async fn run_poll_cycle(
    db: &Pool<Sqlite>,
    config: &ServiceCredentials,
    cancel_token: &CancellationToken,
) -> Result<()> {
    // Create Spotify client
    let spotify_client = match SpotifyClient::from_stored_tokens(db.clone(), config).await {
        Ok(client) => client,
        Err(e) => {
            error!("Global poller: failed to create Spotify client: {:#}", e);
            return Err(e);
        }
    };

    // Refresh token to ensure API access
    if let Err(e) = spotify_client.refresh_token_if_needed().await {
        error!("Global poller: token refresh failed: {:#}", e);
        return Err(e);
    }

    // ── Step 1: Fetch stored snapshots from DB ───────────────────────────
    // Store (db_id, snapshot_id) so we can query track counts for staleness
    // checks when the snapshot matches.
    let db_snapshots: std::collections::HashMap<String, (i64, Option<String>)> =
        match db::get_spotify_playlist_snapshots(db).await {
            Ok(rows) => rows
                .into_iter()
                .map(|(db_id, pid, sid)| (pid, (db_id, sid)))
                .collect(),
            Err(e) => {
                error!("Global poller: failed to query stored snapshots: {:#}", e);
                return Err(e);
            }
        };

    let mut spotify_playlists: Vec<SimplifiedPlaylistData> = Vec::new();

    // ── Step 2: Fetch all playlists from Spotify ─────────────────────────
    let stream = {
        let mut attempt = 0;
        loop {
            match spotify_client.get_user_playlists().await {
                Ok(stream) => break stream,
                Err(e) => {
                    if let Some(secs) = extract_retry_after_secs(&e) {
                        let clamped = secs.min(300);
                        attempt += 1;
                        if attempt >= 3 {
                            error!(
                                "Global poller: failed to get playlists stream after {} retries (rate limited)",
                                attempt
                            );
                            return Err(e);
                        }
                        let sleep_secs = clamped + 1;
                        warn!(
                            "Global poller: rate limited fetching playlist list. Retry-After: {} ({secs}s total, clamped to {clamped}s), attempt {attempt}/3, waiting {sleep_secs}s",
                            format_duration(secs),
                        );
                        tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
                    } else {
                        error!("Global poller: failed to get playlists stream: {:#}", e);
                        return Err(e);
                    }
                }
            }
        }
    };

    tokio::pin!(stream);
    while let Some(item) = stream.next().await {
        match item {
            Ok(playlist) => {
                spotify_playlists.push(SimplifiedPlaylistData {
                    id: playlist.id.id().to_string(),
                    name: playlist.name.clone(),
                    snapshot_id: playlist.snapshot_id.clone(),
                    track_count: playlist.tracks.total as i64,
                });
            }
            Err(e) => {
                if let Some(secs) = extract_retry_after_secs(&e) {
                    warn!(
                        "Global poller: rate limited fetching playlist list. Retry-After: {}s",
                        secs
                    );
                    tokio::time::sleep(Duration::from_secs(secs + 1)).await;
                    continue;
                }
                warn!("Global poller: error fetching playlist: {:#}", e);
                continue;
            }
        }
    }

    let spotify_count = spotify_playlists.len();
    debug!(
        "Global poller: fetched {} playlists from Spotify",
        spotify_count
    );

    // ── Step 3: Process each playlist ────────────────────────────────────
    let mut new_playlists = 0;
    let mut changed_playlists = 0;
    let mut skipped_playlists = 0;
    let mut new_tracks_total = 0;

    for sp in &spotify_playlists {
        if cancel_token.is_cancelled() {
            break;
        }

        let stored_info = db_snapshots.get(&sp.id);

        match stored_info {
            None => {
                // New playlist — not in DB at all
                new_playlists += 1;
                match fetch_and_store_playlist_tracks(db, &spotify_client, sp, cancel_token).await {
                    Ok(track_count) => {
                        new_tracks_total += track_count;
                        info!(
                            "Global poller: discovered new playlist '{}' with {} track(s)",
                            sp.name, track_count
                        );
                    }
                    Err(e) => {
                        error!(
                            "Global poller: failed to sync new playlist '{}': {:#}",
                            sp.name, e
                        );
                    }
                }
            }
            Some((db_id, Some(stored_sid))) if stored_sid == &sp.snapshot_id => {
                // Snapshot matches — check if stale or cold-start
                match db::get_playlist_staleness(db, *db_id).await {
                    Ok((local_count, remote_unique_count, remote_track_count, last_fetched_at)) => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;

                        let is_stale = local_count < remote_unique_count;
                        let is_cold_start = local_count == 0
                            && remote_track_count > 0
                            && !last_fetched_at
                                .map(|ts| now - ts <= config.cold_start_threshold_secs as i64)
                                .unwrap_or(false);

                        if is_stale || is_cold_start {
                            changed_playlists += 1;
                            match fetch_and_store_playlist_tracks(
                                db,
                                &spotify_client,
                                sp,
                                cancel_token,
                            )
                            .await
                            {
                                Ok(count) => {
                                    new_tracks_total += count;
                                    info!(
                                        "Global poller: {} playlist '{}' healed, {} new track(s)",
                                        if is_stale { "stale" } else { "cold-start" },
                                        sp.name,
                                        count,
                                    );
                                }
                                Err(e) => {
                                    error!(
                                        "Global poller: failed to heal {} playlist '{}': {:#}",
                                        if is_stale { "stale" } else { "cold-start" },
                                        sp.name,
                                        e,
                                    );
                                }
                            }
                        } else {
                            skipped_playlists += 1;
                            let reason = if local_count == 0 && remote_track_count == 0 {
                                "empty playlist".to_string()
                            } else {
                                format!(
                                    "local={}, remote_unique={}",
                                    local_count, remote_unique_count
                                )
                            };
                            debug!(
                                "Global poller: skipping '{}' — snapshot matches, {}",
                                sp.name, reason,
                            );
                        }
                    }
                    Err(e) => {
                        error!(
                            "Global poller: failed to check staleness for '{}': {:#}",
                            sp.name, e,
                        );
                        skipped_playlists += 1;
                    }
                }
            }
            Some(_) => {
                // Snapshot differs or was NULL — changed
                changed_playlists += 1;
                match fetch_and_store_playlist_tracks(db, &spotify_client, sp, cancel_token).await {
                    Ok(track_count) => {
                        if track_count > 0 {
                            new_tracks_total += track_count;
                            info!(
                                "Global poller: playlist '{}' changed, {} new track(s)",
                                sp.name, track_count
                            );
                        }
                    }
                    Err(e) => {
                        error!(
                            "Global poller: failed to sync changed playlist '{}': {:#}",
                            sp.name, e
                        );
                    }
                }
            }
        }

        // Small delay between playlists to stay under rate limits
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // ── Step 4: Detect deleted playlists ─────────────────────────────────
    let spotify_ids: std::collections::HashSet<String> =
        spotify_playlists.iter().map(|sp| sp.id.clone()).collect();

    let mut deleted_count = 0;
    if let Ok(rows) = db::get_spotify_playlist_snapshots(db).await {
        for (db_id, pid, _) in rows {
            if !spotify_ids.contains(&pid) {
                warn!(
                    "Global poller: playlist {} (DB id={}) no longer exists on Spotify",
                    pid, db_id
                );
                if let Err(e) = db::mark_playlist_inactive(db, db_id).await {
                    error!(
                        "Global poller: failed to mark playlist {} inactive: {:#}",
                        db_id, e
                    );
                }
                deleted_count += 1;
            }
        }
    }

    // ── Step 5: Summary ──────────────────────────────────────────────────
    info!(
        "Global poller: cycle complete — {} playlists ({} new, {} changed, {} skipped, {} deleted, {} new track(s))",
        spotify_count,
        new_playlists,
        changed_playlists,
        skipped_playlists,
        deleted_count,
        new_tracks_total,
    );

    // ── Step 6: Refresh materialized tag tables if tracks were added ────
    if new_tracks_total > 0 {
        if let Err(e) = crate::db::refresh_file_resolved_tags(db).await {
            error!("Global poller: failed to refresh file_resolved_tags: {}", e);
        }
        if let Err(e) = crate::db::refresh_track_resolved_tags(db).await {
            error!(
                "Global poller: failed to refresh track_resolved_tags: {}",
                e
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct SimplifiedPlaylistData {
    id: String,
    name: String,
    snapshot_id: String,
    track_count: i64,
}

/// Fetch all tracks for a Spotify playlist and store them.
/// Returns the number of NEW tracks added.
async fn fetch_and_store_playlist_tracks(
    db: &Pool<Sqlite>,
    spotify_client: &SpotifyClient,
    playlist: &SimplifiedPlaylistData,
    cancel_token: &CancellationToken,
) -> Result<i64> {
    // ── Upsert the playlist record ───────────────────────────────────────
    let db_playlist_id = {
        let mut tx = db.begin().await?;
        let sp = db::upsert_service_playlist(
            &mut tx,
            "spotify",
            &playlist.id,
            &playlist.name,
            None,
            None,
        )
        .await?;
        tx.commit().await?;
        sp.id
    };

    // ── Fetch and store tracks ──────────────────────────────────────────
    let track_stream = spotify_client.get_playlist_tracks(&playlist.id).await?;
    tokio::pin!(track_stream);

    let mut new_track_count = 0i64;
    let mut position = 0i64;

    while let Some(item_result) = track_stream.next().await {
        if cancel_token.is_cancelled() {
            break;
        }

        let item = match item_result {
            Ok(item) => item,
            Err(e) => {
                if let Some(secs) = extract_retry_after_secs(&e) {
                    let clamped = secs.min(300);
                    warn!(
                        "Global poller: rate limited on '{}' tracks. Retry-After: {} ({secs}s total, clamped to {clamped}s)",
                        playlist.name,
                        format_duration(secs),
                    );
                    tokio::time::sleep(Duration::from_secs(clamped + 1)).await;
                    continue;
                }
                warn!(
                    "Global poller: track error from '{}': {:#}",
                    playlist.name, e
                );
                continue;
            }
        };

        let (track_info, is_episode): (TrackInfo, bool) = match item.track {
            Some(rspotify::model::PlayableItem::Track(track)) => (TrackInfo::from(&track), false),
            Some(rspotify::model::PlayableItem::Episode(episode)) => (
                TrackInfo {
                    id: episode.id.id().to_string(),
                    name: episode.name.clone(),
                    artists: episode.show.name.clone(),
                    album: Some(episode.show.name.clone()),
                    isrc: None,
                    duration_ms: episode.duration.num_milliseconds(),
                    track_number: None,
                    disc_number: None,
                    explicit: episode.explicit,
                    popularity: None,
                },
                true,
            ),
            _ => continue,
        };

        if track_info.id.is_empty() {
            continue;
        }

        position += 1;

        // Check if already linked
        let already_exists: bool = {
            let row: Option<(i64,)> = sqlx::query_as(
                "SELECT 1 FROM service_playlist_tracks spt \
                 JOIN service_tracks st ON spt.track_id = st.id \
                 WHERE st.service = 'spotify' AND st.service_id = ? \
                   AND spt.playlist_id = ?",
            )
            .bind(&track_info.id)
            .bind(db_playlist_id)
            .fetch_optional(db)
            .await?;
            row.is_some()
        };

        if already_exists {
            continue;
        }

        // ── Store new track ──────────────────────────────────────────────
        new_track_count += 1;
        let metadata_json = serde_json::to_string(&track_info)?;

        let mut tx = db.begin().await?;
        let db_track = db::upsert_service_track(
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
        .await?;

        let added_at: Option<i64> = item.added_at.map(|dt| dt.timestamp());
        db::add_track_to_playlist_with_added_at(
            &mut tx,
            db_playlist_id,
            db_track.id,
            Some(position as i32),
            added_at,
        )
        .await?;
        tx.commit().await?;

        if new_track_count <= 5 {
            info!(
                "Global poller: new {} '{}' by {} added to '{}'",
                if is_episode { "episode" } else { "track" },
                track_info.name,
                track_info.artists,
                playlist.name,
            );
        }
    }

    // ── Update snapshot + fetch tracking ─────────────────────────────────
    db::update_playlist_snapshot(db, &playlist.id, &playlist.snapshot_id).await?;
    {
        let mut conn = db.acquire().await?;
        db::update_playlist_fetch_tracking(
            &mut conn,
            "spotify",
            &playlist.id,
            playlist.track_count,
        )
        .await?;
    }

    if new_track_count == 0 {
        debug!("Global poller: synced '{}' — no new tracks", playlist.name,);
    } else {
        info!(
            "Global poller: synced '{}' — {} new track(s) (total remote: {})",
            playlist.name, new_track_count, playlist.track_count,
        );
    }

    Ok(new_track_count)
}
