//! Migration test for the Tonie binding schema (#500, #504).
//!
//! Boots a fresh temp SQLite with only the real migrations applied and checks
//! the persistence invariants of `hub_tonie_bindings`: table/index presence, the
//! column defaults, `UNIQUE(user_id, tonie_id)`, the `ON DELETE CASCADE` on the
//! owning user, and (#504) that `source_id` has **no** foreign key — deleting the
//! bound source leaves the binding in place (acceptance #2).

use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

const TS: &str = "2026-01-01T00:00:00+00:00";

/// The 060 rebuild body, run verbatim by [`migration_060_rebuild_keeps_binding_and_sync_rows`].
const MIGRATION_060: &str = include_str!("../migrations/060_hub_tonie_bindings_source_kind.sql");

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

async fn insert_binding(
    pool: &SqlitePool,
    user_id: i64,
    tonie_id: &str,
    playlist_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO hub_tonie_bindings (user_id, tonie_id, source_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(tonie_id)
    .bind(playlist_id)
    .bind(TS)
    .fetch_one(pool)
    .await
}

async fn insert_sync(pool: &SqlitePool, binding_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tonie_syncs (binding_id, started_at, status)
         VALUES (?1, ?2, 'ok') RETURNING id",
    )
    .bind(binding_id)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert sync")
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("count")
}

#[tokio::test]
async fn table_and_index_exist() {
    let (_dir, pool) = setup().await;
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'hub_tonie_bindings'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "hub_tonie_bindings should exist");

    let idx: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_hub_tonie_bindings_user'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(idx, 1, "user index should exist");
}

#[tokio::test]
async fn column_defaults() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    insert_binding(&pool, user, "ct1", pl).await.expect("insert");

    let (kind, format, mode, cap): (String, String, String, i64) = sqlx::query_as(
        "SELECT source_kind, format, mode, cap_seconds FROM hub_tonie_bindings WHERE tonie_id = 'ct1'",
    )
    .fetch_one(&pool)
    .await
    .expect("row");
    assert_eq!(kind, "playlist");
    assert_eq!(format, "mp3-128");
    assert_eq!(mode, "manual");
    assert_eq!(cap, 5400);
}

#[tokio::test]
async fn unique_per_user_and_tonie() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    insert_binding(&pool, user, "ct1", pl).await.expect("first");

    // Same (user, tonie) again → rejected.
    let dup = insert_binding(&pool, user, "ct1", pl).await;
    assert!(dup.is_err(), "duplicate (user_id, tonie_id) must be rejected");

    // A different user may bind the same Tonie id.
    let bob = insert_user(&pool, "bob").await;
    let pl_b = insert_playlist(&pool, bob, "pl-b").await;
    insert_binding(&pool, bob, "ct1", pl_b).await.expect("other user ok");
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM hub_tonie_bindings").await, 2);
}

#[tokio::test]
async fn user_delete_cascades() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    insert_binding(&pool, user, "ct1", pl).await.expect("insert");

    sqlx::query("DELETE FROM hub_users WHERE id = ?1")
        .bind(user)
        .execute(&pool)
        .await
        .expect("delete user");
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM hub_tonie_bindings").await, 0);
}

#[tokio::test]
async fn playlist_delete_keeps_binding() {
    // #504 acceptance #2: the source is a plain reference — deleting the bound
    // playlist must NOT cascade the binding away. `source_kind` may now be a
    // backpack or tag too, so a playlist FK would be wrong anyway.
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    insert_binding(&pool, user, "ct1", pl).await.expect("insert");

    sqlx::query("DELETE FROM hub_playlists WHERE id = ?1")
        .bind(pl)
        .execute(&pool)
        .await
        .expect("delete playlist");
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM hub_tonie_bindings").await,
        1,
        "the binding must survive its deleted source"
    );
}

#[tokio::test]
async fn source_id_has_no_foreign_key() {
    // The rebuild in 060 drops the `source_id` FK; `foreign_key_list` must show
    // no `source_id` row (the `user_id` FK stays).
    let (_dir, pool) = setup().await;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT \"from\", \"table\" FROM pragma_foreign_key_list('hub_tonie_bindings')",
    )
    .fetch_all(&pool)
    .await
    .expect("foreign_key_list");
    assert!(
        !rows.iter().any(|(from, _)| from == "source_id"),
        "source_id must not have a foreign key: {rows:?}"
    );
    assert!(
        rows.iter().any(|(from, _)| from == "user_id"),
        "user_id must keep its foreign key: {rows:?}"
    );
}

#[tokio::test]
async fn fk_enforcement_is_active() {
    // Control for the rebuild test below: prove the test connection really
    // enforces foreign keys, so "the sync row survives the rebuild" is a
    // meaningful assertion rather than an artefact of FKs being off.
    let (_dir, pool) = setup().await;
    let fk_on: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(fk_on, 1, "test connection must enforce foreign keys");

    let user = insert_user(&pool, "alice").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    let binding = insert_binding(&pool, user, "ct1", pl).await.expect("insert");
    insert_sync(&pool, binding).await;

    // Deleting the binding must cascade its run log — the FK is live.
    sqlx::query("DELETE FROM hub_tonie_bindings WHERE id = ?1")
        .bind(binding)
        .execute(&pool)
        .await
        .expect("delete binding");
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM hub_tonie_syncs").await,
        0,
        "hub_tonie_syncs must cascade with its binding when FKs are on"
    );
}

#[tokio::test]
async fn migration_060_rebuild_keeps_binding_and_sync_rows() {
    // Closes the verifier gap: the 060 table rebuild runs with
    // `PRAGMA foreign_keys=OFF` so that `hub_tonie_syncs.binding_id`
    // (ON DELETE CASCADE) does not fire while the old table is dropped. That
    // guarantee is only indirectly covered when no run row exists yet — this
    // test populates both a binding and a run-log row *before* the rebuild and
    // asserts both survive verbatim.
    let (_dir, pool) = setup().await;

    let user = insert_user(&pool, "alice").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    let binding = insert_binding(&pool, user, "ct1", pl).await.expect("binding");
    let sync = insert_sync(&pool, binding).await;

    let fk_before: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(fk_before, 1, "FKs must be on going into the rebuild");

    // Run the real 060 SQL body verbatim on the populated DB. Without the
    // migration's `foreign_keys=OFF` guard, DROP TABLE would implicitly delete
    // the rows and cascade the sync row away.
    sqlx::raw_sql(MIGRATION_060)
        .execute(&pool)
        .await
        .expect("run migration 060 rebuild");

    // The binding row survives with its id and source reference intact.
    let bindings: Vec<(i64, String, i64)> =
        sqlx::query_as("SELECT id, tonie_id, source_id FROM hub_tonie_bindings")
            .fetch_all(&pool)
            .await
            .expect("binding rows");
    assert_eq!(
        bindings,
        vec![(binding, "ct1".to_string(), pl)],
        "existing binding must survive the rebuild unchanged"
    );

    // The pre-existing run-log row survives — it must NOT be cascaded away.
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM hub_tonie_syncs").await,
        1,
        "existing hub_tonie_syncs row must survive the rebuild"
    );
    let kept: i64 = sqlx::query_scalar("SELECT binding_id FROM hub_tonie_syncs WHERE id = ?1")
        .bind(sync)
        .fetch_one(&pool)
        .await
        .expect("sync row still present");
    assert_eq!(kept, binding, "run log must still point at the same binding");

    // `source_id` lost its FK; the `user_id` FK stays.
    let fk_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT \"from\", \"table\" FROM pragma_foreign_key_list('hub_tonie_bindings')",
    )
    .fetch_all(&pool)
    .await
    .expect("foreign_key_list");
    assert!(
        !fk_rows.iter().any(|(from, _)| from == "source_id"),
        "source_id must have no FK after the rebuild: {fk_rows:?}"
    );
    assert!(
        fk_rows.iter().any(|(from, _)| from == "user_id"),
        "user_id must keep its FK after the rebuild: {fk_rows:?}"
    );

    // `UNIQUE(user_id, tonie_id)` is preserved (duplicate insert is rejected).
    let dup = insert_binding(&pool, user, "ct1", pl).await;
    assert!(
        dup.is_err(),
        "UNIQUE(user_id, tonie_id) must survive the rebuild"
    );

    // The user index is recreated by the rebuild.
    let idx: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_hub_tonie_bindings_user'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(idx, 1, "idx_hub_tonie_bindings_user must survive the rebuild");

    // The migration re-enables FK enforcement on the connection.
    let fk_after: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(fk_after, 1, "FKs must be re-enabled after the rebuild");
}
