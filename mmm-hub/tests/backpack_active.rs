//! Integration tests for the active-backpack scope (#331).
//!
//! Two layers:
//! * helper behaviour on a fresh temp SQLite (`setup()` copied from
//!   `backpack_resolver.rs`) — set/shift/clear + scope set;
//! * HTTP view filtering against the `common` harness (`spawn()`) — `/tags`,
//!   `/artists`, `/albums` with and without an active backpack, plus the
//!   `POST /backpacks/active/clear` roundtrip.

mod common;

use std::collections::HashSet;
use std::str::FromStr;
use std::time::Duration;

use mmm_hub::backpacks::{self, SourceRef};
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

async fn insert_track(pool: &SqlitePool, ext: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id) VALUES ('spotify', ?1) RETURNING id",
    )
    .bind(ext)
    .fetch_one(pool)
    .await
    .expect("insert track")
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

async fn insert_resolved_tag(pool: &SqlitePool, tag_id: i64, track_id: i64) {
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track_id)
        .bind(tag_id)
        .execute(pool)
        .await
        .expect("insert resolved tag");
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

// ── helpers ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn active_backpack_none_then_set_then_shift_then_clear() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let a = insert_backpack(&pool, alice, "A", 0).await;
    let b = insert_backpack(&pool, alice, "B", 0).await;

    assert!(backpacks::active_backpack(&pool, alice).await.unwrap().is_none());

    backpacks::set_active(&pool, alice, a).await.unwrap();
    let active = backpacks::active_backpack(&pool, alice).await.unwrap().unwrap();
    assert_eq!(active.id, a);
    assert_eq!(active.name, "A");
    assert_eq!(active.is_active, 1);

    // Shifting to B must not violate the partial UNIQUE index.
    backpacks::set_active(&pool, alice, b).await.unwrap();
    let active = backpacks::active_backpack(&pool, alice).await.unwrap().unwrap();
    assert_eq!(active.id, b, "only B stays active");
    let active_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_backpacks WHERE user_id = ?1 AND is_active = 1",
    )
    .bind(alice)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active_rows, 1, "partial-unique holds at most one active row");

    let cleared = backpacks::clear_active(&pool, alice).await.unwrap();
    assert_eq!(cleared, 1, "one row deactivated");
    assert!(backpacks::active_backpack(&pool, alice).await.unwrap().is_none());
    assert_eq!(backpacks::clear_active(&pool, alice).await.unwrap(), 0, "idempotent");
}

#[tokio::test]
async fn set_active_rejects_foreign_backpack() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;
    let bob_bp = insert_backpack(&pool, bob, "Bob's", 0).await;

    let err = backpacks::set_active(&pool, alice, bob_bp).await;
    assert!(err.is_err(), "cross-user activation must fail");
    assert!(
        backpacks::active_backpack(&pool, bob).await.unwrap().is_none(),
        "no silent foreign activation"
    );
}

#[tokio::test]
async fn active_scope_track_ids_none_and_some() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, alice, "B", 0).await;
    let tag = insert_tag(&pool, alice, "dark").await;
    let t1 = insert_track(&pool, "t1").await;
    let t2 = insert_track(&pool, "t2").await;
    let t3 = insert_track(&pool, "t3").await;
    insert_resolved_tag(&pool, tag, t1).await;
    insert_resolved_tag(&pool, tag, t2).await;
    backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();

    // No active backpack -> None (unscoped).
    assert!(backpacks::active_scope_track_ids(&pool, alice).await.unwrap().is_none());

    backpacks::set_active(&pool, alice, bp).await.unwrap();
    let scope = backpacks::active_scope_track_ids(&pool, alice)
        .await
        .unwrap()
        .expect("Some when active");
    let want: HashSet<i64> = [t1, t2].into_iter().collect();
    assert_eq!(scope, want);
    assert!(!scope.contains(&t3));

    backpacks::clear_active(&pool, alice).await.unwrap();
    assert!(backpacks::active_scope_track_ids(&pool, alice).await.unwrap().is_none());
}

// ── HTTP view filters + clear roundtrip ──────────────────────────────────────

async fn insert_tags_for_scope(app: &common::TestApp, owner: i64) {
    let tag_a = insert_tag(&app.pool, owner, "tag-a").await;
    let tag_b = insert_tag(&app.pool, owner, "tag-b").await;
    insert_resolved_tag(&app.pool, tag_a, app.seed.t_all).await;
    insert_resolved_tag(&app.pool, tag_a, app.seed.t_two).await;
    insert_resolved_tag(&app.pool, tag_b, app.seed.t_carol).await;

    // Albums for the album view (seeded tracks have no album).
    sqlx::query("UPDATE hub_tracks SET album = 'Alpha' WHERE id IN (?1, ?2)")
        .bind(app.seed.t_all)
        .bind(app.seed.t_two)
        .execute(&app.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE hub_tracks SET album = 'Beta' WHERE id = ?1")
        .bind(app.seed.t_carol)
        .execute(&app.pool)
        .await
        .unwrap();

    // An active backpack owning tag-a as its only source.
    let bp = insert_backpack(&app.pool, owner, "scope-bp", 0).await;
    backpacks::add_source(&app.pool, bp, SourceRef::Tag(tag_a)).await.unwrap();
    backpacks::set_active(&app.pool, owner, bp).await.unwrap();
}

async fn get(app: &common::TestApp, path: &str, cookie: &str) -> String {
    let resp = app
        .client()
        .get(app.url(path))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "GET {path}");
    resp.text().await.unwrap()
}

#[tokio::test]
async fn views_are_scoped_and_clear_restores_unscoped() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("scoper", "pw").await;

    // Baseline before any backpack exists: unscoped list.
    let body = get(&app, "/tags", &cookie).await;
    assert!(!body.contains("tag-a") && !body.contains("tag-b"), "no tags yet");
    insert_tags_for_scope(&app, owner).await;

    // Scoped: only the tags/artists/albums with >= 1 track in the scope survive.
    let tags = get(&app, "/tags", &cookie).await;
    assert!(tags.contains("tag-a"), "scoped tag is listed");
    assert!(!tags.contains("tag-b"), "tag outside the scope is hidden");

    let artists = get(&app, "/artists", &cookie).await;
    assert!(artists.contains("Aaa"), "artist Aaa (t_all) stays");
    assert!(!artists.contains("Fff"), "artist Fff (t_carol) is hidden");

    let albums = get(&app, "/albums", &cookie).await;
    assert!(albums.contains("Alpha"), "album Alpha (t_all/t_two) stays");
    assert!(!albums.contains("Beta"), "album Beta (t_carol) is hidden");

    // Clear roundtrip: POST with a Referer, expect 303 back to it.
    let resp = app
        .client()
        .post(app.url("/backpacks/active/clear"))
        .header("Cookie", &cookie)
        .header("Referer", &app.url("/tags"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/tags")
    );

    // Unscoped again: every tag is back.
    let tags = get(&app, "/tags", &cookie).await;
    assert!(tags.contains("tag-a") && tags.contains("tag-b"), "scope cleared");

    // Clear form is gone from the topbar (no active backpack).
    assert!(!tags.contains("Scope:"), "no scope badge without an active backpack");
}

#[tokio::test]
async fn empty_active_scope_yields_200_and_empty_lists() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("empty-scoper", "pw").await;

    // Content that an *unscoped* view would happily show.
    let tag = insert_tag(&app.pool, owner, "lonely-tag").await;
    insert_resolved_tag(&app.pool, tag, app.seed.t_all).await;
    sqlx::query("UPDATE hub_tracks SET album = 'Solo' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();

    // An active backpack with *no sources* resolves no tracks: Some(empty set),
    // not None. This is the scope the list views must survive.
    let bp = insert_backpack(&app.pool, owner, "empty-bp", 0).await;
    backpacks::set_active(&app.pool, owner, bp).await.unwrap();

    let scope = backpacks::active_scope_track_ids(&app.pool, owner).await.unwrap();
    assert_eq!(
        scope,
        Some(HashSet::<i64>::new()),
        "active backpack without resolving tracks -> Some(empty set)"
    );

    // Empty scope must yield an empty (200) list, never a 500 from `IN ()`.
    let tags = get(&app, "/tags", &cookie).await;
    assert!(!tags.contains("lonely-tag"), "empty scope hides every tag");
    let albums = get(&app, "/albums", &cookie).await;
    assert!(!albums.contains("Solo"), "empty scope hides every album");
}

#[tokio::test]
async fn clear_rejects_protocol_relative_referer() {
    let app = common::spawn().await;
    let (_owner, cookie) = app.user_with_password("scoper3", "pw").await;

    // A protocol-relative Referer (`//evil.test/x`) must not be reflected into
    // the Location header; fall back to the internal `/tags` instead.
    for referer in ["//evil.test/x", "/\\evil.test/x"] {
        let resp = app
            .client()
            .post(app.url("/backpacks/active/clear"))
            .header("Cookie", &cookie)
            .header("Referer", referer)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
        assert_eq!(
            resp.headers().get("location").and_then(|v| v.to_str().ok()),
            Some("/tags"),
            "open-redirect vector {referer:?} must fall back to /tags"
        );
    }
}

#[tokio::test]
async fn clear_without_referer_falls_back_to_tags() {
    let app = common::spawn().await;
    let (_owner, cookie) = app.user_with_password("scoper2", "pw").await;
    let resp = app
        .client()
        .post(app.url("/backpacks/active/clear"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/tags")
    );
}
