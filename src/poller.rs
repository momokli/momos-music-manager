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
use serde_json;
use sqlx::{Pool, Sqlite};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::ServiceCredentials;
use crate::db;
use crate::spotify::client::SpotifyClient;
use crate::spotify::models::TrackInfo;

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
) {
    info!("Subscription poller started (interval: 30s)");

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

        // -- Poll each due subscription ------------------------------------
        for subscription in &subscriptions {
            if cancel_token.is_cancelled() {
                break;
            }

            // Create a Spotify client from the persisted OAuth tokens.
            let spotify_client = match SpotifyClient::from_stored_tokens(db.clone(), &credentials)
                .await
            {
                Ok(client) => client,
                Err(e) => {
                    error!(
                        "Subscription poller: failed to create Spotify client for subscription {} \
                         (playlist_id={}): {:#}",
                        subscription.id, subscription.playlist_id, e,
                    );
                    continue;
                }
            };

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
    // -- Fetch playlist metadata -------------------------------------------
    let playlist = spotify_client
        .get_playlist(&subscription.playlist_id)
        .await
        .context("Failed to fetch playlist from Spotify")?;

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

    // -- Stream tracks from Spotify and store new ones ---------------------
    let track_stream = spotify_client
        .get_playlist_tracks(&subscription.playlist_id)
        .await
        .context("Failed to get playlist tracks stream")?;

    tokio::pin!(track_stream);

    let mut position: i64 = 0;

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

        // Only handle real tracks; skip episodes / placeholders.
        let track = match item.track {
            Some(PlayableItem::Track(track)) => track,
            _ => continue,
        };

        position += 1;

        // Extract the Spotify track ID string.
        let track_service_id = match track.id.as_ref() {
            Some(id) => id.id().to_string(),
            None => {
                debug!("Track '{}' has no Spotify ID, skipping", track.name);
                continue;
            }
        };

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
                "Track '{}' already in playlist '{}', skipping",
                track.name, playlist_name,
            );
            continue;
        }

        // -- New track – store and link ------------------------------------
        let db_track_id =
            store_track_and_add_to_playlist_simple(db, &track, &subscription.playlist_id, position)
                .await
                .context("Failed to store track and add to playlist")?;

        // Build artist string for logging.
        let artists: String = track
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        // Find *other* playlists that already contain this track (i.e. all
        // associations except the one we just created).
        let other_playlists = {
            let mut tx = db.begin().await?;
            let associations = db::get_track_playlist_associations(&mut tx, db_track_id)
                .await
                .context("Failed to query track playlist associations")?;
            tx.commit().await?;

            associations
                .into_iter()
                .filter(|(_, pl_id, _)| pl_id != &subscription.playlist_id)
                .map(|(name, _, _)| name)
                .collect::<Vec<_>>()
        };

        if other_playlists.is_empty() {
            info!(
                "New track '{}' by {} added to '{}'",
                track.name, artists, playlist_name,
            );
        } else {
            info!(
                "New track '{}' by {} added to '{}' (also in: {})",
                track.name,
                artists,
                playlist_name,
                other_playlists.join(", "),
            );
        }
    }

    Ok(())
}

/// Simplified version of [`SpotifySyncWorker::store_track_and_add_to_playlist`].
///
/// 1. Serialises the track metadata to JSON.
/// 2. Starts a transaction.
/// 3. Upserts the track into `service_tracks`.
/// 4. Looks up the local playlist row for the given Spotify `playlist_id`.
/// 5. Links the track to the playlist via `service_playlist_tracks`.
/// 6. Commits the transaction.
/// 7. Returns the DB-internal `id` of the track.
async fn store_track_and_add_to_playlist_simple(
    db: &Pool<Sqlite>,
    track: &rspotify::model::track::FullTrack,
    playlist_id: &str,
    position: i64,
) -> Result<i64> {
    let track_info = TrackInfo::from(track);
    let metadata_json =
        serde_json::to_string(&track_info).context("Failed to serialise track metadata")?;

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
    db::add_track_to_playlist(&mut tx, db_playlist.0, db_track.id, Some(position as i32))
        .await
        .context("Failed to add track to playlist")?;

    tx.commit().await.context("Failed to commit transaction")?;

    Ok(db_track.id)
}
