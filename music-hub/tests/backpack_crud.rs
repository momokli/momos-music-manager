//! Integration tests for backpack CRUD + active toggle (#328).
//!
//! Two layers, mirroring `backpack_active.rs` / `backpack_view.rs`:
//! * helper behaviour on a fresh temp SQLite (`setup()`) — create / rename /
//!   delete with trimming, case-insensitive per-user duplicates, ownership and
//!   CASCADE;
//! * HTTP layer against the `common` harness (`spawn()`) — the four mutating
//!   routes, redirects (`SEE_OTHER`), flash-on-error and the login gate.

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

async fn backpack_count(pool: &SqlitePool, user_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM hub_backpacks WHERE user_id = ?1")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn item_count(pool: &SqlitePool, backpack_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM hub_backpack_items WHERE backpack_id = ?1")
        .bind(backpack_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ── helper layer ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_trims_and_rejects_empty_and_duplicates() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;

    // Trimming: the stored name is the trimmed one.
    let id = backpacks::create_backpack(&pool, alice, "  Chill  ").await.unwrap();
    let bps = backpacks::list_backpacks(&pool, alice).await.unwrap();
    assert_eq!(bps.len(), 1);
    assert_eq!(bps[0].name, "Chill");
    assert_eq!(bps[0].is_active, 0, "new backpack starts inactive");
    assert_eq!(bps[0].id, id);

    // Case-insensitive duplicate of the *same* user is rejected, no insert.
    assert!(
        backpacks::create_backpack(&pool, alice, "chill").await.is_err(),
        "case-insensitive duplicate rejected"
    );
    assert_eq!(backpack_count(&pool, alice).await, 1, "no second row");

    // Empty / whitespace-only rejected.
    assert!(backpacks::create_backpack(&pool, alice, "").await.is_err());
    assert!(backpacks::create_backpack(&pool, alice, "   ").await.is_err());
    assert_eq!(backpack_count(&pool, alice).await, 1);

    // The same name is fine for another user.
    backpacks::create_backpack(&pool, bob, "chill").await.unwrap();
    assert_eq!(backpack_count(&pool, bob).await, 1);
}

#[tokio::test]
async fn rename_enforces_ownership_and_bumps_updated_at() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;
    let a = insert_backpack(&pool, alice, "A", 0).await;
    let b = insert_backpack(&pool, alice, "B", 0).await;
    let bob_bp = insert_backpack(&pool, bob, "Bob's", 0).await;

    // Renaming to another of the user's names is a duplicate error.
    assert!(backpacks::rename_backpack(&pool, alice, a, "B").await.is_err());
    // Empty name rejected.
    assert!(backpacks::rename_backpack(&pool, alice, a, "  ").await.is_err());

    // Trims and bumps updated_at.
    backpacks::rename_backpack(&pool, alice, a, "  Alpha  ").await.unwrap();
    let (name, updated): (String, String) =
        sqlx::query_as("SELECT name, updated_at FROM hub_backpacks WHERE id = ?1")
            .bind(a)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(name, "Alpha");
    assert_ne!(updated, TS, "updated_at bumped on rename");

    // Case-insensitive clash also rejected.
    assert!(backpacks::rename_backpack(&pool, alice, a, "b").await.is_err());

    // Foreign / missing target: error, no write.
    assert!(
        backpacks::rename_backpack(&pool, alice, bob_bp, "hacked").await.is_err(),
        "cross-user rename rejected"
    );
    let bob_name: String =
        sqlx::query_scalar("SELECT name FROM hub_backpacks WHERE id = ?1")
            .bind(bob_bp)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(bob_name, "Bob's", "foreign backpack untouched");

    assert!(
        backpacks::rename_backpack(&pool, alice, 999_999, "ghost").await.is_err(),
        "missing target rejected"
    );
    let _ = b; // keep B alive (used as duplicate target above)
}

#[tokio::test]
async fn delete_cascades_and_enforces_ownership() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;
    let a = insert_backpack(&pool, alice, "A", 0).await;
    let bob_bp = insert_backpack(&pool, bob, "Bob's", 0).await;

    // Give A a source so CASCADE is observable.
    let tag = insert_tag(&pool, alice, "dark").await;
    backpacks::add_source(&pool, a, SourceRef::Tag(tag)).await.unwrap();
    assert_eq!(item_count(&pool, a).await, 1);

    // Foreign / missing -> false, nothing removed.
    assert!(!backpacks::delete_backpack(&pool, alice, bob_bp).await.unwrap());
    assert_eq!(backpack_count(&pool, bob).await, 1);
    assert!(!backpacks::delete_backpack(&pool, alice, 999_999).await.unwrap());

    // Own backpack -> true, items cascade away.
    assert!(backpacks::delete_backpack(&pool, alice, a).await.unwrap());
    assert_eq!(backpack_count(&pool, alice).await, 0);
    assert_eq!(item_count(&pool, a).await, 0, "items removed via CASCADE");

    // Idempotent second delete.
    assert!(!backpacks::delete_backpack(&pool, alice, a).await.unwrap());
}

// ── HTTP layer ───────────────────────────────────────────────────────────────

fn count_cards(body: &str) -> usize {
    body.matches("hub-backpack-card").count()
}

async fn get_backpacks(app: &common::TestApp, cookie: &str) -> String {
    let resp = app
        .client()
        .get(app.url("/backpacks"))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    resp.text().await.unwrap()
}

fn location(resp: &reqwest::Response) -> String {
    resp.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn http_create_then_duplicate_flash() {
    let app = common::spawn().await;
    let (_owner, cookie) = app.user_with_password("creator", "pw").await;

    let resp = app
        .client()
        .post(app.url("/backpacks/create"))
        .header("Cookie", &cookie)
        .form(&[("name", "Roadtrip")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/backpacks");

    let body = get_backpacks(&app, &cookie).await;
    assert!(body.contains("Roadtrip"), "new backpack is listed");
    assert_eq!(count_cards(&body), 1);

    // Duplicate (case-insensitive) -> flash, still one card.
    let resp = app
        .client()
        .post(app.url("/backpacks/create"))
        .header("Cookie", &cookie)
        .form(&[("name", "roadtrip")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(location(&resp).starts_with("/backpacks?msg="), "flash on duplicate");

    let body = get_backpacks(&app, &cookie).await;
    assert_eq!(count_cards(&body), 1, "no second entry after duplicate");
}

#[tokio::test]
async fn http_rename_and_foreign_noop() {
    let app = common::spawn().await;
    let (alice_id, alice_cookie) = app.user_with_password("crud-alice", "pw").await;
    let (bob_id, bob_cookie) = app.user_with_password("crud-bob", "pw").await;
    let a = insert_backpack(&app.pool, alice_id, "Old", 0).await;
    let bob_bp = insert_backpack(&app.pool, bob_id, "Bob's", 0).await;

    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{a}/rename")))
        .header("Cookie", &alice_cookie)
        .form(&[("name", "New")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/backpacks");
    let body = get_backpacks(&app, &alice_cookie).await;
    assert!(body.contains("New") && !body.contains(">Old<"), "renamed");

    // Bob renaming Alice's backpack -> flash, no effect for Alice.
    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{a}/rename")))
        .header("Cookie", &bob_cookie)
        .form(&[("name", "Stolen")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(location(&resp).starts_with("/backpacks?msg="), "flash on foreign rename");
    let body = get_backpacks(&app, &alice_cookie).await;
    assert!(body.contains("New") && !body.contains("Stolen"), "foreign rename had no effect");

    let _ = bob_bp;
}

#[tokio::test]
async fn http_delete_removes_only_own() {
    let app = common::spawn().await;
    let (alice_id, alice_cookie) = app.user_with_password("crud-alice", "pw").await;
    let (bob_id, bob_cookie) = app.user_with_password("crud-bob", "pw").await;
    let a = insert_backpack(&app.pool, alice_id, "Doomed", 0).await;
    let bob_bp = insert_backpack(&app.pool, bob_id, "Survivor", 0).await;

    // Foreign delete -> flash, Bob's stays.
    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{bob_bp}/delete")))
        .header("Cookie", &alice_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(location(&resp).starts_with("/backpacks?msg="), "flash on foreign delete");
    let bob_body = get_backpacks(&app, &bob_cookie).await;
    assert!(bob_body.contains("Survivor"), "foreign backpack survives");

    // Own delete -> 303, name gone.
    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{a}/delete")))
        .header("Cookie", &alice_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/backpacks");
    let body = get_backpacks(&app, &alice_cookie).await;
    assert!(!body.contains("Doomed"), "own backpack removed");
}

#[tokio::test]
async fn http_toggle_active_and_foreign_flash() {
    let app = common::spawn().await;
    let (alice_id, alice_cookie) = app.user_with_password("crud-alice", "pw").await;
    let (bob_id, _bob_cookie) = app.user_with_password("crud-bob", "pw").await;
    let bp1 = insert_backpack(&app.pool, alice_id, "One", 0).await;
    let bp2 = insert_backpack(&app.pool, alice_id, "Two", 0).await;
    let bob_bp = insert_backpack(&app.pool, bob_id, "Bob's", 0).await;

    // Activate bp1.
    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{bp1}/active")))
        .header("Cookie", &alice_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/backpacks");
    assert_eq!(
        backpacks::active_backpack(&app.pool, alice_id).await.unwrap().map(|b| b.id),
        Some(bp1)
    );

    // Toggle bp1 again -> deactivated.
    app.client()
        .post(app.url(&format!("/backpacks/{bp1}/active")))
        .header("Cookie", &alice_cookie)
        .send()
        .await
        .unwrap();
    assert!(
        backpacks::active_backpack(&app.pool, alice_id).await.unwrap().is_none(),
        "second toggle deactivates"
    );

    // Activate bp2 then shift to bp1: at most one active row.
    for id in [bp2, bp1] {
        app.client()
            .post(app.url(&format!("/backpacks/{id}/active")))
            .header("Cookie", &alice_cookie)
            .send()
            .await
            .unwrap();
    }
    let active_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_backpacks WHERE user_id = ?1 AND is_active = 1",
    )
    .bind(alice_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(active_rows, 1, "at most one active row");
    assert_eq!(
        backpacks::active_backpack(&app.pool, alice_id).await.unwrap().map(|b| b.id),
        Some(bp1)
    );

    // Foreign toggle -> flash, Alice's active unchanged.
    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{bob_bp}/active")))
        .header("Cookie", &alice_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(location(&resp).starts_with("/backpacks?msg="), "flash on foreign toggle");
    assert_eq!(
        backpacks::active_backpack(&app.pool, alice_id).await.unwrap().map(|b| b.id),
        Some(bp1),
        "foreign toggle left Alice's active backpack intact"
    );
    assert!(
        backpacks::active_backpack(&app.pool, bob_id).await.unwrap().is_none(),
        "no silent foreign activation"
    );
    let body = get_backpacks(&app, &alice_cookie).await;
    assert!(body.contains("hub-badge-ok"), "active badge rendered");
}

#[tokio::test]
async fn http_auth_gate_redirects_to_login() {
    let app = common::spawn().await;

    // Create + rename need a form body so extraction reaches the handler.
    let create = app
        .client()
        .post(app.url("/backpacks/create"))
        .form(&[("name", "x")])
        .send()
        .await
        .unwrap();
    let rename = app
        .client()
        .post(app.url("/backpacks/1/rename"))
        .form(&[("name", "x")])
        .send()
        .await
        .unwrap();
    let delete = app
        .client()
        .post(app.url("/backpacks/1/delete"))
        .send()
        .await
        .unwrap();
    let toggle = app
        .client()
        .post(app.url("/backpacks/1/active"))
        .send()
        .await
        .unwrap();

    for (name, resp) in [
        ("create", create),
        ("rename", rename),
        ("delete", delete),
        ("toggle", toggle),
    ] {
        assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER, "{name} status");
        assert_eq!(location(&resp), "/login", "{name} redirects to login");
    }
}
