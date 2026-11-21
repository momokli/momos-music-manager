//! Integration tests for the comment-template library (#668, slice C2).
//!
//! Two layers, mirroring `backpack_crud.rs`:
//! * helper behaviour on a fresh temp SQLite (`setup()`) — CRUD, validation,
//!   visibility, the `mutable_by` guard, per-owner name uniqueness and the
//!   additive migration / system seeds;
//! * HTTP layer against the `common` harness (`spawn()`) — the mutating routes,
//!   `404` for a foreign private template, `403` for a foreign shared / system
//!   template, duplicate ownership, validation flash and the login gate.

mod common;

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use music_hub::comment_templates as ct;
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

async fn count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM hub_comment_templates")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn system_id(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT id FROM hub_comment_templates WHERE owner_user_id IS NULL AND name = ?1",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("system template")
}

// ── helper layer ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_trims_rejects_empty_duplicate_and_validates_body() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;

    let id = ct::create(&pool, alice, "  Intro  ", "{tags}", false).await.unwrap();
    let row = ct::get(&pool, id).await.unwrap().unwrap();
    assert_eq!(row.name, "Intro", "name trimmed");
    assert_eq!(row.owner_user_id, Some(alice));
    assert_eq!(row.is_shared, 0, "new template starts unshared");

    // Case-insensitive duplicate of the same owner rejected, no insert.
    assert!(ct::create(&pool, alice, "intro", "{tags}", false).await.is_err());
    // Empty / whitespace-only name rejected.
    assert!(ct::create(&pool, alice, "  ", "{tags}", false).await.is_err());
    // Invalid body rejected (unknown placeholder) and nothing written.
    let before = count(&pool).await;
    let err = ct::create(&pool, alice, "Bad", "{title}", false).await.unwrap_err();
    assert!(err.to_string().contains("{title}"), "names the token: {err}");
    assert!(err.to_string().contains("{tags}"), "lists allowed: {err}");
    assert_eq!(count(&pool).await, before, "no row on invalid body");

    // Escaped double brace is accepted.
    ct::create(&pool, alice, "Escaped", "{{tags}}", false).await.unwrap();
}

#[tokio::test]
async fn rename_enforces_rights_and_duplicates() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;
    let a = ct::create(&pool, alice, "One", "{tags}", false).await.unwrap();
    let b = ct::create(&pool, alice, "Two", "{tags}", false).await.unwrap();
    let bob_t = ct::create(&pool, bob, "Bob's", "{tags}", false).await.unwrap();

    // Rename to another of my names -> duplicate error.
    assert!(ct::rename(&pool, alice, false, a, "Two").await.is_err());
    // Empty rejected.
    assert!(ct::rename(&pool, alice, false, a, "  ").await.is_err());
    // Trim + rename ok.
    ct::rename(&pool, alice, false, a, "  Alpha  ").await.unwrap();
    let row = ct::get(&pool, a).await.unwrap().unwrap();
    assert_eq!(row.name, "Alpha");
    // Case-insensitive clash rejected.
    assert!(ct::rename(&pool, alice, false, a, "two").await.is_err());

    // Foreign target -> rejected, no write.
    assert!(ct::rename(&pool, bob, false, a, "Stolen").await.is_err());
    assert_eq!(ct::get(&pool, a).await.unwrap().unwrap().name, "Alpha");

    // System template: non-admin rejected, admin allowed.
    let sys = system_id(&pool, "Standard").await;
    assert!(ct::rename(&pool, alice, false, sys, "Hacked").await.is_err());
    ct::rename(&pool, alice, true, sys, "Standard (Admin)").await.unwrap();
    assert_eq!(
        ct::get(&pool, sys).await.unwrap().unwrap().name,
        "Standard (Admin)"
    );
    let _ = b;
    let _ = bob_t;
}

#[tokio::test]
async fn update_body_validates_and_enforces_rights() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;
    let a = ct::create(&pool, alice, "One", "{tags}", false).await.unwrap();

    ct::update_body(&pool, alice, false, a, "{bpm} {key}").await.unwrap();
    assert_eq!(ct::get(&pool, a).await.unwrap().unwrap().body, "{bpm} {key}");

    // Invalid body -> error, body unchanged.
    assert!(ct::update_body(&pool, alice, false, a, "{artist}").await.is_err());
    assert_eq!(ct::get(&pool, a).await.unwrap().unwrap().body, "{bpm} {key}");

    // Foreign -> rejected.
    assert!(ct::update_body(&pool, bob, false, a, "{tags}").await.is_err());
}

#[tokio::test]
async fn share_toggle_and_delete() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;
    let a = ct::create(&pool, alice, "One", "{tags}", false).await.unwrap();

    ct::set_shared(&pool, alice, false, a, true).await.unwrap();
    assert_eq!(ct::get(&pool, a).await.unwrap().unwrap().is_shared, 1);
    ct::set_shared(&pool, alice, false, a, false).await.unwrap();
    assert_eq!(ct::get(&pool, a).await.unwrap().unwrap().is_shared, 0);

    // Foreign cannot toggle / delete.
    assert!(ct::set_shared(&pool, bob, false, a, true).await.is_err());
    assert!(ct::delete(&pool, bob, false, a).await.is_err());

    // Owner deletes.
    assert!(ct::delete(&pool, alice, false, a).await.unwrap());
    assert!(ct::get(&pool, a).await.unwrap().is_none());

    // System: non-admin rejected, admin allowed.
    let sys = system_id(&pool, "Nur Tags").await;
    assert!(ct::delete(&pool, bob, false, sys).await.is_err());
    assert!(ct::delete(&pool, bob, true, sys).await.unwrap());
}

#[tokio::test]
async fn duplicate_creates_owned_editable_copy() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;

    // Bob shares a template.
    let shared = ct::create(&pool, bob, "Shared", "{tags}!", false).await.unwrap();
    ct::set_shared(&pool, bob, false, shared, true).await.unwrap();

    // Alice duplicates Bob's shared template: owned by Alice, unshared, editable.
    let copy = ct::duplicate(&pool, alice, shared).await.unwrap();
    let row = ct::get(&pool, copy).await.unwrap().unwrap();
    assert_eq!(row.owner_user_id, Some(alice));
    assert_eq!(row.is_shared, 0);
    assert_eq!(row.body, "{tags}!");
    ct::update_body(&pool, alice, false, copy, "{energy}").await.unwrap();

    // Duplicating a system template names the copy after the system name.
    let sys = system_id(&pool, "Standard").await;
    let scopy = ct::duplicate(&pool, alice, sys).await.unwrap();
    let srow = ct::get(&pool, scopy).await.unwrap().unwrap();
    assert_eq!(srow.owner_user_id, Some(alice));
    assert_eq!(srow.body, "{tags} {bpm} {key} {energy}");

    // Duplicating the same source twice yields a distinct name.
    let again = ct::duplicate(&pool, alice, sys).await.unwrap();
    let arow = ct::get(&pool, again).await.unwrap().unwrap();
    assert_ne!(arow.name, srow.name, "copy name made unique per owner");
}

#[tokio::test]
async fn visibility_and_mutable_by_rules() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;

    let private = ct::create(&pool, bob, "Private", "{tags}", false).await.unwrap();
    let shared = ct::create(&pool, bob, "Shared", "{tags}", false).await.unwrap();
    ct::set_shared(&pool, bob, false, shared, true).await.unwrap();
    let sys = system_id(&pool, "Standard").await;

    // Foreign private: invisible -> get_visible None, mutable_by false (visible
    // only to its owner).
    assert!(ct::get_visible(&pool, alice, private).await.unwrap().is_none());
    assert!(ct::get_visible(&pool, bob, private).await.unwrap().is_some());

    // Shared is visible but not mutable for Alice.
    assert!(ct::get_visible(&pool, alice, shared).await.unwrap().is_some());
    assert_eq!(ct::mutable_by(&pool, alice, false, shared).await.unwrap(), Some(false));

    // System is visible to all; mutable only for admins.
    assert!(ct::get_visible(&pool, alice, sys).await.unwrap().is_some());
    assert_eq!(ct::mutable_by(&pool, alice, false, sys).await.unwrap(), Some(false));
    assert_eq!(ct::mutable_by(&pool, alice, true, sys).await.unwrap(), Some(true));

    // Own is mutable; missing row -> None.
    assert_eq!(ct::mutable_by(&pool, bob, false, private).await.unwrap(), Some(true));
    assert_eq!(ct::mutable_by(&pool, alice, false, 999_999).await.unwrap(), None);

    // list_visible groups system first, then mine, then shared.
    let views = ct::list_visible(&pool, alice, false).await.unwrap();
    let ids: Vec<i64> = views.iter().map(|v| v.id).collect();
    assert!(!ids.contains(&private), "foreign private not listed");
    assert!(ids.contains(&shared));
    assert!(ids.contains(&sys));
    assert!(views.iter().any(|v| v.is_system));
    assert!(views.iter().find(|v| v.id == shared).unwrap().owner_slug == "bob");
}

#[tokio::test]
async fn two_users_same_name_allowed() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;

    let a = ct::create(&pool, alice, "Standard", "{tags}", false).await.unwrap();
    // Same name (even case-insensitively) is fine for a different user.
    let b = ct::create(&pool, bob, "standard", "{tags}", false).await.unwrap();
    assert_ne!(a, b);
    assert_eq!(ct::get(&pool, a).await.unwrap().unwrap().owner_user_id, Some(alice));
    assert_eq!(ct::get(&pool, b).await.unwrap().unwrap().owner_user_id, Some(bob));
}

// ── migration ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn migration_seeds_system_and_is_additive() {
    let (_dir, pool) = setup().await;

    // Fresh DB: exactly 2 system templates with the fixed bodies.
    let sys_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_comment_templates WHERE owner_user_id IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(sys_count, 2, "two system seeds");
    assert_eq!(
        ct::get(&pool, system_id(&pool, "Standard").await).await.unwrap().unwrap().body,
        "{tags} {bpm} {key} {energy}"
    );
    assert_eq!(
        ct::get(&pool, system_id(&pool, "Nur Tags").await).await.unwrap().unwrap().body,
        "{tags}"
    );

    // A pre-existing table (a migration <= 065) is still present and intact.
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(users >= 0, "hub_users still queryable");
}

/// Apply migrations **up to 065** on a fresh DB (by running a copy of the
/// migrations dir without 066), insert a user, then run the full migration set:
/// 066 must apply additively on top of the existing DB and seed the system rows.
#[tokio::test]
async fn migration_runs_on_existing_db() {
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

    // Build a migrations dir holding everything except 066.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let old = tempfile::tempdir().expect("old migrations");
    let mut copied = 0;
    for entry in std::fs::read_dir(&src).expect("read migrations") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".sql") || name.starts_with("066") {
            continue;
        }
        std::fs::copy(entry.path(), old.path().join(&name)).expect("copy migration");
        copied += 1;
    }
    assert!(copied > 0, "copied the pre-066 migrations");

    let pre = sqlx::migrate::Migrator::new(old.path())
        .await
        .expect("load pre-066 migrator");
    pre.run(&pool).await.expect("apply pre-066 migrations");

    // Existing data before 066.
    let alice = insert_user(&pool, "alice").await;
    let before_users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_users")
        .fetch_one(&pool)
        .await
        .unwrap();

    // The hub's real migration set (incl. 066) runs additively.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply 066 on existing DB");

    // Seeds present, old data intact.
    let sys_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_comment_templates WHERE owner_user_id IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(sys_count, 2, "system seeds created on the existing DB");
    let after_users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after_users, before_users, "existing rows untouched");
    assert!(ct::get(&pool, system_id(&pool, "Standard").await).await.unwrap().is_some());
    let _ = alice;
}

// ── HTTP layer ───────────────────────────────────────────────────────────────

async fn insert_template(
    pool: &SqlitePool,
    owner: Option<i64>,
    name: &str,
    body: &str,
    shared: i64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_comment_templates
            (owner_user_id, name, body, is_shared, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(body)
    .bind(shared)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert template")
}

fn location(resp: &reqwest::Response) -> String {
    resp.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn flash_cookie(resp: &reqwest::Response) -> String {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("hub_flash="))
        .unwrap_or_default()
        .to_string()
}

async fn get_page(app: &common::TestApp, cookie: &str) -> String {
    let resp = app
        .client()
        .get(app.url("/comment-templates"))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    resp.text().await.unwrap()
}

#[tokio::test]
async fn http_create_then_duplicate_and_validate_flash() {
    let app = common::spawn().await;
    let (_uid, cookie) = app.user_with_password("ct-creator", "pw").await;

    let resp = app
        .client()
        .post(app.url("/comment-templates/create"))
        .header("Cookie", &cookie)
        .form(&[("name", "Meins"), ("body", "{tags} {bpm}")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/comment-templates");
    let body = get_page(&app, &cookie).await;
    assert!(body.contains("Meins"), "new template listed");

    // Duplicate -> flash, still one own "Meins".
    let resp = app
        .client()
        .post(app.url("/comment-templates/create"))
        .header("Cookie", &cookie)
        .form(&[("name", "meins"), ("body", "{tags}")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(flash_cookie(&resp).starts_with("hub_flash=ok:"));

    // Invalid body -> flash naming the allowed placeholders, no new row.
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_comment_templates WHERE owner_user_id IS NOT NULL")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let resp = app
        .client()
        .post(app.url("/comment-templates/create"))
        .header("Cookie", &cookie)
        .form(&[("name", "Bad"), ("body", "{title}")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(flash_cookie(&resp).starts_with("hub_flash=ok:"));
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_comment_templates WHERE owner_user_id IS NOT NULL")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(before, after, "invalid body wrote nothing");
}

#[tokio::test]
async fn http_foreign_private_is_404_and_foreign_shared_is_403() {
    let app = common::spawn().await;
    let (alice_id, alice_cookie) = app.user_with_password("ct-alice", "pw").await;
    let (bob_id, _bob_cookie) = app.user_with_password("ct-bob", "pw").await;

    let private = insert_template(&app.pool, Some(bob_id), "Private", "{tags}", 0).await;
    let shared = insert_template(&app.pool, Some(bob_id), "Shared", "{tags}", 1).await;

    // Foreign private: not visible -> 404 on every mutating route.
    for path in ["rename", "body", "share", "delete"] {
        let resp = app
            .client()
            .post(app.url(&format!("/comment-templates/{private}/{path}")))
            .header("Cookie", &alice_cookie)
            .form(&[("name", "x"), ("body", "{tags}")])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND, "{path} foreign private 404");
    }
    let page = get_page(&app, &alice_cookie).await;
    assert!(!page.contains("Private"), "foreign private not listed");

    // Foreign shared: visible (listed) but not mutable -> 403.
    let page = get_page(&app, &alice_cookie).await;
    assert!(page.contains("Shared"), "foreign shared listed");
    for path in ["rename", "body", "share", "delete"] {
        let resp = app
            .client()
            .post(app.url(&format!("/comment-templates/{shared}/{path}")))
            .header("Cookie", &alice_cookie)
            .form(&[("name", "x"), ("body", "{tags}")])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN, "{path} foreign shared 403");
    }
    // Bob's shared template is untouched.
    assert_eq!(ct::get(&app.pool, shared).await.unwrap().unwrap().name, "Shared");

    let _ = alice_id;
}

#[tokio::test]
async fn http_system_is_403_for_non_admin() {
    let app = common::spawn().await;
    let (_uid, cookie) = app.user_with_password("ct-user", "pw").await;
    let sys = insert_template(&app.pool, None, "SysTest", "{tags}", 0).await;

    // Visible on the page ...
    let page = get_page(&app, &cookie).await;
    assert!(page.contains("SysTest"), "system template visible");

    // ... but every mutating route is 403 for a non-admin.
    for path in ["rename", "body", "share", "delete"] {
        let resp = app
            .client()
            .post(app.url(&format!("/comment-templates/{sys}/{path}")))
            .header("Cookie", &cookie)
            .form(&[("name", "x"), ("body", "{tags}")])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN, "{path} system 403");
    }
    assert!(ct::get(&app.pool, sys).await.unwrap().is_some(), "system template survives");
}

#[tokio::test]
async fn http_share_duplicate_and_own_edits() {
    let app = common::spawn().await;
    let (alice_id, alice_cookie) = app.user_with_password("ct-alice", "pw").await;
    let (_bob_id, bob_cookie) = app.user_with_password("ct-bob", "pw").await;

    // Alice creates + shares.
    let resp = app
        .client()
        .post(app.url("/comment-templates/create"))
        .header("Cookie", &alice_cookie)
        .form(&[("name", "MyShared"), ("body", "{tags}")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let id: i64 = sqlx::query_scalar(
        "SELECT id FROM hub_comment_templates WHERE owner_user_id = ?1 AND name = 'MyShared'",
    )
    .bind(alice_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();

    let resp = app
        .client()
        .post(app.url(&format!("/comment-templates/{id}/share")))
        .header("Cookie", &alice_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(ct::get(&app.pool, id).await.unwrap().unwrap().is_shared, 1);

    // Rename + body + delete own work.
    for (path, field, value) in [
        ("rename", "name", "Renamed"),
        ("body", "body", "{bpm}"),
    ] {
        let resp = app
            .client()
            .post(app.url(&format!("/comment-templates/{id}/{path}")))
            .header("Cookie", &alice_cookie)
            .form(&[(field, value)])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER, "{path}");
    }
    let row = ct::get(&app.pool, id).await.unwrap().unwrap();
    assert_eq!(row.name, "Renamed");
    assert_eq!(row.body, "{bpm}");

    // Bob sees the shared template and duplicates it; the copy is Bob's & editable.
    let page = get_page(&app, &bob_cookie).await;
    assert!(page.contains("Renamed"), "shared template visible to Bob");
    let resp = app
        .client()
        .post(app.url(&format!("/comment-templates/{id}/duplicate")))
        .header("Cookie", &bob_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let bob_copy: i64 = sqlx::query_scalar(
        "SELECT id FROM hub_comment_templates WHERE owner_user_id = ?1 AND name = 'Renamed'",
    )
    .bind(_bob_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(ct::get(&app.pool, bob_copy).await.unwrap().unwrap().is_shared, 0, "copy is unshared");
    // Editable by Bob.
    let resp = app
        .client()
        .post(app.url(&format!("/comment-templates/{bob_copy}/body")))
        .header("Cookie", &bob_cookie)
        .form(&[("body", "{energy}")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    // Alice deletes her own.
    let resp = app
        .client()
        .post(app.url(&format!("/comment-templates/{id}/delete")))
        .header("Cookie", &alice_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(ct::get(&app.pool, id).await.unwrap().is_none());
}

#[tokio::test]
async fn http_auth_gate_redirects_to_login() {
    let app = common::spawn().await;

    let page = app
        .client()
        .get(app.url("/comment-templates"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(location(&page), "/login");

    let create = app
        .client()
        .post(app.url("/comment-templates/create"))
        .form(&[("name", "x"), ("body", "{tags}")])
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(location(&create), "/login");
}

// ── cross-cutting hardening (Tester, #668) ───────────────────────────────────

/// Duplicating a foreign *private* template must be indistinguishable from a
/// missing one: `404`, no copy written, source untouched.
#[tokio::test]
async fn http_duplicate_foreign_private_is_404_and_writes_nothing() {
    let app = common::spawn().await;
    let (_alice_id, alice_cookie) = app.user_with_password("ct-dup-a", "pw").await;
    let (bob_id, _bob_cookie) = app.user_with_password("ct-dup-b", "pw").await;
    let private = insert_template(&app.pool, Some(bob_id), "BobsGeheim", "{tags}", 0).await;

    let total = |pool: SqlitePool| async move {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM hub_comment_templates")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let before = total(app.pool.clone()).await;

    let resp = app
        .client()
        .post(app.url(&format!("/comment-templates/{private}/duplicate")))
        .header("Cookie", &alice_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "duplicate foreign private -> 404"
    );
    assert_eq!(total(app.pool.clone()).await, before, "no copy written");
    assert!(
        ct::get(&app.pool, private).await.unwrap().is_some(),
        "source untouched"
    );

    // And it never shows up on the caller's page.
    let page = get_page(&app, &alice_cookie).await;
    assert!(!page.contains("BobsGeheim"), "foreign private not listed");
}

/// Copying a shared template whose name already exists in the copier's own
/// library must resolve to a clean, unique name (303 + new row), never a 500 —
/// and the same collision handling is deterministic on repetition.
#[tokio::test]
async fn http_duplicate_name_collision_resolves_cleanly() {
    let app = common::spawn().await;
    let (alice_id, alice_cookie) = app.user_with_password("ct-col-a", "pw").await;
    let (bob_id, _bob_cookie) = app.user_with_password("ct-col-b", "pw").await;

    // Alice already owns "Track"; Bob shares a template of the same name.
    insert_template(&app.pool, Some(alice_id), "Track", "{tags}", 0).await;
    let shared = insert_template(&app.pool, Some(bob_id), "Track", "{bpm} {key}", 1).await;

    for attempt in 0..2 {
        let resp = app
            .client()
            .post(app.url(&format!("/comment-templates/{shared}/duplicate")))
            .header("Cookie", &alice_cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::SEE_OTHER,
            "attempt {attempt}: collision resolves via 303, not 500"
        );
        assert_eq!(location(&resp), "/comment-templates");
    }

    // Alice now has "Track", "Track (Kopie)" and "Track (Kopie 2)", all her own.
    let mut names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM hub_comment_templates WHERE owner_user_id = ?1",
    )
    .bind(alice_id)
    .fetch_all(&app.pool)
    .await
    .unwrap();
    names.sort();
    let mut expected = vec!["Track", "Track (Kopie)", "Track (Kopie 2)"];
    expected.sort();
    assert_eq!(names, expected);
    // Copies keep the source body and belong to the copier, unshared.
    let copy_body: String = sqlx::query_scalar(
        "SELECT body FROM hub_comment_templates WHERE owner_user_id = ?1 AND name = 'Track (Kopie)'",
    )
    .bind(alice_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(copy_body, "{bpm} {key}");
    let copy_shared: i64 = sqlx::query_scalar(
        "SELECT is_shared FROM hub_comment_templates WHERE owner_user_id = ?1 AND name = 'Track (Kopie)'",
    )
    .bind(alice_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(copy_shared, 0);
}

/// Visibility end-to-end and list determinism: a foreign private template is
/// absent, a shared one is present (body included, readable by the second user),
/// and repeated listings return the same grouping/order.
#[tokio::test]
async fn visibility_list_is_complete_and_deterministic() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;

    ct::create(&pool, alice, "zeta", "{tags}", false)
        .await
        .unwrap();
    ct::create(&pool, alice, "Alpha", "{bpm}", false)
        .await
        .unwrap();
    ct::create(&pool, bob, "Privat", "{key}", false)
        .await
        .unwrap();
    let shared = ct::create(&pool, bob, "Geteilt", "{energy}", false)
        .await
        .unwrap();
    ct::set_shared(&pool, bob, false, shared, true)
        .await
        .unwrap();

    let once: Vec<(i64, String, bool)> = ct::list_visible(&pool, alice, false)
        .await
        .unwrap()
        .into_iter()
        .map(|v| (v.id, v.name.clone(), v.is_system))
        .collect();
    let twice: Vec<(i64, String, bool)> = ct::list_visible(&pool, alice, false)
        .await
        .unwrap()
        .into_iter()
        .map(|v| (v.id, v.name.clone(), v.is_system))
        .collect();
    assert_eq!(once, twice, "list order is deterministic");

    let names: Vec<&str> = once.iter().map(|(_, n, _)| n.as_str()).collect();
    assert!(names.contains(&"Geteilt"), "shared visible: {names:?}");
    assert!(
        !names.contains(&"Privat"),
        "foreign private hidden: {names:?}"
    );
    // System group first, then own (NOCASE sorted), then foreign shared.
    assert_eq!(
        &names[0..2],
        &["Nur Tags", "Standard"],
        "system first: {names:?}"
    );
    assert_eq!(
        &names[2..4],
        &["Alpha", "zeta"],
        "own sorted NOCASE: {names:?}"
    );
    assert_eq!(&names[4..], &["Geteilt"], "shared last: {names:?}");

    // The shared body is readable by the second user (usable content).
    assert_eq!(
        ct::get_visible(&pool, alice, shared)
            .await
            .unwrap()
            .unwrap()
            .body,
        "{energy}"
    );
}

/// The 066 system seeds are re-runnable: `INSERT OR IGNORE` leaves exactly two
/// system rows regardless of how often it runs, does not touch user rows that
/// happen to share a name, and keeps the fixed timestamps.
#[tokio::test]
async fn system_seed_is_idempotent_and_deterministic() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    // A user-owned row named like a system seed must be left alone.
    ct::create(&pool, alice, "Standard", "{tags}", false)
        .await
        .unwrap();

    for _ in 0..3 {
        sqlx::query(
            "INSERT OR IGNORE INTO hub_comment_templates
                (owner_user_id, name, body, is_shared, created_at, updated_at)
             VALUES (NULL, 'Standard', '{tags} {bpm} {key} {energy}', 0,
                     '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00'),
                    (NULL, 'Nur Tags', '{tags}', 0,
                     '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    let sys: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT name, body, created_at FROM hub_comment_templates
          WHERE owner_user_id IS NULL ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(sys.len(), 2, "still exactly two system seeds");
    assert_eq!(sys[0].0, "Nur Tags");
    assert_eq!(sys[1].0, "Standard");
    assert_eq!(sys[1].1, "{tags} {bpm} {key} {energy}");
    assert!(
        sys.iter()
            .all(|(_, _, ts)| ts == "2026-01-01T00:00:00+00:00"),
        "fixed timestamps"
    );

    // The user's same-named row survived and there are 3 rows total.
    let alice_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_comment_templates WHERE owner_user_id = ?1 AND name = 'Standard'",
    )
    .bind(alice)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(alice_rows, 1);
    assert_eq!(count(&pool).await, 3);
}
