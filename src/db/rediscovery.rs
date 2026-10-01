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
use sqlx::{FromRow, Pool, Sqlite};

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
/// Uses only `v_track_forgotten_facts` — the curated+liked guard lives in the
/// view. Only liked tracks are counted (`liked = 1`).
pub async fn count_touched_before(pool: &Pool<Sqlite>, days: i64) -> Result<i64> {
    let count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*)
           FROM v_track_forgotten_facts
           WHERE liked = 1
             AND last_touched_at IS NOT NULL
             AND last_touched_at < (strftime('%s','now') - ? * 86400)"#,
    )
    .bind(days)
    .fetch_one(pool)
    .await?;

    Ok(count)
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
    async fn count_touched_before_counts_only_old_likes() {
        let pool = seeded_pool().await;

        // Both liked tracks were last touched long ago (1.4e9 / 1.6e9);
        // Track 3 is not liked (liked = 0) and must not count.
        let count = count_touched_before(&pool, 365).await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn count_touched_before_zero_days_excludes_recent() {
        let pool = seeded_pool().await;

        // Nothing is touched within the last 0 days (all seeds are 2020-2023).
        let count = count_touched_before(&pool, 0).await.unwrap();
        assert_eq!(count, 2);
    }
}
