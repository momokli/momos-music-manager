//! Migration 033 — `rediscovery_pushes` ledger (Issue #79, Milestone 1.15.0).
//!
//! DoD coverage:
//! - fresh DB 001→033 runs cleanly (`common::create_test_db`);
//! - deleting a playlist *pack* keeps the ledger row but nulls `playlist_id`
//!   (`ON DELETE SET NULL`);
//! - deleting a `service_track` cascades the ledger rows away (`ON DELETE CASCADE`);
//! - `EXPLAIN QUERY PLAN` for a `pushed_at > ?` lookup uses one of the indexes.
//!
//! SQLite only runs FK actions with `PRAGMA foreign_keys=ON`, so the mutation
//! tests acquire a dedicated connection and enable it explicitly.

mod common;

use sqlx::{Pool, Row, Sqlite};

async fn insert_track(pool: &Pool<Sqlite>, service_id: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO service_tracks (service, service_id, title, artist) \
         VALUES ('spotify', ?, 'Title', 'Artist') RETURNING id",
    )
    .bind(service_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_playlist(pool: &Pool<Sqlite>, playlist_id: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO service_playlists (service, playlist_id, name) VALUES ('spotify', ?, ?) \
         RETURNING id",
    )
    .bind(playlist_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_push(
    pool: &Pool<Sqlite>,
    track_id: i64,
    playlist_id: Option<i64>,
    pushed_at: i64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO rediscovery_pushes (track_id, playlist_id, pushed_at) \
         VALUES (?, ?, ?) RETURNING id",
    )
    .bind(track_id)
    .bind(playlist_id)
    .bind(pushed_at)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Fresh DB migrates 001→033 and the ledger table exists with the shape the
/// rediscovery scheduler relies on.
#[tokio::test]
async fn migration_033_creates_ledger_table() {
    let pool = common::create_test_db().await;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='rediscovery_pushes'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "rediscovery_pushes table missing after 001→033");

    let cols: Vec<String> = sqlx::query("PRAGMA table_info(rediscovery_pushes)")
        .fetch_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
    for c in ["id", "track_id", "playlist_id", "pushed_at", "facet_json", "slot"] {
        assert!(cols.contains(&c.to_string()), "column {c} missing: {cols:?}");
    }
}

/// Deleting the playlist pack must NOT erase history: `playlist_id` becomes
/// NULL and the ledger row survives.
#[tokio::test]
async fn migration_033_playlist_delete_keeps_row_and_nulls_fk() {
    let pool = common::create_test_db().await;
    let track = insert_track(&pool, "t_keep").await;
    let playlist = insert_playlist(&pool, "p_keep", "Daily Pack").await;
    insert_push(&pool, track, Some(playlist), 1_700_000_000).await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys=ON").execute(&mut *conn).await.unwrap();
    sqlx::query("DELETE FROM service_playlists WHERE id = ?")
        .bind(playlist)
        .execute(&mut *conn)
        .await
        .unwrap();

    let row = sqlx::query("SELECT playlist_id, track_id FROM rediscovery_pushes WHERE track_id = ?")
        .bind(track)
        .fetch_one(&pool)
        .await
        .unwrap();
    let playlist_id: Option<i64> = row.get("playlist_id");
    assert_eq!(
        playlist_id, None,
        "playlist_id must be NULLed by ON DELETE SET NULL"
    );
    assert_eq!(row.get::<i64, _>("track_id"), track, "ledger row must survive");
}

/// Deleting a `service_track` must cascade its ledger rows away.
#[tokio::test]
async fn migration_033_track_delete_cascades_ledger() {
    let pool = common::create_test_db().await;
    let track = insert_track(&pool, "t_cascade").await;
    let playlist = insert_playlist(&pool, "p_cascade", "Pack").await;
    insert_push(&pool, track, Some(playlist), 1_700_000_000).await;
    insert_push(&pool, track, None, 1_700_000_001).await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys=ON").execute(&mut *conn).await.unwrap();
    sqlx::query("DELETE FROM service_tracks WHERE id = ?")
        .bind(track)
        .execute(&mut *conn)
        .await
        .unwrap();

    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM rediscovery_pushes WHERE track_id = ?")
            .bind(track)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0, "ON DELETE CASCADE must remove ledger rows");
}

/// `pushed_at > ?` lookups must be served by one of the ledger indexes.
#[tokio::test]
async fn migration_033_pushed_at_lookup_uses_index() {
    let pool = common::create_test_db().await;
    let track = insert_track(&pool, "t_idx").await;
    let playlist = insert_playlist(&pool, "p_idx", "Pack").await;

    // Seed enough rows that the planner has a real choice.
    for i in 0..500i64 {
        insert_push(&pool, track, Some(playlist), 1_700_000_000 + i).await;
    }

    let plan: Vec<String> =
        sqlx::query("EXPLAIN QUERY PLAN SELECT track_id FROM rediscovery_pushes WHERE pushed_at > ?")
            .bind(1_700_000_100i64)
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.get::<String, _>("detail"))
            .collect();
    let joined = plan.join(" | ");
    assert!(
        joined.contains("idx_rediscovery_pushes_track")
            || joined.contains("idx_rediscovery_pushes_slot"),
        "query plan does not reference a ledger index: {joined}"
    );
}
