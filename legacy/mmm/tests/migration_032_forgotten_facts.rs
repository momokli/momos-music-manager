//! Migration 032 — `playlist_kind` backfill + `v_track_forgotten_facts` semantics.
//!
//! Issue #56 DoD:
//! - `playlist_count` ignores both `liked` and `generated`.
//! - `last_touched_at` spans curated AND liked.
//! - ADR-072 guard: an archiving playlist's tombstone still counts; a
//!   non-archiving playlist's tombstone does not.
//! - Backfill: `liked`/`Likes` → `liked`, `Daily-%` → `generated`, rest `curated`.

mod common;

use sqlx::{Pool, Row, Sqlite};

/// Build an in-memory DB and run every migration *except* 032, so a test can
/// seed playlists first and then observe the real backfill.
async fn db_before_032() -> Pool<Sqlite> {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&pool)
        .await
        .unwrap();

    let mut dir = tokio::fs::read_dir("migrations").await.unwrap();
    let mut files = Vec::new();
    while let Some(entry) = dir.next_entry().await.unwrap() {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "sql")
            && path.file_name().unwrap() != "032_playlist_kind.sql"
        {
            files.push(path);
        }
    }
    files.sort();
    for path in &files {
        let sql = tokio::fs::read_to_string(path).await.unwrap();
        let sql = sql.trim();
        if !sql.is_empty() {
            sqlx::query(sql).execute(&pool).await.unwrap();
        }
    }
    pool
}

/// Apply the real migration 032 file on top of the seeded pre-032 DB.
async fn apply_032(pool: &Pool<Sqlite>) {
    let sql = tokio::fs::read_to_string("migrations/032_playlist_kind.sql")
        .await
        .unwrap();
    sqlx::query(sql.trim()).execute(pool).await.unwrap();
}

/// Insert a service playlist and return its id.
async fn insert_playlist(
    pool: &Pool<Sqlite>,
    service: &str,
    playlist_id: &str,
    name: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO service_playlists (service, playlist_id, name) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(service)
    .bind(playlist_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn set_kind(pool: &Pool<Sqlite>, playlist_id: i64, kind: &str) {
    sqlx::query("UPDATE service_playlists SET playlist_kind = ? WHERE id = ?")
        .bind(kind)
        .bind(playlist_id)
        .execute(pool)
        .await
        .unwrap();
}

/// Insert a service track and return its id.
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

/// Add a track to a playlist (`deleted_at = None` → live row).
async fn add_track(
    pool: &Pool<Sqlite>,
    playlist_id: i64,
    track_id: i64,
    added_at: i64,
    deleted_at: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO service_playlist_tracks (playlist_id, track_id, added_at, deleted_at) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(playlist_id)
    .bind(track_id)
    .bind(added_at)
    .bind(deleted_at)
    .execute(pool)
    .await
    .unwrap();
}

async fn kind_of(pool: &Pool<Sqlite>, name: &str) -> String {
    sqlx::query_scalar("SELECT playlist_kind FROM service_playlists WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Backfill classifies by name/service: liked-like names, local Dailies, rest curated.
#[tokio::test]
async fn migration_032_backfill_classifies_playlists() {
    let pool = db_before_032().await;

    insert_playlist(&pool, "spotify", "p1", "Liked").await;
    insert_playlist(&pool, "spotify", "p2", "  likes ").await;
    insert_playlist(&pool, "local", "p3", "Daily-2026-01-01").await;
    insert_playlist(&pool, "spotify", "p4", "Daily-Focus").await; // not local
    insert_playlist(&pool, "spotify", "p5", "Chill Vibes").await;

    apply_032(&pool).await;

    assert_eq!(kind_of(&pool, "Liked").await, "liked");
    assert_eq!(kind_of(&pool, "  likes ").await, "liked");
    assert_eq!(kind_of(&pool, "Daily-2026-01-01").await, "generated");
    // Non-local `Daily-%` is NOT generated (backfill is service-scoped).
    assert_eq!(kind_of(&pool, "Daily-Focus").await, "curated");
    assert_eq!(kind_of(&pool, "Chill Vibes").await, "curated");
}

/// `playlist_count` counts curated rows only — liked and generated are excluded,
/// while `liked`/`liked_at` come from the liked mirror.
#[tokio::test]
async fn migration_032_playlist_count_ignores_liked_and_generated() {
    let pool = common::create_test_db().await;
    let track = insert_track(&pool, "t1").await;

    let curated_a = insert_playlist(&pool, "spotify", "c1", "Curated A").await;
    let curated_b = insert_playlist(&pool, "spotify", "c2", "Curated B").await;
    let liked = insert_playlist(&pool, "spotify", "l1", "Liked Mirror").await;
    let generated = insert_playlist(&pool, "local", "g1", "Daily Pack").await;
    set_kind(&pool, liked, "liked").await;
    set_kind(&pool, generated, "generated").await;

    add_track(&pool, curated_a, track, 100, None).await;
    add_track(&pool, curated_b, track, 150, None).await;
    add_track(&pool, liked, track, 200, None).await;
    add_track(&pool, generated, track, 300, None).await;

    let row = sqlx::query(
        "SELECT playlist_count, last_touched_at, liked_at, liked \
         FROM v_track_forgotten_facts WHERE track_id = ?",
    )
    .bind(track)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.get::<i64, _>("playlist_count"), 2, "only curated counted");
    // last_touched_at spans curated+liked → max curated/liked added_at, not the 300 generated.
    assert_eq!(row.get::<i64, _>("last_touched_at"), 200);
    assert_eq!(row.get::<i64, _>("liked_at"), 200);
    assert!(row.get::<bool, _>("liked"));
}

/// A track that is only liked (no curated membership) still surfaces with a
/// `liked_at` and `playlist_count = 0`.
#[tokio::test]
async fn migration_032_liked_only_track_has_zero_count() {
    let pool = common::create_test_db().await;
    let track = insert_track(&pool, "t2").await;
    let liked = insert_playlist(&pool, "spotify", "l2", "Likes Mirror").await;
    set_kind(&pool, liked, "liked").await;
    add_track(&pool, liked, track, 42, None).await;

    let row = sqlx::query(
        "SELECT playlist_count, last_touched_at, liked_at, liked \
         FROM v_track_forgotten_facts WHERE track_id = ?",
    )
    .bind(track)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.get::<i64, _>("playlist_count"), 0);
    assert_eq!(row.get::<i64, _>("last_touched_at"), 42);
    assert_eq!(row.get::<i64, _>("liked_at"), 42);
    assert!(row.get::<bool, _>("liked"));
}

/// ADR-072 guard: tombstones on an *archiving* playlist still count; tombstones
/// on a non-archiving playlist are ignored.
#[tokio::test]
async fn migration_032_adr072_guard_tombstone_semantics() {
    let pool = common::create_test_db().await;

    let archived_track = insert_track(&pool, "t_arch").await;
    let plain_track = insert_track(&pool, "t_plain").await;

    // Archiving playlist: tombstone row must still contribute.
    let archiving = insert_playlist(&pool, "spotify", "a1", "Archived").await;
    sqlx::query("UPDATE service_playlists SET archive_deleted = 1 WHERE id = ?")
        .bind(archiving)
        .execute(&pool)
        .await
        .unwrap();
    add_track(&pool, archiving, archived_track, 10, Some(11)).await;

    // Non-archiving playlist: tombstone row must be ignored.
    let plain = insert_playlist(&pool, "spotify", "b1", "Plain").await;
    add_track(&pool, plain, plain_track, 20, Some(21)).await;

    let arch_count: i64 = sqlx::query_scalar(
        "SELECT playlist_count FROM v_track_forgotten_facts WHERE track_id = ?",
    )
    .bind(archived_track)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(arch_count, 1, "archiving playlist tombstone must count");

    let plain_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM v_track_forgotten_facts WHERE track_id = ?",
    )
    .bind(plain_track)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(plain_count, 0, "non-archiving tombstone must be invisible");
}
