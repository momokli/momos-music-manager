//! Liked Songs (`/me/tracks`) → `service_playlists` mirror.
//!
//! The liked-songs playlist is a **mirror of the user's library**, not a
//! curated playlist: every liked track lands in a single synthetic playlist
//! (`playlist_kind = 'liked'`) carrying the Spotify like date as its
//! `added_at`. Unlike the global playlist poller, an already-present
//! membership is **not** skipped — the like date is refreshed on every pass
//! (relike semantics: unlike → relike moves the date at Spotify and thus our
//! `last_touched_at` signal). Do not "unify" this with the poller.
//!
//! Fetch and DB layers are deliberately split so the merge logic can be
//! exercised without HTTP: [`merge_liked_items`] and [`retire_missing_likes`]
//! only touch the database; [`sync_liked_songs`] drives Spotify.

use std::collections::HashSet;

use anyhow::Result;
use sqlx::{Pool, Sqlite};
use tokio_stream::StreamExt;
use tracing::{debug, info, warn};

use crate::db;
use crate::spotify::client::SpotifyClient;
use crate::spotify::cooldown::cooldown as spotify_cooldown;
use crate::spotify::models::TrackInfo;
use crate::spotify::retry::extract_retry_after_secs;
use crate::spotify::retry::format_duration;

/// `playlist_id` of the synthetic likes mirror inside `service_playlists`.
pub const LIKED_PLAYLIST_ID: &str = "spotify:liked";

/// One liked song, already fetched from Spotify.
pub struct LikedItem {
    /// Track metadata (public fields so tests can build it directly).
    pub track: TrackInfo,
    /// Spotify like date as a Unix timestamp (seconds).
    pub added_at: i64,
}

/// Outcome of one [`sync_liked_songs`] pass.
pub struct LikedSyncStats {
    /// Memberships written by the merge (new + refreshed).
    pub linked: usize,
    /// Memberships tombstoned as no longer liked.
    pub retired: usize,
    /// Spotify's reported total number of saved tracks.
    pub total: i64,
}

/// Upsert the likes mirror playlist row and return its DB id.
///
/// `upsert_service_playlist` does not know about `playlist_kind`, so the kind
/// is set in a second statement (ServicePlaylist::from_row ignores the extra
/// column anyway).
async fn ensure_liked_playlist(db: &Pool<Sqlite>, liked_playlist_id: &str) -> Result<i64> {
    let mut tx = db.begin().await?;
    let sp = db::upsert_service_playlist(
        &mut tx,
        "spotify",
        liked_playlist_id,
        "liked",
        None,
        None,
    )
    .await?;
    sqlx::query("UPDATE service_playlists SET playlist_kind = 'liked' WHERE id = ?")
        .bind(sp.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(sp.id)
}

/// Resolve the DB id of the likes mirror without creating it.
async fn liked_playlist_db_id(
    db: &Pool<Sqlite>,
    liked_playlist_id: &str,
) -> Result<Option<i64>> {
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM service_playlists WHERE service = 'spotify' AND playlist_id = ?",
    )
    .bind(liked_playlist_id)
    .fetch_optional(db)
    .await?;
    Ok(id)
}

/// Merge Spotify's liked songs into the mirror playlist.
///
/// Every item is written through [`db::add_track_to_playlist_with_added_at`],
/// **including** memberships that already exist, so `added_at` always reflects
/// the current Spotify like date. Returns the number of memberships written.
pub async fn merge_liked_items(
    db: &Pool<Sqlite>,
    items: &[LikedItem],
    liked_playlist_id: &str,
) -> Result<usize> {
    let db_playlist_id = ensure_liked_playlist(db, liked_playlist_id).await?;

    let mut linked = 0usize;
    for (index, item) in items.iter().enumerate() {
        if item.track.id.is_empty() {
            continue;
        }

        let metadata_json = serde_json::to_string(&item.track)?;
        let mut tx = db.begin().await?;
        let db_track = db::upsert_service_track(
            &mut tx,
            "spotify",
            &item.track.id,
            &item.track.name,
            &item.track.artists,
            item.track.album.as_deref(),
            item.track.isrc.as_deref(),
            Some(item.track.duration_ms),
            Some(&metadata_json),
        )
        .await?;

        db::add_track_to_playlist_with_added_at(
            &mut tx,
            db_playlist_id,
            db_track.id,
            Some(index as i32),
            Some(item.added_at),
        )
        .await?;
        tx.commit().await?;
        linked += 1;
    }

    debug!(
        "liked_sync: merged {} liked item(s) into '{}'",
        linked, liked_playlist_id
    );
    Ok(linked)
}

/// Soft-delete (tombstone) every active membership of the likes mirror whose
/// track is not in `current_track_ids`.
///
/// Only this playlist's memberships are touched — a track that is still part
/// of another playlist keeps that membership active.
pub async fn retire_missing_likes(
    db: &Pool<Sqlite>,
    liked_playlist_id: &str,
    current_track_ids: &HashSet<String>,
) -> Result<usize> {
    let Some(db_playlist_id) = liked_playlist_db_id(db, liked_playlist_id).await? else {
        return Ok(0);
    };

    let active: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT st.service_id
        FROM service_playlist_tracks spt
        JOIN service_tracks st ON st.id = spt.track_id
        WHERE spt.playlist_id = ? AND spt.deleted_at IS NULL
        "#,
    )
    .bind(db_playlist_id)
    .fetch_all(db)
    .await?;

    let now = chrono::Utc::now().timestamp();
    let mut retired = 0usize;
    for (service_id,) in active {
        if current_track_ids.contains(&service_id) {
            continue;
        }
        sqlx::query(
            r#"
            UPDATE service_playlist_tracks
               SET deleted_at = ?
             WHERE playlist_id = ?
               AND track_id = (SELECT id FROM service_tracks WHERE service = 'spotify' AND service_id = ?)
               AND deleted_at IS NULL
            "#,
        )
        .bind(now)
        .bind(db_playlist_id)
        .bind(&service_id)
        .execute(db)
        .await?;
        retired += 1;
    }

    if retired > 0 {
        info!(
            "liked_sync: retired {} unliked track(s) from '{}'",
            retired, liked_playlist_id
        );
    }
    Ok(retired)
}

/// Count active memberships of the likes mirror.
async fn active_liked_count(db: &Pool<Sqlite>, liked_playlist_id: &str) -> Result<i64> {
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM service_playlist_tracks spt
        JOIN service_playlists sp ON sp.id = spt.playlist_id
        WHERE sp.service = 'spotify' AND sp.playlist_id = ? AND spt.deleted_at IS NULL
        "#,
    )
    .bind(liked_playlist_id)
    .fetch_one(db)
    .await?;
    Ok(count)
}

/// Full cycle: fetch the user's liked songs from Spotify and reconcile the
/// local mirror.
pub async fn sync_liked_songs(
    db: &Pool<Sqlite>,
    spotify_client: &SpotifyClient,
) -> Result<LikedSyncStats> {
    // 1. Cheap "how many likes does Spotify think there are" probe.
    let total = spotify_client.get_saved_tracks_total().await?;

    // 2. Stream every saved track and build the fetch-layer items.
    let stream = spotify_client.get_saved_tracks().await?;
    tokio::pin!(stream);

    let mut items: Vec<LikedItem> = Vec::new();
    let mut current_track_ids: HashSet<String> = HashSet::new();

    while let Some(item_result) = stream.next().await {
        let saved = match item_result {
            Ok(saved) => saved,
            Err(e) => {
                if let Some(secs) = extract_retry_after_secs(&e) {
                    spotify_cooldown().note_retry_after(secs);
                    warn!(
                        "liked_sync: rate limited on /me/tracks. Retry-After: {} — \
                         aborting cycle, no inline retry",
                        format_duration(secs),
                    );
                    return Err(e);
                }
                warn!("liked_sync: saved-track error: {:#}", e);
                continue;
            }
        };

        let track = TrackInfo::from(&saved.track);
        if track.id.is_empty() {
            continue;
        }
        current_track_ids.insert(track.id.clone());
        items.push(LikedItem {
            track,
            added_at: saved.added_at.timestamp(),
        });
    }

    // 3. Merge into the mirror (refreshes added_at for existing likes too).
    let linked = merge_liked_items(db, &items, LIKED_PLAYLIST_ID).await?;

    // 4. Only when Spotify's total shrank is a missing like plausible —
    //    a full diff every cycle would be wasteful.
    let saved_count = active_liked_count(db, LIKED_PLAYLIST_ID).await?;
    let retired = if total < saved_count {
        retire_missing_likes(db, LIKED_PLAYLIST_ID, &current_track_ids).await?
    } else {
        0
    };

    // 5. Best-effort tag refresh so the `liked` tag resolves immediately.
    if linked > 0 {
        if let Err(e) = db::refresh_track_tags(db).await {
            warn!("liked_sync: failed to refresh track tags: {}", e);
        }
        if let Err(e) = db::refresh_file_resolved_tags(db).await {
            warn!("liked_sync: failed to refresh file_resolved_tags: {}", e);
        }
    }

    info!(
        "liked_sync: cycle done — total={}, linked={}, retired={}",
        total, linked, retired
    );

    Ok(LikedSyncStats {
        linked,
        retired,
        total,
    })
}
