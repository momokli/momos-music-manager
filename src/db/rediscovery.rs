//! Rediscovery facts — typed access to `v_track_forgotten_facts` plus the
//! linked-file audio features needed to rank resurfacing candidates.
//!
//! The view `v_track_forgotten_facts` (migration 032) already applies the
//! ADR-072 archive guard and only counts *curated* playlists towards
//! `playlist_count` (likes are the baseline every track shares, generated
//! dailies are our own output). This module does **not** re-derive any of that.
//!
//! File-side columns come from the linked file via `v_file_track_link`
//! (deterministically: lowest `files.id` when several files link to one track).
//! A track with no linked file yields `None` for all file-side fields,
//! including `in_backpack`.

use anyhow::Result;
use sqlx::{FromRow, Pool, QueryBuilder, Sqlite};
use std::collections::{HashMap, HashSet};

/// Facts about a single track for the rediscovery ranking.
///
/// View-derived fields (`track_id`..`liked`) are always present; file-derived
/// fields (`bpm`..`in_backpack`) are `None` when no file is linked.
#[derive(Debug, Clone, FromRow)]
pub struct TrackFacts {
    pub track_id: i64,
    pub playlist_count: i64,
    pub last_touched_at: Option<i64>,
    pub liked_at: Option<i64>,
    pub liked: bool,
    // ── File-side (nullable: no linked file) ────────────────────────────
    #[sqlx(default)]
    pub bpm: Option<f64>,
    #[sqlx(default)]
    pub musical_key: Option<String>,
    #[sqlx(default)]
    pub genre: Option<String>,
    #[sqlx(default)]
    pub play_count: Option<i64>,
    #[sqlx(default)]
    pub last_played: Option<i64>,
    #[sqlx(default)]
    pub in_backpack: Option<bool>,
}

/// File-derived columns, selected explicitly (never `SELECT *`).
#[derive(Debug, Clone, FromRow)]
struct LinkedFile {
    bpm: Option<f64>,
    musical_key: Option<String>,
    genre: Option<String>,
    play_count: Option<i64>,
    last_played: Option<i64>,
}

/// Fetch the rediscovery facts for a single track.
///
/// Returns `Ok(None)` when the track has no row in `v_track_forgotten_facts`
/// (i.e. it is in no curated/liked playlist).
pub async fn get_track_facts(pool: &Pool<Sqlite>, track_id: i64) -> Result<Option<TrackFacts>> {
    let mut facts = sqlx::query_as::<_, TrackFacts>(
        r#"SELECT track_id, playlist_count, last_touched_at, liked_at, liked
           FROM v_track_forgotten_facts
           WHERE track_id = ?"#,
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;

    let Some(facts) = facts.as_mut() else {
        return Ok(None);
    };

    // Deterministic pick: lowest files.id when a track has several linked files.
    let file = sqlx::query_as::<_, LinkedFile>(
        r#"SELECT f.bpm, f.musical_key, f.genre, f.play_count, f.last_played
           FROM v_file_track_link link
           JOIN files f ON f.id = link.file_id
           WHERE link.track_id = ?
           ORDER BY f.id
           LIMIT 1"#,
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;

    if let Some(file) = file {
        facts.bpm = file.bpm;
        facts.musical_key = file.musical_key;
        facts.genre = file.genre;
        facts.play_count = file.play_count;
        facts.last_played = file.last_played;
        // in_backpack is file-side: only meaningful once a file is linked.
        facts.in_backpack = Some(
            crate::backpack::get_backpack_track_ids(pool)
                .await?
                .contains(&track_id),
        );
    }

    Ok(Some(facts.clone()))
}

/// Count tracks whose last curated/liked contact is older than `days`.
///
/// Uses only `v_track_forgotten_facts` — the curated+liked scope lives in the
/// view (it contains exactly the tracks in curated and liked playlists), so no
/// `liked = 1` filter is applied here: the count spans curated **and** liked
/// contacts, independent of whether the track is liked.
pub async fn count_touched_before(pool: &Pool<Sqlite>, days: i64) -> Result<i64> {
    let count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*)
           FROM v_track_forgotten_facts
           WHERE last_touched_at IS NOT NULL
             AND last_touched_at < (strftime('%s','now') - ? * 86400)"#,
    )
    .bind(days)
    .fetch_one(pool)
    .await?;

    Ok(count)
}

// ── Batched loaders for the rediscovery candidates endpoint ───────────────
//
// Every loader here is batched: one query for the whole candidate set, never a
// per-track loop. The `track_id` direction of `v_file_track_link` is NOT usable
// in a correlated pro-row subquery (see `src/backpack.rs`), so file-side access
// always goes through the fast `file_id` direction or through
// [`crate::backpack::get_file_ids_for_track_ids`].

/// File-side facts of a candidate track, taken from its deterministically
/// chosen linked file (lowest `files.id`, same pick as [`get_track_facts`]).
#[derive(Debug, Clone, PartialEq)]
pub struct FileFacts {
    pub file_id: i64,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub genre: Option<String>,
    pub play_count: Option<i64>,
    pub last_played: Option<i64>,
}

/// File-side `files` filter for [`tracks_matching_audio`].
#[derive(Debug, Clone, Default)]
pub struct AudioFilter {
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub require_bpm: bool,
    pub require_key: bool,
    pub keys: Vec<String>,
    pub genres: Vec<String>,
    pub play_count_max: Option<i64>,
    /// Match when `last_played IS NULL OR last_played < this` (unix seconds).
    pub not_played_before: Option<i64>,
}

#[derive(Debug, Clone, FromRow)]
struct RowFileFacts {
    track_id: i64,
    file_id: i64,
    bpm: Option<f64>,
    musical_key: Option<String>,
    genre: Option<String>,
    play_count: Option<i64>,
    last_played: Option<i64>,
}

#[derive(Debug, Clone, FromRow)]
struct TrackMetaRow {
    id: i64,
    service_id: Option<String>,
    title: String,
    artist: String,
}

fn placeholders(n: usize) -> String {
    std::iter::repeat("?").take(n).collect::<Vec<_>>().join(",")
}

/// One row per forgotten track (view facts only; file-side fields stay `None`).
///
/// This is the full candidate universe for a request — a single indexed scan of
/// `v_track_forgotten_facts`, never one query per track.
pub async fn list_forgotten_facts(pool: &Pool<Sqlite>) -> Result<Vec<TrackFacts>> {
    let rows = sqlx::query_as::<_, TrackFacts>(
        r#"SELECT track_id, playlist_count, last_touched_at, liked_at, liked
           FROM v_track_forgotten_facts"#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Load the file-side facts for many tracks in one batch.
///
/// Semantics match [`get_track_facts`]: the linked file with the lowest
/// `files.id` wins. A track with no linked file is simply absent from the map.
pub async fn load_file_facts(
    pool: &Pool<Sqlite>,
    track_ids: &[i64],
) -> Result<HashMap<i64, FileFacts>> {
    let mut map = HashMap::new();
    if track_ids.is_empty() {
        return Ok(map);
    }
    let file_ids = crate::backpack::get_file_ids_for_track_ids(pool, track_ids).await?;
    if file_ids.is_empty() {
        return Ok(map);
    }

    let wanted: HashSet<i64> = track_ids.iter().copied().collect();
    let sql = format!(
        r#"SELECT v.track_id AS track_id, f.id AS file_id, f.bpm, f.musical_key,
                  f.genre, f.play_count, f.last_played
           FROM v_file_track_link v
           JOIN files f ON f.id = v.file_id
           WHERE v.file_id IN ({})
           ORDER BY v.track_id, f.id"#,
        placeholders(file_ids.len())
    );
    let mut query = sqlx::query_as::<_, RowFileFacts>(&sql);
    for id in &file_ids {
        query = query.bind(id);
    }
    for row in query.fetch_all(pool).await? {
        if !wanted.contains(&row.track_id) {
            continue;
        }
        // `ORDER BY v.track_id, f.id` — first insert is the lowest file id.
        map.entry(row.track_id).or_insert(FileFacts {
            file_id: row.file_id,
            bpm: row.bpm,
            musical_key: row.musical_key,
            genre: row.genre,
            play_count: row.play_count,
            last_played: row.last_played,
        });
    }
    Ok(map)
}

/// Track ids that have at least one local file location, in one batch.
///
/// `owned` = a local `file_locations` row exists for any file linked to the
/// track (ADR: local copy present).
pub async fn owned_track_ids(
    pool: &Pool<Sqlite>,
    track_ids: &[i64],
) -> Result<HashSet<i64>> {
    let mut owned = HashSet::new();
    if track_ids.is_empty() {
        return Ok(owned);
    }
    let wanted: HashSet<i64> = track_ids.iter().copied().collect();
    let file_ids = crate::backpack::get_file_ids_for_track_ids(pool, track_ids).await?;
    if file_ids.is_empty() {
        return Ok(owned);
    }

    let sql = format!(
        r#"SELECT DISTINCT v.track_id
           FROM v_file_track_link v
           WHERE v.file_id IN ({})
             AND v.file_id IN (
                 SELECT file_id FROM file_locations WHERE location_type = 'local'
             )"#,
        placeholders(file_ids.len())
    );
    let mut query = sqlx::query_scalar::<_, i64>(&sql);
    for id in &file_ids {
        query = query.bind(id);
    }
    for id in query.fetch_all(pool).await? {
        if wanted.contains(&id) {
            owned.insert(id);
        }
    }
    Ok(owned)
}

/// Track ids whose linked `files` satisfy the audio facets.
///
/// Built as `SELECT DISTINCT track_id FROM v_file_track_link WHERE file_id IN
/// (SELECT id FROM files WHERE <facets>)` — the fast `file_id` direction, never
/// the slow `track_id` direction.
pub async fn tracks_matching_audio(
    pool: &Pool<Sqlite>,
    filter: &AudioFilter,
) -> Result<HashSet<i64>> {
    let mut qb = QueryBuilder::new(
        "SELECT DISTINCT v.track_id FROM v_file_track_link v \
         WHERE v.file_id IN (SELECT f.id FROM files f WHERE 1 = 1",
    );
    if let Some(min) = filter.bpm_min {
        qb.push(" AND f.bpm >= ").push_bind(min);
    }
    if let Some(max) = filter.bpm_max {
        qb.push(" AND f.bpm <= ").push_bind(max);
    }
    if filter.require_bpm {
        qb.push(" AND f.bpm IS NOT NULL");
    }
    if filter.require_key {
        qb.push(" AND f.musical_key IS NOT NULL");
    }
    if !filter.keys.is_empty() {
        qb.push(" AND f.musical_key IN (");
        let mut sep = qb.separated(", ");
        for key in &filter.keys {
            sep.push_bind(key);
        }
        sep.push_unseparated(")");
    }
    if !filter.genres.is_empty() {
        qb.push(" AND f.genre IN (");
        let mut sep = qb.separated(", ");
        for genre in &filter.genres {
            sep.push_bind(genre);
        }
        sep.push_unseparated(")");
    }
    if let Some(max) = filter.play_count_max {
        qb.push(" AND f.play_count IS NOT NULL AND f.play_count <= ")
            .push_bind(max);
    }
    if let Some(cutoff) = filter.not_played_before {
        qb.push(" AND (f.last_played IS NULL OR f.last_played < ")
            .push_bind(cutoff)
            .push(")");
    }
    qb.push(")");

    let ids: Vec<i64> = qb.build_query_scalar().fetch_all(pool).await?;
    Ok(ids.into_iter().collect())
}

/// Track ids pushed within the last `days` (relative to `now`), from the
/// migration-033 ledger. Used to exclude freshly re-pushed tracks.
pub async fn pushed_track_ids_since(
    pool: &Pool<Sqlite>,
    days: i64,
    now: i64,
) -> Result<HashSet<i64>> {
    let cutoff = now - days * 86_400;
    let ids: Vec<i64> = sqlx::query_scalar(
        r#"SELECT DISTINCT track_id FROM rediscovery_pushes WHERE pushed_at >= ?"#,
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    Ok(ids.into_iter().collect())
}

/// Latest `pushed_at` per track, for the `pushed-<date>` / `never-pushed`
/// reason on candidates that passed the `excludePushedSinceDays` facet.
pub async fn last_push_dates(
    pool: &Pool<Sqlite>,
    track_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        r#"SELECT track_id, MAX(pushed_at) FROM rediscovery_pushes
           WHERE track_id IN ({}) GROUP BY track_id"#,
        placeholders(track_ids.len())
    );
    let mut query = sqlx::query_as::<_, (i64, i64)>(&sql);
    for id in track_ids {
        query = query.bind(id);
    }
    Ok(query.fetch_all(pool).await?.into_iter().collect())
}

/// Service-track metadata (`spotifyId`, `title`, `artist`) for a page of
/// candidates, resolved in one batch query.
pub async fn track_metadata(
    pool: &Pool<Sqlite>,
    track_ids: &[i64],
) -> Result<HashMap<i64, (Option<String>, String, String)>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        r#"SELECT id, service_id, title, artist FROM service_tracks
           WHERE id IN ({})"#,
        placeholders(track_ids.len())
    );
    let mut query = sqlx::query_as::<_, TrackMetaRow>(&sql);
    for id in track_ids {
        query = query.bind(id);
    }
    Ok(query
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| (r.id, (r.service_id, r.title, r.artist)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::testing::seed_liked_songs_scenario;
    use sqlx::SqlitePool;

    /// In-memory DB with all migrations applied + backpack column fix, then the
    /// shared liked-songs seed (Issue #58).
    async fn seeded_pool() -> Pool<Sqlite> {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        crate::db::ensure_backpack_column(&pool).await.unwrap();
        seed_liked_songs_scenario(&pool).await;
        pool
    }

    #[tokio::test]
    async fn get_track_facts_seed_values() {
        let pool = seeded_pool().await;

        // Track 1: one curated playlist (old) + an old like; file 1 is the
        // lowest linked file (bpm 128.0, key 4m).
        let t1 = get_track_facts(&pool, 1).await.unwrap().unwrap();
        assert_eq!(t1.track_id, 1);
        assert_eq!(t1.playlist_count, 1);
        assert_eq!(t1.last_touched_at, Some(1600000000));
        assert!(t1.liked);
        assert_eq!(t1.liked_at, Some(1500000000));
        assert_eq!(t1.bpm, Some(128.0));
        assert_eq!(t1.musical_key.as_deref(), Some("4m"));
        assert_eq!(t1.play_count, Some(10));
        assert_eq!(t1.last_played, Some(1700000000));

        // Track 2: no curated playlist, liked only; file 3 (bpm 140.0, key 8m).
        let t2 = get_track_facts(&pool, 2).await.unwrap().unwrap();
        assert_eq!(t2.track_id, 2);
        assert_eq!(t2.playlist_count, 0);
        assert_eq!(t2.last_touched_at, Some(1400000000));
        assert!(t2.liked);
        assert_eq!(t2.liked_at, Some(1400000000));
        assert_eq!(t2.bpm, Some(140.0));
        assert_eq!(t2.musical_key.as_deref(), Some("8m"));

        // Track 3: two curated playlists, not liked, no linked file.
        let t3 = get_track_facts(&pool, 3).await.unwrap().unwrap();
        assert_eq!(t3.track_id, 3);
        assert_eq!(t3.playlist_count, 2);
        assert_eq!(t3.last_touched_at, Some(1700000000));
        assert!(!t3.liked);
        assert_eq!(t3.liked_at, None);
    }

    #[tokio::test]
    async fn track_without_file_has_no_file_facts() {
        let pool = seeded_pool().await;

        let t3 = get_track_facts(&pool, 3).await.unwrap().unwrap();
        assert_eq!(t3.bpm, None);
        assert_eq!(t3.musical_key, None);
        assert_eq!(t3.genre, None);
        assert_eq!(t3.play_count, None);
        assert_eq!(t3.last_played, None);
        // file-side: no linked file => in_backpack is None (not false).
        assert_eq!(t3.in_backpack, None);
    }

    #[tokio::test]
    async fn track_with_file_reports_backpack_membership() {
        let pool = seeded_pool().await;

        let t1 = get_track_facts(&pool, 1).await.unwrap().unwrap();
        assert_eq!(t1.in_backpack, Some(false));
    }

    #[tokio::test]
    async fn unknown_track_returns_none() {
        let pool = seeded_pool().await;
        assert!(get_track_facts(&pool, 999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn count_touched_before_counts_curated_and_liked() {
        let pool = seeded_pool().await;

        // All three tracks were last touched long ago (1.4e9 / 1.6e9 / 1.7e9)
        // and all are in at least one curated or liked playlist. The count
        // spans curated+liked contacts regardless of the `liked` flag, so the
        // not-liked Track 3 counts too.
        let count = count_touched_before(&pool, 365).await.unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn count_touched_before_zero_days_includes_all_past_touches() {
        let pool = seeded_pool().await;

        // Nothing is touched within the last 0 days (all seeds are 2020-2023),
        // so every curated+liked track counts.
        let count = count_touched_before(&pool, 0).await.unwrap();
        assert_eq!(count, 3);
    }
}
