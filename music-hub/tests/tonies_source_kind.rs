//! Source-kind resolver (#504): a playlist, a tag and a backpack that cover the
//! same tracks must yield the same desired-track list (and therefore the same
//! chapter list after the greedy fit). Also covers the error paths: an unknown
//! kind, a missing source id, and an existing-but-empty source.
//!
//! Pure DB, no network: the resolver only reads `hub_*` tables.

use std::collections::BTreeSet;

use music_hub::tonies::{self, DesiredTrack};
use sqlx::SqlitePool;

const TS: &str = "2026-01-01T00:00:00+00:00";

async fn setup() -> (tempfile::TempDir, SqlitePool) {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let pool = music_hub::db::connect(&url).await.expect("connect");
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

async fn insert_track(
    pool: &SqlitePool,
    sid: &str,
    isrc: &str,
    title: &str,
    duration_ms: i64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, isrc, title, duration_ms, first_seen_at)
         VALUES ('spotify', ?1, ?2, ?3, ?4, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(sid)
    .bind(isrc)
    .bind(title)
    .bind(duration_ms)
    .fetch_one(pool)
    .await
    .expect("insert track")
}

async fn insert_playlist(pool: &SqlitePool, user_id: i64, ext: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, fetched_at)
         VALUES (?1, 'spotify', ?2, ?3, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(user_id)
    .bind(ext)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("insert playlist")
}

async fn insert_tag(pool: &SqlitePool, user_id: i64, slug: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, ?2, ?3, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(slug)
    .bind(name)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert tag")
}

async fn insert_backpack(pool: &SqlitePool, user_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, ?2, 0, ?3, ?3) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert backpack")
}

/// A world where one playlist, one tag and one backpack all cover `a, b, c`.
struct World {
    pool: SqlitePool,
    _dir: tempfile::TempDir,
    playlist: i64,
    tag: i64,
    backpack: i64,
}

async fn world() -> World {
    let (dir, pool) = setup().await;
    let uid = insert_user(&pool, "alice").await;

    let a = insert_track(&pool, "s-a", "isrc-a", "A", 100_000).await;
    let b = insert_track(&pool, "s-b", "isrc-b", "B", 200_000).await;
    let c = insert_track(&pool, "s-c", "isrc-c", "C", 300_000).await;

    // Playlist order (position) matches track-id order, so the equivalent
    // playlist / tag / backpack can be compared as identical ordered lists.
    let playlist = insert_playlist(&pool, uid, "pl-1", "Mix").await;
    for (pos, t) in [a, b, c].iter().enumerate() {
        sqlx::query(
            "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
        )
        .bind(playlist)
        .bind(t)
        .bind(pos as i64)
        .execute(&pool)
        .await
        .unwrap();
    }

    let tag = insert_tag(&pool, uid, "mix", "Mix").await;
    for t in [a, b, c] {
        sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
            .bind(t)
            .bind(tag)
            .execute(&pool)
            .await
            .unwrap();
    }

    let backpack = insert_backpack(&pool, uid, "Bundle").await;
    sqlx::query(
        "INSERT INTO hub_backpack_items (backpack_id, source_kind, source_playlist_id, position, added_at)
         VALUES (?1, 'playlist', ?2, 0, ?3)",
    )
    .bind(backpack)
    .bind(playlist)
    .bind(TS)
    .execute(&pool)
    .await
    .unwrap();

    World {
        pool,
        _dir: dir,
        playlist,
        tag,
        backpack,
    }
}

/// The chapter list a source produces after the greedy fit: `(title, seconds)`.
fn chapters(desired: &[DesiredTrack]) -> BTreeSet<(String, i64)> {
    let plan = tonies::greedy_fit(desired, 5400, |_| true);
    plan.tracks
        .iter()
        .map(|t| (t.title.clone(), tonies::ms_to_seconds(t.duration_ms)))
        .collect()
}

#[tokio::test]
async fn playlist_tag_and_backpack_yield_the_same_chapters() {
    let w = world().await;
    let pl = tonies::desired_for_source(&w.pool, "playlist", Some(w.playlist))
        .await
        .unwrap();
    let tag = tonies::desired_for_source(&w.pool, "tag", Some(w.tag))
        .await
        .unwrap();
    let bp = tonies::desired_for_source(&w.pool, "backpack", Some(w.backpack))
        .await
        .unwrap();

    // Acceptance #1: the same chapter list (same tracks, same order), whichever
    // source feeds the Tonie.
    assert_eq!(chapters(&pl), chapters(&tag));
    assert_eq!(chapters(&pl), chapters(&bp));
    assert_eq!(pl.len(), 3);
    assert_eq!(pl, tag);
    assert_eq!(pl, bp);
}

#[tokio::test]
async fn playlist_respects_position_order() {
    let w = world().await;
    // A second playlist over the same tracks in a different position order must
    // follow `position`, not track-id order.
    let shuffled = insert_playlist(&w.pool, 1, "pl-shuffled", "Shuffled").await;
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT track_id FROM hub_playlist_tracks WHERE playlist_id = ?1 ORDER BY position",
    )
    .bind(w.playlist)
    .fetch_all(&w.pool)
    .await
    .unwrap();
    let order = [ids[2], ids[0], ids[1]];
    for (pos, t) in order.iter().enumerate() {
        sqlx::query(
            "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
        )
        .bind(shuffled)
        .bind(t)
        .bind(pos as i64)
        .execute(&w.pool)
        .await
        .unwrap();
    }
    let d = tonies::desired_for_source(&w.pool, "playlist", Some(shuffled))
        .await
        .unwrap();
    assert_eq!(
        d.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
        vec!["C", "A", "B"]
    );
}

#[tokio::test]
async fn empty_sources_resolve_to_empty_lists() {
    let w = world().await;
    // An existing but empty source is not an error — it is an empty list.
    let empty_pl = insert_playlist(&w.pool, 1, "pl-empty", "Empty").await;
    let empty_tag = insert_tag(&w.pool, 1, "empty", "Empty Tag").await;
    let empty_bp = insert_backpack(&w.pool, 1, "Empty Bundle").await;

    for (kind, id) in [
        ("playlist", empty_pl),
        ("tag", empty_tag),
        ("backpack", empty_bp),
    ] {
        let d = tonies::desired_for_source(&w.pool, kind, Some(id))
            .await
            .unwrap();
        assert!(d.is_empty(), "{kind} should resolve to an empty list");
    }
}

#[tokio::test]
async fn unknown_kind_and_missing_source_are_errors() {
    let w = world().await;
    let err = tonies::desired_for_source(&w.pool, "lens", Some(1))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Unbekannte Quellenart"), "{err}");

    let err = tonies::desired_for_source(&w.pool, "playlist", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Keine Quelle"), "{err}");
}
