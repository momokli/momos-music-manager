//! Integration tests for the backpack resolver (#330).
//!
//! Boots a fresh temp SQLite with all migrations applied (setup/seed helpers
//! copied from `backpack_sources.rs`), seeds users/tags/playlists/tracks and
//! exercises `backpacks::resolve`.

use std::str::FromStr;
use std::time::Duration;

use music_hub::backpacks::{self, ResolvedSource, ResolvedTrack, SourceRef};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

const TS: &str = "2026-01-01T00:00:00+00:00";

/// Fresh temp DB, FKs on, all migrations applied.
async fn setup() -> (tempfile::TempDir, SqlitePool) {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let opts = SqliteConnectOptions::from_str(&url)
        .expect("parse url")
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(15))
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("connect");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate");
    (dir, pool)
}

async fn insert_user(pool: &SqlitePool, slug: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_users (slug, display_name, created_at) VALUES (?1, ?1, ?2) RETURNING id",
    )
    .bind(slug)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert user")
}

async fn insert_tag(pool: &SqlitePool, owner_user_id: i64, slug: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at) VALUES (?1, ?2, ?2, ?3) RETURNING id",
    )
    .bind(owner_user_id)
    .bind(slug)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert tag")
}

async fn insert_playlist(pool: &SqlitePool, user_id: i64, ext: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id) VALUES (?1, 'spotify', ?2) RETURNING id",
    )
    .bind(user_id)
    .bind(ext)
    .fetch_one(pool)
    .await
    .expect("insert playlist")
}

async fn insert_track(pool: &SqlitePool, ext: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id) VALUES ('spotify', ?1) RETURNING id",
    )
    .bind(ext)
    .fetch_one(pool)
    .await
    .expect("insert track")
}

async fn insert_backpack(pool: &SqlitePool, user_id: i64, name: &str, active: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(active)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert backpack")
}

async fn insert_resolved_tag(pool: &SqlitePool, tag_id: i64, track_id: i64) {
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track_id)
        .bind(tag_id)
        .execute(pool)
        .await
        .expect("insert resolved tag");
}

async fn insert_playlist_track(pool: &SqlitePool, playlist_id: i64, track_id: i64, position: i64) {
    sqlx::query(
        "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position, added_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(playlist_id)
    .bind(track_id)
    .bind(position)
    .bind(TS)
    .execute(pool)
    .await
    .expect("insert playlist track");
}

async fn insert_liked(pool: &SqlitePool, user_id: i64, track_id: i64, liked_at: &str) {
    sqlx::query("INSERT INTO hub_liked_tracks (user_id, track_id, liked_at) VALUES (?1, ?2, ?3)")
        .bind(user_id)
        .bind(track_id)
        .bind(liked_at)
        .execute(pool)
        .await
        .expect("insert liked");
}

async fn insert_traktor_play(pool: &SqlitePool, user_id: i64, track_id: i64, play_count: i64) {
    sqlx::query(
        "INSERT INTO hub_traktor_tracks (user_id, track_id, play_count, imported_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(user_id)
    .bind(track_id)
    .bind(play_count)
    .bind(TS)
    .execute(pool)
    .await
    .expect("insert traktor track");
}

/// Insert a raw backpack item, bypassing app-level lens validation (test-only).
async fn insert_raw_lens_item(pool: &SqlitePool, backpack_id: i64, lens: &str, position: i64) {
    sqlx::query(
        "INSERT INTO hub_backpack_items
            (backpack_id, source_kind, source_lens, position, added_at)
         VALUES (?1, 'lens', ?2, ?3, ?4)",
    )
    .bind(backpack_id)
    .bind(lens)
    .bind(position)
    .bind(TS)
    .execute(pool)
    .await
    .expect("insert raw lens item");
}

fn ids(tracks: &[ResolvedTrack]) -> Vec<i64> {
    tracks.iter().map(|t| t.track_id).collect()
}

// ── 1..11 from the plan ──────────────────────────────────────────────────────

#[tokio::test]
async fn union_across_all_four_kinds() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;

    let t1 = insert_track(&pool, "t1").await;
    let t2 = insert_track(&pool, "t2").await;
    let t3 = insert_track(&pool, "t3").await;
    let t4 = insert_track(&pool, "t4").await;
    let t5 = insert_track(&pool, "t5").await;
    let t6 = insert_track(&pool, "t6").await;

    insert_resolved_tag(&pool, tag, t1).await;
    insert_resolved_tag(&pool, tag, t2).await;
    insert_playlist_track(&pool, pl, t3, 1).await;

    // tag(1) + playlist(2) + lens newest(3) + literal track(4).
    backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Lens("newest".into())).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Track(t6)).await.unwrap();

    let got = backpacks::resolve(&pool, bp, 100).await.unwrap();
    assert_eq!(got.len(), 6, "union of all four kinds is 6 tracks");

    let mut want = vec![t1, t2, t3, t4, t5, t6];
    want.sort();
    let mut have = ids(&got);
    have.sort();
    assert_eq!(have, want, "resolver returns exactly the union {{T1..T6}}");
}

#[tokio::test]
async fn track_in_two_sources_is_deduped_and_scored_two() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    let t1 = insert_track(&pool, "t1").await;

    insert_resolved_tag(&pool, tag, t1).await;
    insert_playlist_track(&pool, pl, t1, 1).await;

    backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap();

    let got = backpacks::resolve(&pool, bp, 100).await.unwrap();
    assert_eq!(got.len(), 1, "track deduped across sources");
    assert_eq!(got[0].track_id, t1);
    assert_eq!(got[0].score, 2, "two distinct source targets contribute");
    assert!(got[0].sources.contains(&ResolvedSource::Tag { tag_id: tag }));
    assert!(got[0].sources.contains(&ResolvedSource::Playlist { playlist_id: pl }));
}

#[tokio::test]
async fn score_ordering_desc() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    let t1 = insert_track(&pool, "t1").await;
    let t2 = insert_track(&pool, "t2").await;

    // T1 in tag + playlist (score 2); T2 in tag only (score 1).
    insert_resolved_tag(&pool, tag, t1).await;
    insert_resolved_tag(&pool, tag, t2).await;
    insert_playlist_track(&pool, pl, t1, 1).await;

    backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap();

    let got = backpacks::resolve(&pool, bp, 100).await.unwrap();
    assert_eq!(ids(&got), vec![t1, t2], "higher score first");
    let scores: Vec<i64> = got.iter().map(|t| t.score).collect();
    assert_eq!(scores, vec![2, 1]);
}

#[tokio::test]
async fn limit_truncates_after_sort() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;

    let t1 = insert_track(&pool, "t1").await;
    let t2 = insert_track(&pool, "t2").await;
    let t3 = insert_track(&pool, "t3").await;
    let t4 = insert_track(&pool, "t4").await;
    let t5 = insert_track(&pool, "t5").await;

    // Source i (position i) contains T1..Ti -> scores 5,4,3,2,1 for T1..T5.
    for (i, tracks) in [
        vec![t1],
        vec![t1, t2],
        vec![t1, t2, t3],
        vec![t1, t2, t3, t4],
        vec![t1, t2, t3, t4, t5],
    ]
    .into_iter()
    .enumerate()
    {
        let tag = insert_tag(&pool, user, &format!("tag-{i}")).await;
        for tr in tracks {
            insert_resolved_tag(&pool, tag, tr).await;
        }
        backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    }

    let got = backpacks::resolve(&pool, bp, 2).await.unwrap();
    assert_eq!(got.len(), 2, "limit truncates after sort");
    assert_eq!(ids(&got), vec![t1, t2], "highest scores survive, not insertion order");
}

#[tokio::test]
async fn limit_is_clamped() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let t1 = insert_track(&pool, "t1").await;
    let t2 = insert_track(&pool, "t2").await;
    let t3 = insert_track(&pool, "t3").await;

    backpacks::add_source(&pool, bp, SourceRef::Track(t1)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Track(t2)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Track(t3)).await.unwrap();

    let low = backpacks::resolve(&pool, bp, 0).await.unwrap();
    assert!(low.len() <= 1, "limit 0 clamps up to 1, got {}", low.len());

    let high = backpacks::resolve(&pool, bp, 9999).await.unwrap();
    assert_eq!(high.len(), 3, "high limit clamps to 500, all 3 tracks present");
}

#[tokio::test]
async fn deterministic_tie_break_by_position_then_id() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "early").await;
    let pl = insert_playlist(&pool, user, "late").await;

    let t_small = insert_track(&pool, "small").await;
    let t_large = insert_track(&pool, "large").await;
    let t_late = insert_track(&pool, "late").await;

    // Equal score (1), same source position: id ascending decides.
    insert_resolved_tag(&pool, tag, t_large).await;
    insert_resolved_tag(&pool, tag, t_small).await;
    // Earlier position than the playlist: position decides before id.
    insert_playlist_track(&pool, pl, t_late, 1).await;

    backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap(); // position 1
    backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap(); // position 2

    let first = backpacks::resolve(&pool, bp, 100).await.unwrap();
    let second = backpacks::resolve(&pool, bp, 100).await.unwrap();

    let mut expected_ids = vec![t_small, t_large];
    expected_ids.sort();
    assert_eq!(
        ids(&first),
        vec![expected_ids[0], expected_ids[1], t_late],
        "position 1 source's tracks (id-sorted) precede position 2's track"
    );
    assert_eq!(first, second, "resolve is deterministic across runs");
}

#[tokio::test]
async fn lens_liked_is_user_scoped() {
    let (_dir, pool) = setup().await;
    let u1 = insert_user(&pool, "alice").await;
    let u2 = insert_user(&pool, "bob").await;
    let bp = insert_backpack(&pool, u1, "B", 0).await;
    let t1 = insert_track(&pool, "t1").await;
    let t2 = insert_track(&pool, "t2").await;

    insert_liked(&pool, u1, t1, TS).await;
    insert_liked(&pool, u2, t2, TS).await;

    backpacks::add_source(&pool, bp, SourceRef::Lens("liked".into())).await.unwrap();

    let got = backpacks::resolve(&pool, bp, 100).await.unwrap();
    assert_eq!(ids(&got), vec![t1], "only the backpack owner's likes resolve");
    assert!(!ids(&got).contains(&t2), "other user's like must be absent");
}

#[tokio::test]
async fn lens_unheard_orders_by_play_count() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let t_low = insert_track(&pool, "low").await;
    let t_high = insert_track(&pool, "high").await;

    insert_traktor_play(&pool, user, t_low, 0).await;
    insert_traktor_play(&pool, user, t_high, 9).await;

    backpacks::add_source(&pool, bp, SourceRef::Lens("unheard".into())).await.unwrap();

    let got = backpacks::resolve(&pool, bp, 100).await.unwrap();
    assert_eq!(ids(&got), vec![t_low, t_high], "least-played first");
}

#[tokio::test]
async fn empty_and_unknown_backpack_yield_empty() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;

    let unknown = backpacks::resolve(&pool, 999, 50).await.unwrap();
    assert!(unknown.is_empty(), "unknown backpack resolves empty");

    let bp = insert_backpack(&pool, user, "empty", 0).await;
    let empty = backpacks::resolve(&pool, bp, 50).await.unwrap();
    assert!(empty.is_empty(), "backpack without items resolves empty");
}

#[tokio::test]
async fn unknown_lens_key_falls_back_to_newest() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let t1 = insert_track(&pool, "t1").await;
    let t2 = insert_track(&pool, "t2").await;

    // Bypass #329 validation: write a bogus lens key directly.
    insert_raw_lens_item(&pool, bp, "bogus", 1).await;

    let got = backpacks::resolve(&pool, bp, 100).await.unwrap();
    let mut have = ids(&got);
    have.sort();
    let mut want = vec![t1, t2];
    want.sort();
    assert_eq!(have, want, "unknown lens key falls back to newest (all hub tracks)");
    assert!(!got.is_empty(), "fallback never errors");
}

#[tokio::test]
async fn sources_listed_once_each_in_position_order() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    let t1 = insert_track(&pool, "t1").await;

    insert_resolved_tag(&pool, tag, t1).await;
    insert_playlist_track(&pool, pl, t1, 1).await;

    backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap(); // position 1
    backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap(); // position 2

    let got = backpacks::resolve(&pool, bp, 100).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].score, 2);
    assert_eq!(
        got[0].sources,
        vec![
            ResolvedSource::Tag { tag_id: tag },
            ResolvedSource::Playlist { playlist_id: pl },
        ],
        "sources appear once each, ordered by item position"
    );
}
