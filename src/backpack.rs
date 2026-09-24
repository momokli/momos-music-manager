//! The single "Backpack" concept.
//!
//! Backpack (the *set*) is the DISTINCT union of:
//!   (a) every track of the playlists marked in MMM (`playlist_subscriptions`,
//!       `is_active = 1`), and
//!   (b) every track whose tags have `backpack = 1` (inherited via the
//!       playlist→tag chain, see `plans/done/backpack-system.md`).
//!
//! The Backpack has two guaranteed effects for every member track:
//!   1. **Auto-download** — best locally available version, priority
//!      `stem.m4a` > `flac` > `mp3` (WAV sources excluded), see
//!      [`crate::db::get_backpack_pull_candidates`].
//!   2. **Prune-safe** — never a prune candidate, see
//!      [`crate::db::get_prune_candidates`].
//!
//! The Backpack is also the **download priority**: the music-api consumer orders
//! the whole library, but Backpack members jump the queue (see
//! [`crate::db::music_api::demand_isrcs`]). The former Spotify-playlist transport
//! was removed in ADR-067.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite};

use crate::db::settings::{KEY_BACKPACK_SYNC_ENABLED, get_setting, set_setting};

// ── Aggregation ───────────────────────────────────────────────────────────

/// Return the deduplicated, stably-ordered set of `service_tracks.id` that form
/// the Backpack (union of (a) subscribed playlists and (b) backpack tags).
pub async fn get_backpack_track_ids(pool: &Pool<Sqlite>) -> Result<Vec<i64>> {
    // (a) tracks in active (subscribed) playlists.
    let subscribed: Vec<i64> = sqlx::query_scalar(
        r#"SELECT DISTINCT spt.track_id
           FROM playlist_subscriptions ps
           JOIN service_playlists sp
             ON sp.service = ps.service AND sp.playlist_id = ps.playlist_id
           JOIN service_playlist_tracks spt
             ON spt.playlist_id = sp.id AND spt.deleted_at IS NULL
           WHERE ps.is_active = 1"#,
    )
    .fetch_all(pool)
    .await?;

    // (b) tracks whose resolved tags have backpack = 1 (inheritance).
    let tagged: Vec<i64> = sqlx::query_scalar(
        r#"SELECT DISTINCT vtt.track_id
           FROM v_track_tags vtt
           JOIN tags t ON t.id = vtt.tag_id
           WHERE t.backpack = 1"#,
    )
    .fetch_all(pool)
    .await?;

    // Deduplicate and order stably (ascending id).
    let ids: BTreeSet<i64> = subscribed.into_iter().chain(tagged).collect();
    Ok(ids.into_iter().collect())
}

/// Return the set of local `files.id` linked to Backpack tracks (via
/// `v_file_track_link`). This is the set the two effects operate on
/// (auto-download candidates + prune protection).
///
/// # Why this does not use the view
///
/// `v_file_track_link` is a `UNION` of a hand-written match table and a
/// `files`-driven `JOIN ... ON (OR)` chain. SQLite cannot push a
/// `track_id IN (...)` predicate into that shape: it materialises the whole
/// view (every file × every service track, plus a correlated subquery per
/// file) and only then applies the filter. Measured through this crate's own
/// sqlx/bundled SQLite 3.46 on a production-scale library (~14k backpack
/// tracks / 13.7k files):
///
/// | query                                    | time    |
/// |------------------------------------------|---------|
/// | `SELECT v.file_id FROM v_file_track_link v WHERE v.track_id IN (...)` | **101.5 s** |
/// | `SELECT COUNT(*) ... WHERE v.track_id IN (SELECT ... FROM v_file_track_link)` | 27.0 s |
/// | [`get_file_ids_for_track_ids`] (this file) | **0.41 s** |
///
/// `file_id`-driven access to the same view is fine (~50 ms) — only the
/// `track_id` direction degenerates. The cost grows with
/// `rows(service_tracks) × rows(files) × IN-list size`, so it gets worse as
/// the library grows.
///
/// The set is computed instead by inverting the join in
/// [`get_file_ids_for_track_ids`].
pub async fn get_backpack_file_ids(pool: &Pool<Sqlite>) -> Result<Vec<i64>> {
    let ids = get_backpack_track_ids(pool).await?;
    get_file_ids_for_track_ids(pool, &ids).await
}

/// Resolve a set of `service_tracks.id` to the `files.id` linked to them,
/// without going through `v_file_track_link`'s slow `track_id` direction.
///
/// Semantics preserved from the view (see `023_file_track_corrections.sql`):
///   * a row exists when *any* match key agrees (ISRC, or the service-specific
///     id column), and for `service = 'local'` when `service_id = files.id`;
///   * `link_type = 'exclude'` rows remove an automatic match;
///   * `link_type = 'include'` rows add a link that never existed.
///
/// Returns ascending, deduplicated file ids.
pub async fn get_file_ids_for_track_ids(pool: &Pool<Sqlite>, ids: &[i64]) -> Result<Vec<i64>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    // One bind per track id (a `VALUES` CTE) instead of repeating the whole
    // id list once per match arm — same plan, a fifth of the binds.
    let values = ids.iter().map(|_| "(?)").collect::<Vec<_>>().join(",");
    let sql = format!(
        "WITH backpack_track(track_id) AS (VALUES {values}),
         candidate(file_id, track_id) AS (
             SELECT f.id, st.id FROM backpack_track bt
               JOIN service_tracks st ON st.id = bt.track_id
               JOIN files f ON f.isrc = st.isrc
              WHERE st.isrc IS NOT NULL
           UNION SELECT f.id, st.id FROM backpack_track bt
               JOIN service_tracks st ON st.id = bt.track_id
               JOIN files f ON st.service = 'spotify' AND st.service_id = f.spotify_id
           UNION SELECT f.id, st.id FROM backpack_track bt
               JOIN service_tracks st ON st.id = bt.track_id
               JOIN files f ON st.service = 'soundcloud' AND st.service_id = f.soundcloud_id
           UNION SELECT f.id, st.id FROM backpack_track bt
               JOIN service_tracks st ON st.id = bt.track_id
               JOIN files f ON st.service = 'youtube' AND st.service_id = f.youtube_id
           UNION SELECT f.id, st.id FROM backpack_track bt
               JOIN service_tracks st ON st.id = bt.track_id
               -- `service_id` is the file id as text. The extra
               -- `CAST(f.id AS TEXT)` term keeps the comparison exactly as
               -- literal as the view's, so ids with unlikely spellings
               -- ('007', ' 7', '+7') behave identically; the integer
               -- comparison is what the index can serve.
               JOIN files f ON CAST(st.service_id AS INTEGER) = f.id
                           AND CAST(f.id AS TEXT) = st.service_id
              WHERE st.service = 'local'
         )
         SELECT DISTINCT c.file_id FROM candidate c
          WHERE NOT EXISTS (
              SELECT 1 FROM file_track_corrections ftc
               WHERE ftc.file_id = c.file_id
                 AND ftc.track_id = c.track_id
                 AND ftc.link_type = 'exclude'
          )
         UNION
         SELECT file_id FROM file_track_corrections
          WHERE link_type = 'include'
            AND track_id IN (SELECT track_id FROM backpack_track)
         ORDER BY 1"
    );
    let mut query = sqlx::query_scalar::<_, i64>(&sql);
    for id in ids {
        query = query.bind(id);
    }
    Ok(query.fetch_all(pool).await?)
}

/// Resolve the `files.id` of every file that shares a track with the given
/// backpack files, plus backpack files that are linked to no track at all.
///
/// This is the "variants of the same song" expansion the backpack effects
/// need (pick one best format per track). It replaces the view's slow
/// `track_id IN (SELECT ...)` form with the `file_id` direction (fast) plus
/// [`get_file_ids_for_track_ids`].
pub async fn get_backpack_family_file_ids(
    pool: &Pool<Sqlite>,
    backpack_file_ids: &[i64],
) -> Result<Vec<i64>> {
    if backpack_file_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = backpack_file_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");

    // Tracks any backpack file belongs to (file_id direction: fast).
    let tracks_sql = format!(
        "SELECT DISTINCT track_id FROM v_file_track_link WHERE file_id IN ({placeholders})"
    );
    let mut q = sqlx::query_scalar::<_, i64>(&tracks_sql);
    for id in backpack_file_ids {
        q = q.bind(id);
    }
    let tracks: Vec<i64> = q.fetch_all(pool).await?;

    // Every file on those tracks (key-driven: fast). This already covers the
    // backpack files themselves, since they are on those tracks.
    let mut file_ids = get_file_ids_for_track_ids(pool, &tracks).await?;

    // Backpack files linked to *no* track are their own group.
    let unlinked_sql = format!(
        "SELECT f.id FROM files f
          WHERE f.id IN ({placeholders})
            AND NOT EXISTS (SELECT 1 FROM v_file_track_link v WHERE v.file_id = f.id)"
    );
    let mut q = sqlx::query_scalar::<_, i64>(&unlinked_sql);
    for id in backpack_file_ids {
        q = q.bind(id);
    }
    file_ids.extend(q.fetch_all(pool).await?);

    file_ids.sort_unstable();
    file_ids.dedup();
    Ok(file_ids)
}

// ── Transport traits (mockable seam) ──────────────────────────────────────

// ── Materialisation ───────────────────────────────────────────────────────

// ── Dirty marker + coordinator ────────────────────────────────────────────

/// Whether the Backpack *file* sync (pull missing files from the NAS backup +
/// format cleanup) may run. Unset = enabled, so existing installs are
/// unaffected until the value is written explicitly.
pub async fn backpack_sync_enabled(pool: &Pool<Sqlite>) -> bool {
    match get_setting(pool, KEY_BACKPACK_SYNC_ENABLED).await {
        Ok(Some(v)) => !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"),
        _ => true,
    }
}

/// Persist the Backpack file-sync switch.
pub async fn set_backpack_sync_enabled(pool: &Pool<Sqlite>, enabled: bool) -> Result<()> {
    set_setting(pool, KEY_BACKPACK_SYNC_ENABLED, if enabled { "1" } else { "0" })
        .await
        .context("Failed to persist backpack.sync_enabled")
}

// ── Transport impls for the concrete clients ──────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    /// Minimal schema covering the tables/queries the Backpack aggregation reads.
    async fn test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

        sqlx::query(
            "CREATE TABLE tags (
                id INTEGER PRIMARY KEY,
                name TEXT UNIQUE NOT NULL,
                category_id INTEGER NOT NULL,
                backpack INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE tag_categories (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                prefix TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE service_tracks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service TEXT NOT NULL,
                service_id TEXT NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                isrc TEXT,
                UNIQUE(service, service_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE service_playlists (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service TEXT NOT NULL,
                playlist_id TEXT NOT NULL,
                name TEXT NOT NULL,
                UNIQUE(service, playlist_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE service_playlist_tracks (
                playlist_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                position INTEGER,
                added_at INTEGER DEFAULT 0,
                deleted_at INTEGER,
                PRIMARY KEY (playlist_id, track_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE playlist_subscriptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                service TEXT NOT NULL,
                playlist_id TEXT NOT NULL,
                service_playlist_id INTEGER,
                is_active INTEGER NOT NULL DEFAULT 1,
                UNIQUE(service, playlist_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE VIEW v_track_tags AS
             SELECT DISTINCT spt.track_id,
                    t.id AS tag_id, t.name AS tag_name,
                    tc.id AS category_id, tc.name AS category_name,
                    tc.prefix, tc.is_default
             FROM service_playlist_tracks spt
             JOIN service_playlists sp ON sp.id = spt.playlist_id
             JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
             JOIN tag_categories tc ON tc.id = t.category_id",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL,
                file_type TEXT NOT NULL,
                file_size INTEGER NOT NULL DEFAULT 0,
                isrc TEXT,
                spotify_id TEXT,
                soundcloud_id TEXT,
                youtube_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Table backing the view's manual includes / excludes; usually created
        // by migration 023, extended here so callers can seed corrections.
        sqlx::query(
            "CREATE TABLE file_track_corrections (
                file_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                link_type TEXT NOT NULL,
                reason TEXT,
                created_at INTEGER DEFAULT (unixepoch()),
                UNIQUE(file_id, track_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE VIEW v_file_track_link AS
             SELECT file_id, track_id FROM file_track_corrections WHERE link_type = 'include'
             UNION
             SELECT f.id AS file_id, st.id AS track_id
             FROM files f
             JOIN service_tracks st ON (
                 st.isrc = f.isrc
                 OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
                 OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
                 OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
                 OR (st.service = 'local' AND st.service_id = CAST(f.id AS TEXT))
             )
             WHERE NOT EXISTS (
                 SELECT 1 FROM file_track_corrections ftc
                 WHERE ftc.file_id = f.id
                   AND ftc.track_id = st.id
                   AND ftc.link_type = 'exclude'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    async fn seed(pool: &SqlitePool) {
        // Tags: 'Groovy' (backpack=0), 'Deep' (backpack=1).
        sqlx::query(
            "INSERT INTO tags (id, name, category_id, backpack) VALUES
                (1, 'Groovy', 1, 0),
                (2, 'Deep', 1, 1)",
        )
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO tag_categories (id, name, prefix, is_default) VALUES (1, 'Mood', 'M', 0)",
        )
        .execute(pool)
        .await
        .unwrap();

        // Tracks 1..5.
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist) VALUES
                (1, 'spotify', 'aaa', 'T1', 'A'),
                (2, 'spotify', 'bbb', 'T2', 'A'),
                (3, 'spotify', 'ccc', 'T3', 'B'),
                (4, 'spotify', 'ddd', 'T4', 'B'),
                (5, 'spotify', 'eee', 'T5', 'C')",
        )
        .execute(pool)
        .await
        .unwrap();

        // Playlists: 'Groovy' (id 1) and 'Deep Mix' (id 2, no tag match).
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name) VALUES
                (1, 'spotify', 'pl-groovy', 'Groovy'),
                (2, 'spotify', 'pl-other', 'Deep Mix')",
        )
        .execute(pool)
        .await
        .unwrap();

        // Track 1 in 'Groovy', track 2 in 'Deep Mix'.
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position) VALUES
                (1, 1, 0),
                (2, 2, 0)",
        )
        .execute(pool)
        .await
        .unwrap();

        // Subscription on 'Deep Mix' (track 2) — source (a).
        sqlx::query(
            "INSERT INTO playlist_subscriptions (service, playlist_id, is_active) VALUES
                ('spotify', 'pl-other', 1)",
        )
        .execute(pool)
        .await
        .unwrap();

        // Files: file 1 → track 1 (via spotify_id), file 2 → track 3 (no backpack
        // membership), file 3 → track 2 (subscribed).
        sqlx::query(
            "INSERT INTO files (id, file_path, file_type, file_size, spotify_id) VALUES
                (1, '/t1.flac', 'flac', 1, 'aaa'),
                (2, '/t3.flac', 'flac', 1, 'ccc'),
                (3, '/t2.flac', 'flac', 1, 'bbb')",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn backpack_track_ids_union_and_dedupe() {
        let pool = test_db().await;
        seed(&pool).await;

        // Source (a): subscribed 'Deep Mix' → track 2.
        // Source (b): tag 'Deep' backpack=1 → no playlist matches 'Deep' (playlist
        // is 'Deep Mix'), so only track 2 from (a). Now add a matching playlist/tag.
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name) VALUES
                (3, 'spotify', 'pl-deep', 'Deep')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position) VALUES (3, 4, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let ids = get_backpack_track_ids(&pool).await.unwrap();
        // track 2 (subscribed) ∪ track 4 (backpack tag) → sorted [2, 4].
        assert_eq!(ids, vec![2, 4]);
    }

    #[tokio::test]
    async fn backpack_track_ids_dedupes_overlapping_sources() {
        let pool = test_db().await;
        seed(&pool).await;

        // Make track 2 belong to BOTH sources: subscribe the playlist AND give it
        // a backpack tag via a playlist named 'Deep' that also contains track 2.
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name) VALUES
                (3, 'spotify', 'pl-deep', 'Deep')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position) VALUES (3, 2, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let ids = get_backpack_track_ids(&pool).await.unwrap();
        assert_eq!(
            ids,
            vec![2],
            "overlapping sources must dedupe to a single id"
        );
    }

    #[tokio::test]
    async fn backpack_file_ids_maps_tracks_to_files() {
        let pool = test_db().await;
        seed(&pool).await;

        let files = get_backpack_file_ids(&pool).await.unwrap();
        // Only file 3 is linked to a backpack track (track 2).
        assert_eq!(files, vec![3]);
    }

    /// The backpack file set must be exactly what `v_file_track_link` yields.
    ///
    /// `get_backpack_file_ids` deliberately avoids the view because SQLite
    /// cannot push a `track_id IN (...)` predicate into its `UNION … ON (OR)`
    /// shape and instead materialises the whole view (~65 s in production).
    /// This test pins the two together so the hand-written rewrite can never
    /// silently drift from the view's semantics.
    #[tokio::test]
    async fn backpack_file_ids_matches_the_view() {
        let pool = test_db().await;
        seed(&pool).await;

        // Extra playlists/tracks so the backpack set is larger than one id.
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name) VALUES
                (3, 'spotify', 'pl-deep', 'Deep')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position) VALUES
                (3, 1, 0), (3, 2, 0), (3, 3, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Track 1 also matches via ISRC (not just spotify_id).
        sqlx::query("UPDATE service_tracks SET isrc = 'ISRC-1' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE files SET isrc = 'ISRC-1' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        // A file with no matching service track at all.
        sqlx::query(
            "INSERT INTO files (id, file_path, file_type, file_size, spotify_id) VALUES
                (9, '/orphan.flac', 'flac', 1, 'zzz')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Corrections: exclude one automatic match, include one hand-made link
        // to a track that has no matching file.
        sqlx::query(
            "INSERT INTO file_track_corrections (file_id, track_id, link_type) VALUES
                (1, 1, 'exclude'),
                (1, 4, 'exclude'),
                (9, 5, 'include')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Track 4 is a backpack member with a matching file (id 2 → only ties to
        // track 3, so give track 4 a real file to exclude).
        sqlx::query("UPDATE files SET spotify_id = 'ddd' WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();

        let ids = get_backpack_track_ids(&pool).await.unwrap();
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let expected_sql = format!(
            "SELECT DISTINCT v.file_id FROM v_file_track_link v WHERE v.track_id IN ({placeholders}) ORDER BY v.file_id"
        );
        let mut q = sqlx::query_scalar::<_, i64>(&expected_sql);
        for id in &ids {
            q = q.bind(id);
        }
        let expected = q.fetch_all(&pool).await.unwrap();

        let actual = get_backpack_file_ids(&pool).await.unwrap();
        assert_eq!(
            actual, expected,
            "rewrite must return exactly the view's file set (backpack tracks: {ids:?})"
        );
        // Sanity: the fixture exercises exclusions and inclusions, i.e. the set
        // is not simply "every file".
        assert!(
            !actual.is_empty() && actual.len() < 6,
            "fixture should yield a partial set, got {actual:?}"
        );
    }

    /// The `service = 'local'` match arm must be index-driven and stay
    /// byte-identical to the view's `service_id = CAST(files.id AS TEXT)`.
    #[tokio::test]
    async fn backpack_file_ids_matches_local_service_tracks() {
        let pool = test_db().await;
        seed(&pool).await;

        // Local service tracks point at a file by id-as-text.
        sqlx::query(
            "INSERT INTO service_tracks (id, service, service_id, title, artist) VALUES
                (101, 'local', '1', 'L1', 'A'),
                (102, 'local', '2', 'L2', 'A'),
                (103, 'local', '999', 'L-missing', 'A')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlists (id, service, playlist_id, name) VALUES
                (3, 'spotify', 'pl-deep', 'Deep')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO service_playlist_tracks (playlist_id, track_id, position) VALUES
                (3, 101, 0), (3, 102, 0), (3, 103, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let ids = get_backpack_track_ids(&pool).await.unwrap();
        // track 2 comes from the seeded 'Deep Mix' subscription.
        assert_eq!(ids, vec![2, 101, 102, 103]);

        let actual = get_backpack_file_ids(&pool).await.unwrap();
        // file 3 ties to the subscribed track 2; files 1 and 2 come from the
        // local tracks. The dangling local id 999 contributes nothing.
        assert_eq!(
            actual,
            vec![1, 2, 3],
            "local tracks resolve to their file by id; a dangling id contributes nothing"
        );

        // Cross-check against the view for the same predicate.
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let view_sql = format!(
            "SELECT DISTINCT v.file_id FROM v_file_track_link v WHERE v.track_id IN ({placeholders}) ORDER BY v.file_id"
        );
        let mut q = sqlx::query_scalar::<_, i64>(&view_sql);
        for id in &ids {
            q = q.bind(id);
        }
        assert_eq!(actual, q.fetch_all(&pool).await.unwrap());
    }

    /// Empty backpack → no query at all, no error.
    #[tokio::test]
    async fn backpack_file_ids_empty_when_no_backpack_tracks() {
        let pool = test_db().await;

        let files = get_backpack_file_ids(&pool).await.unwrap();
        assert!(files.is_empty());
    }

    /// The track-family expansion must equal the view-based SQL it replaced
    /// (`SELECT ... WHERE track_id IN (SELECT ... FROM v_file_track_link)` plus
    /// the unlinked-files branch).
    #[tokio::test]
    async fn backpack_family_file_ids_matches_the_view_query() {
        let pool = test_db().await;
        seed(&pool).await;

        // Give a second file the same ISRC as an existing one, so one track has
        // two variants, and leave one file linked to no track at all.
        sqlx::query(
            "INSERT INTO files (id, file_path, file_type, file_size, isrc, spotify_id) VALUES
                (4, '/t1-alt.mp3', 'mp3', 1, NULL, 'aaa'),
                (5, '/orphan.flac', 'flac', 1, NULL, 'zzz')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Backpack = track 2 (subscribed) and its file (id 3) plus the orphan 5.
        let backpack_file_ids = vec![3i64, 5i64];

        let placeholders = backpack_file_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let legacy_sql = format!(
            "SELECT DISTINCT f.id FROM files f
              JOIN v_file_track_link v ON v.file_id = f.id
              WHERE v.track_id IN (
                  SELECT DISTINCT v2.track_id FROM v_file_track_link v2
                  WHERE v2.file_id IN ({placeholders})
              )
              UNION
              SELECT DISTINCT f.id FROM files f
              WHERE f.id IN ({placeholders})
                AND f.id NOT IN (SELECT file_id FROM v_file_track_link)
              ORDER BY 1"
        );
        let mut q = sqlx::query_scalar::<_, i64>(&legacy_sql);
        for id in &backpack_file_ids {
            q = q.bind(id);
        }
        for id in &backpack_file_ids {
            q = q.bind(id);
        }
        let expected = q.fetch_all(&pool).await.unwrap();

        let actual = get_backpack_family_file_ids(&pool, &backpack_file_ids)
            .await
            .unwrap();
        assert_eq!(
            actual, expected,
            "family expansion must match the view query"
        );
    }

    // ── Materialisation (mocked transport) ────────────────────────────────

    // ── Mirror semantics, verification, guards ──────────────────────────

}
