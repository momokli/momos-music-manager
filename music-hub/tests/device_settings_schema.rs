//! Migration test for the per-device Kellner settings schema (#369).
//!
//! Boots a fresh temp SQLite with only the real migrations applied and checks
//! the persistence invariants of `hub_device_settings` / `hub_device_backpacks`:
//! table + index presence, the `preferred_format` `CHECK`, the composite PK on
//! the join table and `ON DELETE CASCADE` behaviour.

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

async fn insert_device(pool: &SqlitePool, user_id: i64, name: &str, token_hash: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_devices (user_id, name, token_hash, created_at, paired_at)
         VALUES (?1, ?2, ?3, ?4, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(token_hash)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert device")
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

async fn object_exists(pool: &SqlitePool, kind: &str, name: &str) -> bool {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
    )
    .bind(kind)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("sqlite_master");
    n == 1
}

// ── schema presence ─────────────────────────────────────────────────────────

#[tokio::test]
async fn tables_and_indexes_exist() {
    let (_dir, pool) = setup().await;
    assert!(object_exists(&pool, "table", "hub_device_settings").await);
    assert!(object_exists(&pool, "table", "hub_device_backpacks").await);
    assert!(object_exists(&pool, "index", "idx_hub_device_backpacks_device").await);
    assert!(object_exists(&pool, "index", "idx_hub_device_backpacks_backpack").await);
}

// ── preferred_format CHECK ──────────────────────────────────────────────────

#[tokio::test]
async fn preferred_format_check_rejects_unknown() {
    let (_dir, pool) = setup().await;
    let uid = insert_user(&pool, "alice").await;
    let did = insert_device(&pool, uid, "kellner-1", &"a".repeat(64)).await;

    for good in ["flac", "mp3", "wav", "m4a"] {
        let res = sqlx::query(
            "INSERT INTO hub_device_settings (device_id, preferred_format, comment_template, updated_at)
             VALUES (?1, ?2, '', ?3)",
        )
        .bind(did)
        .bind(good)
        .bind(TS)
        .execute(&pool)
        .await;
        assert!(res.is_ok(), "{good} should be accepted: {res:?}");
        sqlx::query("DELETE FROM hub_device_settings WHERE device_id = ?1")
            .bind(did)
            .execute(&pool)
            .await
            .unwrap();
    }

    let bad = sqlx::query(
        "INSERT INTO hub_device_settings (device_id, preferred_format, comment_template, updated_at)
         VALUES (?1, 'ogg', '', ?2)",
    )
    .bind(did)
    .bind(TS)
    .execute(&pool)
    .await;
    assert!(bad.is_err(), "CHECK must reject preferred_format='ogg'");
}

// ── cascade ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn deleting_device_cascades_settings_and_backpacks() {
    let (_dir, pool) = setup().await;
    let uid = insert_user(&pool, "alice").await;
    let did = insert_device(&pool, uid, "kellner-1", &"a".repeat(64)).await;
    let bpa = insert_backpack(&pool, uid, "bp-a").await;
    let bpb = insert_backpack(&pool, uid, "bp-b").await;

    sqlx::query(
        "INSERT INTO hub_device_settings (device_id, preferred_format, comment_template, updated_at)
         VALUES (?1, 'mp3', 'hi', ?2)",
    )
    .bind(did)
    .bind(TS)
    .execute(&pool)
    .await
    .expect("insert settings");

    for (pos, bp) in [bpa, bpb].into_iter().enumerate() {
        sqlx::query(
            "INSERT INTO hub_device_backpacks (device_id, backpack_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(did)
        .bind(bp)
        .bind(pos as i64)
        .bind(TS)
        .execute(&pool)
        .await
        .expect("insert backpack link");
    }

    // Composite PK rejects a duplicate (device_id, backpack_id).
    let dup = sqlx::query(
        "INSERT INTO hub_device_backpacks (device_id, backpack_id, position, added_at)
         VALUES (?1, ?2, 5, ?3)",
    )
    .bind(did)
    .bind(bpa)
    .bind(TS)
    .execute(&pool)
    .await;
    assert!(dup.is_err(), "composite PK must reject duplicate link");

    sqlx::query("DELETE FROM hub_devices WHERE id = ?1")
        .bind(did)
        .execute(&pool)
        .await
        .expect("delete device");

    let settings: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_device_settings WHERE device_id = ?1")
            .bind(did)
            .fetch_one(&pool)
            .await
            .unwrap();
    let links: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_device_backpacks WHERE device_id = ?1")
            .bind(did)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(settings, 0, "settings row cascades on device delete");
    assert_eq!(links, 0, "backpack links cascade on device delete");
}
