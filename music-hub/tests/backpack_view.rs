//! Integration tests for the backpack view (#332): `/backpacks` cards, live
//! counts and labelled sources.
//!
//! Two layers:
//! * helper behaviour on a fresh temp SQLite (`setup()`) — `list_backpacks`
//!   scoping/order and `list_source_views` labels/fallbacks;
//! * HTTP view against the `common` harness (`spawn()`) — cards render with
//!   source counts, labels and unclamped live track counts, plus access
//!   control (foreign backpacks hidden, logged-out redirect).

mod common;

use std::str::FromStr;
use std::time::Duration;

use music_hub::backpacks::{self, SourceRef};
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
    sqlx::migrate!("./migrations").run(&pool).await.expect("migrate");
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

async fn insert_backpack(pool: &SqlitePool, user_id: i64, name: &str, at: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, ?2, 0, ?3, ?3) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(at)
    .fetch_one(pool)
    .await
    .expect("insert backpack")
}

async fn insert_tag(pool: &SqlitePool, owner: i64, slug: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at) VALUES (?1, ?2, ?3, ?4) RETURNING id",
    )
    .bind(owner)
    .bind(slug)
    .bind(name)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert tag")
}

// ── Story 1: list_backpacks ──────────────────────────────────────────────────

#[tokio::test]
async fn list_backpacks_scopes_to_user_and_orders_by_created_at() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;

    // Insert out of order so ordering is actually exercised.
    insert_backpack(&pool, alice, "Later", "2026-02-01T00:00:00+00:00").await;
    let first = insert_backpack(&pool, alice, "Earlier", "2026-01-01T00:00:00+00:00").await;
    insert_backpack(&pool, bob, "Bob's", "2025-01-01T00:00:00+00:00").await;

    let list = backpacks::list_backpacks(&pool, alice).await.unwrap();
    assert_eq!(list.len(), 2, "only alice's backpacks");
    assert_eq!(list[0].id, first, "ordered by created_at");
    assert_eq!(list[0].name, "Earlier");
    assert!(list.iter().all(|b| b.user_id == alice));

    // Unknown user -> empty, no error.
    let none = backpacks::list_backpacks(&pool, 999_999).await.unwrap();
    assert!(none.is_empty());
}

// ── Story 2: list_source_views ───────────────────────────────────────────────

#[tokio::test]
async fn list_source_views_resolves_labels_and_fallbacks() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, alice, "B", TS).await;

    let tag = insert_tag(&pool, alice, "dark", "Dark").await;
    let track: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists) VALUES ('spotify','x','Carol Only','Fff') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Lens("liked".to_string())).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Track(track)).await.unwrap();

    let views = backpacks::list_source_views(&pool, bp).await.unwrap();
    assert_eq!(views.len(), 3);
    assert_eq!(views[0].kind, "tag");
    assert_eq!(views[0].label, "Dark");
    assert_eq!(views[1].kind, "lens");
    assert_eq!(views[1].label, "Meine Likes");
    assert_eq!(views[2].kind, "track");
    assert_eq!(views[2].label, "Carol Only — Fff");
    // Positions ascend in append order.
    assert!(views[0].position < views[1].position && views[1].position < views[2].position);

    // Unknown backpack -> empty, never an error.
    assert!(backpacks::list_source_views(&pool, 999_999).await.unwrap().is_empty());
}

// ── Story 3 + 4 + 5: HTTP view ───────────────────────────────────────────────

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
async fn backpacks_page_renders_cards_with_counts_and_labels() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("packer", "pw").await;

    // A tag carrying two of the seeded tracks.
    let tag = insert_tag(&app.pool, owner, "moody", "Moody").await;
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(app.seed.t_all)
        .bind(tag)
        .execute(&app.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(app.seed.t_two)
        .bind(tag)
        .execute(&app.pool)
        .await
        .unwrap();

    let bp = insert_backpack(&app.pool, owner, "Reisekoffer", TS).await;
    backpacks::add_source(&app.pool, bp, SourceRef::Tag(tag)).await.unwrap();
    // pl_alice (seeded, owned by fixture alice) contains t_all + t_pl.
    backpacks::add_source(&app.pool, bp, SourceRef::Playlist(app.seed.pl_alice)).await.unwrap();
    // owner likes nothing; use the fixture-free "newest" lens instead: every hub track.
    let bp2 = insert_backpack(&app.pool, owner, "Alles", TS).await;
    backpacks::add_source(&app.pool, bp2, SourceRef::Lens("newest".to_string())).await.unwrap();

    let body = get(&app, "/backpacks", &cookie).await;

    assert!(body.contains("Reisekoffer"), "backpack name is shown");
    assert!(body.contains("2 Quellen"), "source count is shown");
    assert!(body.contains("Moody"), "tag label is shown");
    assert!(body.contains("tag"), "source kind is shown");
    assert!(body.contains("Deep House"), "playlist label is shown");
    // Union of tag {t_all,t_two} + playlist {t_all,t_pl} = 3 distinct tracks.
    assert!(body.contains("3 Tracks (live)"), "live track count without clamp");
    assert!(body.contains("Alles"), "second card renders");
    // Nav marks the page current.
    assert!(body.contains("aria-current=\"page\""));
}

#[tokio::test]
async fn backpacks_page_empty_state() {
    let app = common::spawn().await;
    let (_owner, cookie) = app.user_with_password("nothing", "pw").await;
    let body = get(&app, "/backpacks", &cookie).await;
    assert!(body.contains("Noch keine Backpacks"), "empty state is shown");
}

#[tokio::test]
async fn backpacks_page_hides_foreign_backpacks() {
    let app = common::spawn().await;
    let (_owner_a, cookie_a) = app.user_with_password("owner-a", "pw").await;
    let (owner_b, _cookie_b) = app.user_with_password("owner-b", "pw").await;
    insert_backpack(&app.pool, owner_b, "Secret B", TS).await;

    let body = get(&app, "/backpacks", &cookie_a).await;
    assert!(!body.contains("Secret B"), "foreign backpack must not leak");
}

#[tokio::test]
async fn backpacks_page_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/backpacks"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

#[tokio::test]
async fn track_count_exceeds_old_500_clamp() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("big", "pw").await;

    // 1200 extra tracks so lens("newest") resolves well past the old clamp.
    for i in 0..1200 {
        sqlx::query(
            "INSERT INTO hub_tracks (service, service_track_id, title, artists) VALUES ('spotify', ?1, 'Bulk', 'X')",
        )
        .bind(format!("bulk-{i}"))
        .execute(&app.pool)
        .await
        .unwrap();
    }
    let bp = insert_backpack(&app.pool, owner, "Big", TS).await;
    backpacks::add_source(&app.pool, bp, SourceRef::Lens("newest".to_string())).await.unwrap();

    let count = backpacks::resolve_count(&app.pool, bp).await.unwrap();
    assert!(count > 500, "resolver count exceeds the old clamp: {count}");

    let body = get(&app, "/backpacks", &cookie).await;
    assert!(
        body.contains(&format!("{count} Tracks (live)")),
        "unclamped live count renders ({count})"
    );
}
