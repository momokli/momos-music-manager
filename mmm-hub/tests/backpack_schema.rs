//! Migration test for the backpack schema (#327).
//!
//! Boots a fresh temp SQLite with only the real migrations applied and checks the
//! persistence invariants of `hub_backpacks` / `hub_backpack_items`: table/index
//! presence, the `source_kind` CHECK, the "exactly one source column per kind"
//! CHECK, "at most one active backpack per user", per-user case-insensitive name
//! uniqueness, and `ON DELETE CASCADE` behaviour.

use std::str::FromStr;
use std::time::Duration;

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

/// Insert an item of the given kind, binding the relevant source column.
async fn insert_item(pool: &SqlitePool, backpack_id: i64, kind: &str, source: i64) -> Result<(), sqlx::Error> {
    let sql = match kind {
        "tag" => "INSERT INTO hub_backpack_items (backpack_id, source_kind, source_tag_id, added_at) VALUES (?1, 'tag', ?2, ?3)",
        "playlist" => "INSERT INTO hub_backpack_items (backpack_id, source_kind, source_playlist_id, added_at) VALUES (?1, 'playlist', ?2, ?3)",
        "track" => "INSERT INTO hub_backpack_items (backpack_id, source_kind, source_track_id, added_at) VALUES (?1, 'track', ?2, ?3)",
        "lens" => "INSERT INTO hub_backpack_items (backpack_id, source_kind, source_lens, added_at) VALUES (?1, 'lens', ?2, ?3)",
        other => panic!("unknown kind {other}"),
    };
    if kind == "lens" {
        sqlx::query(sql)
            .bind(backpack_id)
            .bind("newest")
            .bind(TS)
            .execute(pool)
            .await
            .map(|_| ())
    } else {
        sqlx::query(sql)
            .bind(backpack_id)
            .bind(source)
            .bind(TS)
            .execute(pool)
            .await
            .map(|_| ())
    }
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("count")
}

#[tokio::test]
async fn tables_exist() {
    let (_dir, pool) = setup().await;
    for table in ["hub_backpacks", "hub_backpack_items"] {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1, "table {table} should exist");
    }
}

#[tokio::test]
async fn source_kind_bogus_is_rejected() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let res = sqlx::query(
        "INSERT INTO hub_backpack_items (backpack_id, source_kind, source_lens, added_at)
         VALUES (?1, 'bogus', 'x', ?2)",
    )
    .bind(bp)
    .bind(TS)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "source_kind='bogus' must be rejected");
}

#[tokio::test]
async fn source_kind_requires_matching_source_column() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    let tr = insert_track(&pool, "t-1").await;

    // tag without a tag id → error.
    let res = sqlx::query(
        "INSERT INTO hub_backpack_items (backpack_id, source_kind, added_at)
         VALUES (?1, 'tag', ?2)",
    )
    .bind(bp)
    .bind(TS)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "tag without source_tag_id must fail");

    // tag with both tag_id and playlist_id → error.
    let res = sqlx::query(
        "INSERT INTO hub_backpack_items (backpack_id, source_kind, source_tag_id, source_playlist_id, added_at)
         VALUES (?1, 'tag', ?2, ?3, ?4)",
    )
    .bind(bp)
    .bind(tag)
    .bind(pl)
    .bind(TS)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "tag with two source columns must fail");

    // Valid combination per kind → OK.
    insert_item(&pool, bp, "tag", tag).await.expect("valid tag item");
    insert_item(&pool, bp, "playlist", pl).await.expect("valid playlist item");
    insert_item(&pool, bp, "lens", 0).await.expect("valid lens item");
    insert_item(&pool, bp, "track", tr).await.expect("valid track item");

    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM hub_backpack_items").await,
        4
    );
}

#[tokio::test]
async fn at_most_one_active_backpack_per_user() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;

    insert_backpack(&pool, alice, "A1", 1).await;

    // Second active backpack for the same user → error.
    let res = sqlx::query(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, 'A2', 1, ?2, ?2)",
    )
    .bind(alice)
    .bind(TS)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "second active backpack for same user must fail");

    // Inactive second backpack for the same user → OK.
    insert_backpack(&pool, alice, "A3", 0).await;

    // Active backpack for a different user → OK.
    insert_backpack(&pool, bob, "B1", 1).await;

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM hub_backpacks").await, 3);
}

#[tokio::test]
async fn backpack_name_unique_per_user_case_insensitive() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;

    insert_backpack(&pool, alice, "Party", 0).await;

    // Same name, different case, same user → error.
    let res = sqlx::query(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, 'party', 0, ?2, ?2)",
    )
    .bind(alice)
    .bind(TS)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "case-insensitive duplicate name must fail");

    // Same name for a different user → OK.
    insert_backpack(&pool, bob, "Party", 0).await;
}

#[tokio::test]
async fn foreign_keys_cascade() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let tag = insert_tag(&pool, alice, "dark").await;
    let bp = insert_backpack(&pool, alice, "B", 0).await;

    // Item referencing a tag, plus a second backpack whose item references a track.
    insert_item(&pool, bp, "tag", tag).await.unwrap();
    let bp2 = insert_backpack(&pool, alice, "B2", 0).await;
    let tr = insert_track(&pool, "t-1").await;
    insert_item(&pool, bp2, "track", tr).await.unwrap();

    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM hub_backpack_items").await,
        2
    );

    // Delete one backpack (the track one) → its items go away.
    sqlx::query("DELETE FROM hub_backpacks WHERE id = ?1")
        .bind(bp2)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM hub_backpack_items").await,
        1,
        "deleting a backpack must cascade to its items"
    );

    // Delete the source tag of the remaining item → the item goes away.
    sqlx::query("DELETE FROM hub_tags WHERE id = ?1")
        .bind(tag)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM hub_backpack_items").await,
        0,
        "deleting a source tag must cascade to referencing items"
    );

    // Delete the user → backpacks (and their items) go away.
    insert_item(&pool, bp, "track", tr).await.unwrap();
    sqlx::query("DELETE FROM hub_users WHERE id = ?1")
        .bind(alice)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM hub_backpacks").await,
        0,
        "deleting a user must cascade to their backpacks"
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM hub_backpack_items").await,
        0,
        "deleting a user must cascade to their backpacks' items"
    );
}

#[tokio::test]
async fn indexes_exist() {
    let (_dir, pool) = setup().await;
    let expected = [
        "idx_hub_backpacks_active_user",
        "idx_hub_backpacks_user",
        "idx_hub_backpacks_user_name",
        "idx_hub_backpack_items_backpack",
        "idx_hub_backpack_items_tag",
        "idx_hub_backpack_items_playlist",
        "idx_hub_backpack_items_track",
    ];
    for idx in expected {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
        )
        .bind(idx)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1, "index {idx} should exist");
    }
}
