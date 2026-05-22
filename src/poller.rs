//! Background poller for playlist subscriptions
//!
//! This module provides a [`start_subscription_poller`] function that spawns a
//! tokio background task. Every 30 seconds it checks for due playlist
//! subscriptions, fetches the current tracks from the Spotify API, upserts any
//! new tracks into the database, and logs what changed.
//!
//! # Architecture
//!
//! The poller runs an infinite loop with a 30-second tick. On each tick:
//!
//! 1. Query the database for subscriptions whose `last_polled_at + poll_interval_secs`
//!    is in the past (or that have never been polled).
//! 2. For each due subscription, create a [`SpotifyClient`] from stored OAuth tokens.
//! 3. Fetch the full playlist metadata and all its tracks via the Spotify API.
//! 4. Upsert the playlist record and ensure each track is stored and linked.
//! 5. Log any new tracks that appeared since the last poll.
//! 6. Update the `last_polled_at` timestamp on the subscription.
//!
//! The loop can be cleanly cancelled by signalling the provided
//! [`CancellationToken`].

use anyhow::{Context, Result};
use rspotify::model::PlayableItem;
use rspotify::prelude::Id;

use sqlx::{Pool, Sqlite};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::ServiceCredentials;
use crate::db;
use crate::deemix::DeemixClient;
use crate::spotify::client::SpotifyClient;
use crate::spotify::models::TrackInfo;
use crate::spotify::retry::{extract_retry_after_clamped, format_duration};

use std::time::Duration;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start the subscription poller background task.
///
/// Spawns a loop that runs until the provided [`CancellationToken`] is
/// signalled.  Every 30 seconds it queries for due subscriptions, polls each
/// one via the Spotify API, and records any new tracks.
///
/// # Arguments
/// * `db`          – SQLite connection pool (shared across the application).
/// * `credentials` – Service credentials loaded from `.env`.
/// * `cancel_token` – Token used to signal shutdown.
pub async fn start_subscription_poller(
    db: Pool<Sqlite>,
    credentials: ServiceCredentials,
    cancel_token: CancellationToken,
    subscription_count: i64,
) {
    if subscription_count == 0 {
        info!("Subscription poller started (idle, 0 subscriptions)");
    } else {
        info!(
            "Subscription poller started ({sub} subscription(s), interval: 30s)",
            sub = subscription_count,
        );
    }

    loop {
        // Short sleep first so the outer task has a chance to register
        // before the initial batch of work.
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;

        if cancel_token.is_cancelled() {
            info!("Subscription poller: cancellation requested, shutting down");
            break;
        }

        // -- Fetch due subscriptions ---------------------------------------
        let subscriptions = match db::get_due_subscriptions(&db).await {
            Ok(subs) => subs,
            Err(e) => {
                error!(
                    "Subscription poller: failed to query due subscriptions: {:#}",
                    e
                );
                continue;
            }
        };

        let due_count = subscriptions.len();
        debug!(
            "Subscription poller: found {} due subscription(s)",
            due_count
        );

        // -- Create Spotify client once per cycle -----------------------------
        let spotify_client = match SpotifyClient::from_stored_tokens(db.clone(), &credentials).await
        {
            Ok(client) => client,
            Err(e) => {
                error!(
                    "Subscription poller: failed to create Spotify client, skipping cycle: {:#}",
                    e,
                );
                continue;
            }
        };

        // -- Poll each due subscription ------------------------------------
        for subscription in &subscriptions {
            if cancel_token.is_cancelled() {
                break;
            }

            if let Err(e) = poll_subscribed_playlist(&db, &spotify_client, subscription).await {
                error!(
                    "Subscription poller: error polling subscription {} (playlist_id={}): {:#}",
                    subscription.id, subscription.playlist_id, e,
                );
                // Continue to the next subscription despite the error.
            }

            // Mark the subscription as polled (even on partial errors so we
            // don't retry immediately and risk hitting rate limits).
            if let Err(e) = db::update_subscription_last_polled(&db, subscription.id).await {
                error!(
                    "Subscription poller: failed to update last_polled_at for subscription {}: {:#}",
                    subscription.id, e,
                );
            }

            // Small delay between subscriptions to avoid rate limit bursts
            tokio::time::sleep(Duration::from_millis(300)).await;
        }

        info!(
            "Subscription poller: checked {} due subscription(s)",
            due_count,
        );
    }

    info!("Subscription poller: stopped");
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Poll a single subscribed playlist.
///
/// 1. Fetches the playlist metadata from Spotify.
/// 2. Upserts the playlist into the `service_playlists` table.
/// 3. Updates the subscription's `service_playlist_id` foreign key.
/// 4. Streams all tracks and stores any that are not yet associated with this
///    playlist.
/// 5. Logs each new track together with a list of other playlists that already
///    contain it.
async fn poll_subscribed_playlist(
    db: &Pool<Sqlite>,
    spotify_client: &SpotifyClient,
    subscription: &db::PlaylistSubscription,
) -> Result<()> {
    // -- Fetch playlist metadata (with retry) -------------------------------
    let playlist = {
        let mut attempt = 0;
        loop {
            match spotify_client.get_playlist(&subscription.playlist_id).await {
                Ok(p) => break p,
                Err(e) => {
                    if let Some(raw_secs) = extract_retry_after_clamped(&e) {
                        attempt += 1;
                        if attempt >= 3 {
                            return Err(e)
                                .context("Failed to fetch playlist from Spotify after 3 retries");
                        }
                        let sleep_secs = raw_secs + 1;
                        warn!(
                            "Subscription poller: rate limited fetching playlist '{}'. \
                             Retry-After: {} ({raw_secs}s), attempt {attempt}/3, waiting {sleep_secs}s",
                            subscription.playlist_id,
                            format_duration(raw_secs),
                        );
                        tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
                    } else {
                        return Err(e).context("Failed to fetch playlist from Spotify");
                    }
                }
            }
        }
    };

    let playlist_name = &playlist.name;
    let playlist_description = playlist.description.as_deref();

    debug!(
        "Polling playlist '{}' (spotify_id={})",
        playlist_name, subscription.playlist_id,
    );

    // -- Upsert the playlist record ----------------------------------------
    // Use a transaction so we can atomically upsert the playlist and re-read
    // its DB-generated id.
    let db_playlist_id = {
        let mut tx = db.begin().await.context("Failed to begin transaction")?;

        let sp = db::upsert_service_playlist(
            &mut tx,
            "spotify",
            &subscription.playlist_id,
            playlist_name,
            playlist_description,
            None, // metadata_json – not needed for poller
        )
        .await
        .context("Failed to upsert service playlist")?;

        tx.commit().await.context("Failed to commit transaction")?;

        sp.id
    };

    // -- Link the subscription to the DB playlist record -------------------
    if subscription.service_playlist_id != Some(db_playlist_id) {
        db::update_subscription_playlist_id(db, subscription.id, db_playlist_id)
            .await
            .context("Failed to update subscription playlist_id")?;
    }

    // -- Stream tracks from Spotify and store new ones (with retry) --------
    let track_stream = {
        let mut attempt = 0;
        loop {
            match spotify_client
                .get_playlist_tracks(&subscription.playlist_id)
                .await
            {
                Ok(s) => break s,
                Err(e) => {
                    if let Some(raw_secs) = extract_retry_after_clamped(&e) {
                        attempt += 1;
                        if attempt >= 3 {
                            return Err(e).context("Failed to get playlist tracks after 3 retries");
                        }
                        let sleep_secs = raw_secs + 1;
                        warn!(
                            "Subscription poller: rate limited fetching tracks for playlist '{}'. \
                             Retry-After: {} ({raw_secs}s), attempt {attempt}/3, waiting {sleep_secs}s",
                            subscription.playlist_id,
                            format_duration(raw_secs),
                        );
                        tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
                    } else {
                        return Err(e).context("Failed to get playlist tracks");
                    }
                }
            }
        }
    };

    tokio::pin!(track_stream);

    let mut position: i64 = 0;
    let mut new_tracks_found = false;

    while let Some(item_result) = track_stream.next().await {
        let item = match item_result {
            Ok(item) => item,
            Err(e) => {
                warn!(
                    "Subscription poller: error fetching track from '{}': {:#}",
                    playlist_name, e,
                );
                continue;
            }
        };

        // Handle tracks and episodes (store both so counts match).
        let (track_info, is_episode) = match item.track {
            Some(PlayableItem::Track(track)) => (TrackInfo::from(&track), false),
            Some(PlayableItem::Episode(episode)) => (
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

        // Skip items with empty IDs
        if track_info.id.is_empty() {
            debug!("Item '{}' has no Spotify ID, skipping", track_info.name);
            continue;
        }
        let track_service_id = track_info.id.clone();

        position += 1;

        // Check whether this track is already linked to the DB playlist.
        let already_exists: bool = {
            let row: Option<(i64,)> = sqlx::query_as(
                "SELECT 1 FROM service_playlist_tracks spt \
                 JOIN service_tracks st ON spt.track_id = st.id \
                 WHERE st.service = 'spotify' AND st.service_id = ? \
                   AND spt.playlist_id = ?",
            )
            .bind(&track_service_id)
            .bind(db_playlist_id)
            .fetch_optional(db)
            .await
            .context("Failed to check existing track association")?;

            row.is_some()
        };

        if already_exists {
            debug!(
                "Item '{}' already in playlist '{}', skipping",
                track_info.name, playlist_name,
            );
            continue;
        }

        // -- New item – store and link ------------------------------------
        let added_at: Option<i64> = item.added_at.map(|dt| dt.timestamp());
        let db_track_id = store_track_info_and_add_to_playlist(
            db,
            &track_info,
            &subscription.playlist_id,
            position,
            added_at,
        )
        .await
        .context("Failed to store item and add to playlist")?;

        new_tracks_found = true;

        // Build artist string for logging (already a comma-separated string from TrackInfo).
        let artists = track_info.artists.clone();

        // Find *other* playlists that already contain this track (i.e. all
        // associations except the one we just created).
        let other_playlists = {
            let associations = db::get_track_playlist_associations(db, db_track_id)
                .await
                .context("Failed to query track playlist associations")?;

            associations
                .into_iter()
                .filter(|(_, pl_id, _)| pl_id != &subscription.playlist_id)
                .map(|(name, _, _)| name)
                .collect::<Vec<_>>()
        };

        if other_playlists.is_empty() {
            info!(
                "New {} '{}' by {} added to '{}'",
                if is_episode { "episode" } else { "track" },
                track_info.name,
                artists,
                playlist_name,
            );
        } else {
            info!(
                "New {} '{}' by {} added to '{}' (also in: {})",
                if is_episode { "episode" } else { "track" },
                track_info.name,
                artists,
                playlist_name,
                other_playlists.join(", "),
            );
        }
    }

    // -- Auto-download via deemix on first poll or new tracks -----------
    if subscription.last_polled_at.is_none() || new_tracks_found {
        match DeemixClient::from_db(db.clone()).await {
            Some(client) => {
                let url = format!(
                    "https://open.spotify.com/playlist/{}",
                    subscription.playlist_id
                );
                match client.ensure_queued(&url).await {
                    Ok(()) => {
                        // Insert/update local deemix_downloads table so the
                        // Playlists page shows the correct status immediately
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;
                        let _ = sqlx::query(
                            "INSERT INTO deemix_downloads (spotify_playlist_url, status, created_at, updated_at)
                             VALUES (?, 'queued', ?, ?)
                             ON CONFLICT(spotify_playlist_url) DO UPDATE SET
                                 status = 'queued',
                                 error_message = NULL,
                                 updated_at = excluded.updated_at"
                        )
                        .bind(&url)
                        .bind(now)
                        .bind(now)
                        .execute(db)
                        .await;

                        info!(
                            "Subscription poller: auto-download triggered for '{}'",
                            playlist_name,
                        );
                    }
                    Err(e) => warn!(
                        "Subscription poller: failed to trigger deemix download for '{}': {:#}",
                        playlist_name, e,
                    ),
                }
            }
            None => debug!(
                "Subscription poller: deemix not configured, skipping auto-download for '{}'",
                playlist_name,
            ),
        }
    }

    // -- Update the playlist's remote track counts -----------------------
    // After streaming all tracks, update the remote_track_count so the
    // frontend displays accurate "local / unique / total" stats.
    match db.acquire().await {
        Ok(mut conn) => {
            let remote_count = playlist.tracks.total as i64;
            if let Err(e) = crate::db::update_playlist_fetch_tracking(
                &mut conn,
                "spotify",
                &subscription.playlist_id,
                remote_count,
            )
            .await
            {
                warn!(
                    "Subscription poller: failed to update fetch tracking for playlist '{}' \
                     (playlist_id={}): {:#}",
                    playlist_name, subscription.playlist_id, e,
                );
            }
        }
        Err(e) => {
            warn!(
                "Subscription poller: failed to acquire DB connection for fetch tracking update: {:#}",
                e,
            );
        }
    }

    Ok(())
}

/// Store a TrackInfo (track or episode) and link it to a playlist.
/// Works with both tracks and episodes — the TrackInfo already encapsulates
/// all the metadata.
async fn store_track_info_and_add_to_playlist(
    db: &Pool<Sqlite>,
    track_info: &TrackInfo,
    playlist_id: &str,
    position: i64,
    added_at: Option<i64>,
) -> Result<i64> {
    let metadata_json =
        serde_json::to_string(track_info).context("Failed to serialise track metadata")?;

    let mut tx = db.begin().await.context("Failed to begin transaction")?;

    // Upsert the track record.
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
    .await
    .context("Failed to upsert service track")?;

    // Find the local playlist that corresponds to the Spotify `playlist_id`.
    let db_playlist: (i64,) = sqlx::query_as(
        "SELECT id FROM service_playlists WHERE service = 'spotify' AND playlist_id = ?",
    )
    .bind(playlist_id)
    .fetch_one(&mut *tx)
    .await
    .context("Failed to find playlist in database")?;

    // Link the track to the playlist.
    db::add_track_to_playlist_with_added_at(
        &mut tx,
        db_playlist.0,
        db_track.id,
        Some(position as i32),
        added_at,
    )
    .await
    .context("Failed to add track to playlist")?;

    tx.commit().await.context("Failed to commit transaction")?;

    Ok(db_track.id)
}
