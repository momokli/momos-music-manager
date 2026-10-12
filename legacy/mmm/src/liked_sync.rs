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

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use sqlx::{Pool, Sqlite};
use tokio_stream::StreamExt;
use tracing::{debug, info, warn};

use crate::db;
use crate::spotify::client::SpotifyClient;
use crate::spotify::cooldown::cooldown as spotify_cooldown;
use crate::spotify::metrics::{self, Source};
use crate::spotify::models::TrackInfo;
use crate::spotify::retry::extract_retry_after_secs;
use crate::spotify::retry::format_duration;

/// `playlist_id` of the synthetic likes mirror inside `service_playlists`.
pub const LIKED_PLAYLIST_ID: &str = "spotify:liked";

/// Number of consecutive unchanged items (same stored `added_at`) after which a
/// newest-first scan stops early: everything further down is older and unchanged.
const EARLY_STOP_UNCHANGED: usize = 100;

/// Spotify's page size for `/me/tracks` — used only to estimate request counts
/// for the metrics counters.
const SAVED_TRACKS_PAGE_SIZE: u64 = 50;

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

/// Whether a streamed liked item is already known locally with the exact same
/// `added_at`.
///
/// `stored` maps `service_tracks.service_id` → the `added_at` currently held for
/// that membership (itself nullable). A stored `NULL` never counts as unchanged,
/// so the first pass that discovers a real date rewrites the row.
fn is_unchanged(stored: &HashMap<String, Option<i64>>, id: &str, added_at: i64) -> bool {
    matches!(stored.get(id), Some(Some(stored_added)) if *stored_added == added_at)
}

/// Run-length counter over the newest-first liked stream.
///
/// An unchanged item extends the run; any change (a new like, or a relike that
/// moved an item to the front with a newer `added_at`) resets it. Because the
/// list is ordered by `added_at` descending, a sufficiently long run of unchanged
/// items guarantees everything below is unchanged too — so the scan can stop.
#[derive(Default)]
struct UnchangedRun {
    run: usize,
}

impl UnchangedRun {
    /// Feed one item's classification; returns nothing, just advances the run.
    fn observe(&mut self, unchanged: bool) {
        if unchanged {
            self.run += 1;
        } else {
            self.run = 0;
        }
    }

    /// Whether the current run has reached `threshold`.
    fn reached(&self, threshold: usize) -> bool {
        self.run >= threshold
    }
}

/// Interpret a streamed `/me/tracks` item error.
///
/// A `Retry-After` (429) is reported into the process-wide cooldown and the
/// caller must abort the pass (`Err`, no inline retry); any other error is logged
/// and the item is skipped (`Ok`).
fn handle_fetch_error(e: anyhow::Error) -> Result<()> {
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
    Ok(())
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

/// Load the `added_at` currently stored for every **active** member of the likes
/// mirror, keyed by `service_tracks.service_id`.
///
/// Tombstoned (`deleted_at IS NOT NULL`) memberships are excluded — a relike
/// reactivates them through the merge, so they are treated as unknown here.
async fn load_stored_liked_added_at(
    db: &Pool<Sqlite>,
    liked_playlist_id: &str,
) -> Result<HashMap<String, Option<i64>>> {
    let rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        r#"
        SELECT st.service_id, spt.added_at
        FROM service_playlist_tracks spt
        JOIN service_playlists sp ON sp.id = spt.playlist_id
        JOIN service_tracks st ON st.id = spt.track_id
        WHERE sp.service = 'spotify' AND sp.playlist_id = ?
          AND spt.deleted_at IS NULL
        "#,
    )
    .bind(liked_playlist_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Full pass over `/me/tracks`, returning the complete set of saved track ids.
///
/// Only called when retirement is possible (Spotify's total shrank). It costs
/// O(library) requests, which is acceptable because it is rare, and it must not
/// use the truncated early-stop set — that would tombstone every older like.
async fn scan_all_saved_track_ids(spotify_client: &SpotifyClient) -> Result<HashSet<String>> {
    let stream = spotify_client.get_saved_tracks().await?;
    tokio::pin!(stream);

    let mut ids: HashSet<String> = HashSet::new();
    let mut scanned: u64 = 0;
    while let Some(item_result) = stream.next().await {
        scanned += 1;
        let saved = match item_result {
            Ok(saved) => saved,
            Err(e) => {
                handle_fetch_error(e)?;
                continue;
            }
        };
        let track = TrackInfo::from(&saved.track);
        if !track.id.is_empty() {
            ids.insert(track.id);
        }
    }

    metrics::add(Source::LikedSync, scanned.div_ceil(SAVED_TRACKS_PAGE_SIZE));
    Ok(ids)
}

/// Full cycle: fetch the user's liked songs from Spotify and reconcile the
/// local mirror.
pub async fn sync_liked_songs(
    db: &Pool<Sqlite>,
    spotify_client: &SpotifyClient,
) -> Result<LikedSyncStats> {
    // 1. Cheap "how many likes does Spotify think there are" probe.
    let total = spotify_client.get_saved_tracks_total().await?;

    // 2. What we already know locally, so an unchanged item can be skipped.
    let stored = load_stored_liked_added_at(db, LIKED_PLAYLIST_ID).await?;

    // 3. Stream newest-first and collect only new/changed memberships. `/me/tracks`
    //    is ordered by `added_at` descending and a relike moves an item to the
    //    front with a newer date, so once a long enough run of items is unchanged
    //    everything older is unchanged too and the scan can stop.
    let items = {
        let stream = spotify_client.get_saved_tracks().await?;
        tokio::pin!(stream);

        let mut items: Vec<LikedItem> = Vec::new();
        let mut current_track_ids: HashSet<String> = HashSet::new();
        let mut run = UnchangedRun::default();
        let mut items_scanned: u64 = 0;

        while let Some(item_result) = stream.next().await {
            items_scanned += 1;
            let saved = match item_result {
                Ok(saved) => saved,
                Err(e) => {
                    handle_fetch_error(e)?;
                    continue;
                }
            };

            let track = TrackInfo::from(&saved.track);
            if track.id.is_empty() {
                continue;
            }
            let added_at = saved.added_at.timestamp();
            current_track_ids.insert(track.id.clone());

            let unchanged = is_unchanged(&stored, &track.id, added_at);
            run.observe(unchanged);
            if !unchanged {
                items.push(LikedItem { track, added_at });
            }

            if run.reached(EARLY_STOP_UNCHANGED) {
                debug!(
                    "liked_sync: early stop after {} item(s) scanned \
                     ({} unchanged in a row)",
                    items_scanned, EARLY_STOP_UNCHANGED
                );
                break;
            }
        }

        // Account for the `total` probe (+1) plus the pages we actually paged.
        metrics::add(
            Source::LikedSync,
            items_scanned.div_ceil(SAVED_TRACKS_PAGE_SIZE) + 1,
        );
        debug!(
            "liked_sync: scanned {} item(s), {} unique id(s), {} changed",
            items_scanned,
            current_track_ids.len(),
            items.len()
        );
        items
    };

    // 4. Merge only what changed — steady state writes nothing.
    let linked = merge_liked_items(db, &items, LIKED_PLAYLIST_ID).await?;

    // 5. A missing like is only plausible when Spotify's total shrank; only then
    //    pay for a full scan to build a complete id set for retirement.
    let saved_count = active_liked_count(db, LIKED_PLAYLIST_ID).await?;
    let mut retired = 0usize;
    if total < saved_count {
        let complete = scan_all_saved_track_ids(spotify_client).await?;
        retired = retire_missing_likes(db, LIKED_PLAYLIST_ID, &complete).await?;
    }

    // 6. Best-effort tag refresh so the `liked` tag resolves immediately.
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    /// Fresh in-memory DB with all migrations applied.
    async fn test_pool() -> Pool<Sqlite> {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        crate::db::ensure_backpack_column(&pool).await.unwrap();
        pool
    }

    fn item(id: &str, added_at: i64) -> LikedItem {
        LikedItem {
            track: TrackInfo {
                id: id.to_string(),
                name: format!("Track {id}"),
                artists: "Some Artist".to_string(),
                album: Some("Some Album".to_string()),
                isrc: None,
                duration_ms: 180_000,
                track_number: Some(1),
                disc_number: Some(1),
                explicit: false,
                popularity: Some(50),
            },
            added_at,
        }
    }

    #[test]
    fn is_unchanged_only_when_stored_added_at_matches() {
        let mut stored: HashMap<String, Option<i64>> = HashMap::new();
        stored.insert("known".to_string(), Some(1_000));
        stored.insert("null_date".to_string(), None);

        assert!(is_unchanged(&stored, "known", 1_000));
        assert!(!is_unchanged(&stored, "known", 999)); // relike → newer date
        assert!(!is_unchanged(&stored, "null_date", 1_000)); // stored NULL ≠ present
        assert!(!is_unchanged(&stored, "unknown", 1_000)); // brand-new like
    }

    #[test]
    fn unchanged_run_extends_resets_and_reaches_threshold() {
        let mut run = UnchangedRun::default();
        run.observe(true);
        run.observe(true);
        assert!(
            !run.reached(3),
            "two unchanged items must not stop the scan"
        );

        run.observe(false); // a change resets the run
        assert!(!run.reached(1), "the run must reset on a change");

        for _ in 0..3 {
            run.observe(true);
        }
        assert!(run.reached(3));
    }

    #[tokio::test]
    async fn loader_maps_service_id_to_added_at_and_skips_tombstoned() {
        let pool = test_pool().await;
        merge_liked_items(
            &pool,
            &[item("t1", 111), item("t2", 222)],
            LIKED_PLAYLIST_ID,
        )
        .await
        .unwrap();

        // Unlike t2 → its membership is tombstoned and must drop out of the map.
        let keep: HashSet<String> = ["t1".to_string()].into_iter().collect();
        retire_missing_likes(&pool, LIKED_PLAYLIST_ID, &keep)
            .await
            .unwrap();

        let stored = load_stored_liked_added_at(&pool, LIKED_PLAYLIST_ID)
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored.get("t1"), Some(&Some(111)));
        assert_eq!(stored.get("t2"), None);
    }

    #[tokio::test]
    async fn loader_is_empty_without_the_playlist() {
        let pool = test_pool().await;
        let stored = load_stored_liked_added_at(&pool, LIKED_PLAYLIST_ID)
            .await
            .unwrap();
        assert!(stored.is_empty());
    }
}
