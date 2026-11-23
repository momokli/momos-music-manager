//! Integration tests for the per-device Kellner settings API + page (issue #369):
//! web-session gating, defaults, upsert/idempotency, format + template
//! validation, active-backpack ownership (400, no partial write), owner scoping
//! (foreign/unknown → 404), the "no token hash ever leaks" guarantee, cascade
//! cleanup and the HTML `/kellner` page + form POST.

mod common;

use music_hub::devices::hash_token;
use reqwest::StatusCode;
use serde_json::{Value, json};

const TS: &str = "2026-01-01T00:00:00+00:00";

/// Insert a **bound** device owned by `user_id`; returns the device id.
async fn bound_device(app: &common::TestApp, user_id: i64, name: &str, raw_token: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_devices (user_id, name, token_hash, created_at, paired_at)
         VALUES (?1, ?2, ?3, ?4, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(hash_token(raw_token))
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .expect("insert bound device")
}

/// Insert an unbound (pending) device; returns the device id.
async fn pending_device(app: &common::TestApp, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_devices (user_id, name, token_hash, created_at)
         VALUES (NULL, ?1, ?2, ?3) RETURNING id",
    )
    .bind(name)
    .bind("b".repeat(64))
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .expect("insert pending device")
}

/// Insert a backpack owned by `user_id`; returns its id.
async fn backpack(app: &common::TestApp, user_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, ?2, 0, ?3, ?3) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .expect("insert backpack")
}

/// Insert a comment template; returns its id. `owner` = `None` is a system row.
async fn template(
    app: &common::TestApp,
    owner: Option<i64>,
    name: &str,
    body: &str,
    shared: bool,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_comment_templates
            (owner_user_id, name, body, is_shared, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(body)
    .bind(i64::from(shared))
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .expect("insert template")
}

/// Force a settings row's `comment_template_id` directly (bypasses visibility).
async fn set_ref(app: &common::TestApp, device_id: i64, id: Option<i64>) {
    sqlx::query(
        "INSERT INTO hub_device_settings
            (device_id, preferred_format, comment_template, comment_template_id, updated_at)
         VALUES (?1, 'flac', '', ?2, ?3)
         ON CONFLICT(device_id) DO UPDATE SET comment_template_id = excluded.comment_template_id",
    )
    .bind(device_id)
    .bind(id)
    .bind(TS)
    .execute(&app.pool)
    .await
    .expect("force ref");
}

async fn json_body(resp: reqwest::Response) -> (StatusCode, Value) {
    let status = resp.status();
    let txt = resp.text().await.expect("body");
    let v = serde_json::from_str(&txt).unwrap_or(Value::Null);
    (status, v)
}

async fn get(app: &common::TestApp, cookie: Option<&str>, id: i64) -> (StatusCode, Value) {
    let mut req = app
        .client()
        .get(app.url(&format!("/api/hub/devices/{id}/settings")));
    if let Some(c) = cookie {
        req = req.header("Cookie", c);
    }
    json_body(req.send().await.expect("GET")).await
}

async fn put(app: &common::TestApp, cookie: &str, id: i64, body: Value) -> (StatusCode, Value) {
    json_body(
        app.client()
            .put(app.url(&format!("/api/hub/devices/{id}/settings")))
            .header("Cookie", cookie)
            .json(&body)
            .send()
            .await
            .expect("PUT"),
    )
    .await
}

async fn active_ids(app: &common::TestApp, device_id: i64) -> Vec<i64> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT backpack_id FROM hub_device_backpacks WHERE device_id = ?1 ORDER BY position",
    )
    .bind(device_id)
    .fetch_all(&app.pool)
    .await
    .expect("load links");
    rows.into_iter().map(|(x,)| x).collect()
}

// (a) no session → redirect to /login ───────────────────────────────────────

#[tokio::test]
async fn no_session_redirects_to_login() {
    let app = common::spawn().await;
    let (uid, _c) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let (status, _) = get(&app, None, did).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "no session → redirect");

    let resp = app
        .client()
        .get(app.url(&format!("/api/hub/devices/{did}/settings")))
        .send()
        .await
        .expect("GET");
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(loc.contains("/login"), "redirect location: {loc}");
}

// (b) own device without a row → defaults ───────────────────────────────────

#[tokio::test]
async fn missing_row_yields_defaults() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let (status, v) = get(&app, Some(&cookie), did).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["device_id"], did);
    assert_eq!(v["preferred_format"], "flac");
    assert_eq!(v["comment_template"], "");
    assert_eq!(v["active_backpack_ids"], json!([]));
    assert!(v["updated_at"].is_null(), "no row → updated_at null");
}

// (c) PUT creates, GET reflects, second PUT updates (idempotent upsert) ──────

#[tokio::test]
async fn put_creates_then_updates() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;
    let bp = backpack(&app, uid, "bp-a").await;

    let (status, v) = put(
        &app,
        &cookie,
        did,
        json!({ "preferred_format": "mp3", "comment_template": " hi ", "active_backpack_ids": [bp] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["preferred_format"], "mp3");
    assert_eq!(v["comment_template"], "hi", "template is trimmed");
    assert_eq!(v["active_backpack_ids"], json!([bp]));

    let (status, v) = get(&app, Some(&cookie), did).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["preferred_format"], "mp3");
    assert_eq!(v["comment_template"], "hi");
    assert_eq!(v["active_backpack_ids"], json!([bp]));

    // Second PUT changes only the format; absent fields stay unchanged.
    let (status, v) = put(&app, &cookie, did, json!({ "preferred_format": "wav" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["preferred_format"], "wav");
    assert_eq!(v["comment_template"], "hi", "unchanged");
    assert_eq!(v["active_backpack_ids"], json!([bp]), "unchanged");
}

// (d) foreign backpack → 400, nothing written ───────────────────────────────

#[tokio::test]
async fn foreign_backpack_is_400_without_partial_write() {
    let app = common::spawn().await;
    let (alice, alice_cookie) = app.user_with_password("ds-alice", "1234").await;
    let (bob, _bob_cookie) = app.user_with_password("ds-bob", "1234").await;
    let did = bound_device(&app, alice, "kellner-1", &"a".repeat(64)).await;
    let bob_bp = backpack(&app, bob, "bob-bp").await;

    let (status, v) = put(
        &app,
        &alice_cookie,
        did,
        json!({ "comment_template": "should-not-persist", "active_backpack_ids": [bob_bp] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");

    // Neither the template nor the link were written (single transaction/validate).
    let (_, v) = get(&app, Some(&alice_cookie), did).await;
    assert_eq!(v["comment_template"], "", "no partial write");
    assert_eq!(v["active_backpack_ids"], json!([]));
    assert!(active_ids(&app, did).await.is_empty());
}

// (e) removing one backpack from the set removes only that link ─────────────

#[tokio::test]
async fn removing_backpack_prunes_only_that_link() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;
    let bp1 = backpack(&app, uid, "bp-1").await;
    let bp2 = backpack(&app, uid, "bp-2").await;

    let (s, _) = put(
        &app,
        &cookie,
        did,
        json!({ "active_backpack_ids": [bp1, bp2] }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, v) = put(&app, &cookie, did, json!({ "active_backpack_ids": [bp2] })).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["active_backpack_ids"], json!([bp2]));
    assert_eq!(active_ids(&app, did).await, vec![bp2]);
}

// (f) invalid format → 400 ──────────────────────────────────────────────────

#[tokio::test]
async fn invalid_format_is_400() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let (status, v) = put(&app, &cookie, did, json!({ "preferred_format": "ogg" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].is_string());
}

// (f2) over-long template → 400 ─────────────────────────────────────────────

#[tokio::test]
async fn over_long_template_is_400() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let long = "a".repeat(1025);
    let (status, v) = put(&app, &cookie, did, json!({ "comment_template": long })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
}

// (f2b) unknown placeholder → 400 listing the allowed ones ──────────────────

#[tokio::test]
async fn unknown_placeholder_is_400() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let (status, v) = put(
        &app,
        &cookie,
        did,
        json!({ "comment_template": "{title} {tags}" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
    let err = v["error"].as_str().unwrap_or_default();
    assert!(err.contains("{title}"), "names the token: {err}");
    for allowed in ["{tags}", "{bpm}", "{key}", "{energy}"] {
        assert!(err.contains(allowed), "lists {allowed}: {err}");
    }
}

// (f2c) valid placeholders + `{{` edge cases → 200 ─────────────────────────

#[tokio::test]
async fn valid_placeholders_and_brace_edge_cases_are_200() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    for tpl in [
        "{tags} {bpm} {key} {energy}",
        "{{tags}}",
        "pre: {key}",
        "{{ literal",
    ] {
        let (status, v) = put(&app, &cookie, did, json!({ "comment_template": tpl })).await;
        assert_eq!(status, StatusCode::OK, "{tpl:?} rejected: {v}");
    }

    // Empty template is accepted and reads back as empty (default resolved on export).
    let (status, v) = put(&app, &cookie, did, json!({ "comment_template": "" })).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["comment_template"], "");
}

// (f3) empty body {} → 200, nothing changed (all fields absent = unchanged) ───

#[tokio::test]
async fn empty_body_changes_nothing() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;
    let bp = backpack(&app, uid, "bp-a").await;

    let (s, _) = put(
        &app,
        &cookie,
        did,
        json!({ "preferred_format": "mp3", "comment_template": "keep", "active_backpack_ids": [bp] }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // PUT {} is a no-op: 200 and every field preserved.
    let (status, v) = put(&app, &cookie, did, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["preferred_format"], "mp3");
    assert_eq!(v["comment_template"], "keep");
    assert_eq!(v["active_backpack_ids"], json!([bp]));

    // And it really persisted (GET reflects the same state).
    let (status, v) = get(&app, Some(&cookie), did).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["preferred_format"], "mp3");
    assert_eq!(v["comment_template"], "keep");
    assert_eq!(v["active_backpack_ids"], json!([bp]));
}

// (g) owner scope: foreign device → 404 on GET and PUT; unknown → 404 ────────

#[tokio::test]
async fn owner_scoping_hides_foreign_and_unknown() {
    let app = common::spawn().await;
    let (alice, alice_cookie) = app.user_with_password("ds-alice", "1234").await;
    let (bob, bob_cookie) = app.user_with_password("ds-bob", "1234").await;
    let alice_dev = bound_device(&app, alice, "alice-kellner", &"a".repeat(64)).await;
    let bob_dev = bound_device(&app, bob, "bob-kellner", &"c".repeat(64)).await;

    // Bob reading Alice's device → 404.
    let (status, v) = get(&app, Some(&bob_cookie), alice_dev).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{v}");
    // Bob PUTting Alice's device → 404.
    let (status, _) = put(
        &app,
        &bob_cookie,
        alice_dev,
        json!({ "preferred_format": "mp3" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Bob reading his own device is fine (control).
    let (status, _) = get(&app, Some(&bob_cookie), bob_dev).await;
    assert_eq!(status, StatusCode::OK);
    // Alice reading an unknown id → 404 (indistinguishable from foreign).
    let (status, v) = get(&app, Some(&alice_cookie), 999_999).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{v}");
    // Unbound (pending) device of Alice: not owned yet → 404.
    let pending = pending_device(&app, "pending").await;
    let (status, _) = get(&app, Some(&alice_cookie), pending).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// (h) no token hash ever leaks ──────────────────────────────────────────────

#[tokio::test]
async fn responses_never_leak_token_hash() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let raw = "d".repeat(64);
    let did = bound_device(&app, uid, "kellner-1", &raw).await;
    let bp = backpack(&app, uid, "bp-a").await;

    let (_, v) = put(&app, &cookie, did, json!({ "active_backpack_ids": [bp] })).await;
    let body = v.to_string();
    assert!(
        !body.contains(&hash_token(&raw)),
        "token hash leaked in PUT"
    );
    assert!(!body.contains(&raw), "token leaked in PUT");

    let (_, v) = get(&app, Some(&cookie), did).await;
    let body = v.to_string();
    assert!(
        !body.contains(&hash_token(&raw)),
        "token hash leaked in GET"
    );
    assert!(!body.contains("token_hash"));

    // The rendered page must not leak either.
    let resp = app
        .client()
        .get(app.url("/kellner"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("page");
    assert_eq!(resp.status(), StatusCode::OK);
    let html = resp.text().await.expect("html");
    assert!(
        !html.contains(&hash_token(&raw)),
        "token hash leaked in page"
    );
    assert!(!html.contains(&raw));
}

// (i) deleting the device cascades settings away ────────────────────────────

#[tokio::test]
async fn device_delete_cascades_settings() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;
    let bp = backpack(&app, uid, "bp-a").await;

    let (s, _) = put(
        &app,
        &cookie,
        did,
        json!({ "preferred_format": "m4a", "active_backpack_ids": [bp] }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    sqlx::query("DELETE FROM hub_devices WHERE id = ?1")
        .bind(did)
        .execute(&app.pool)
        .await
        .expect("delete device");

    let settings: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_device_settings WHERE device_id = ?1")
            .bind(did)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    let links: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_device_backpacks WHERE device_id = ?1")
            .bind(did)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(settings, 0);
    assert_eq!(links, 0);
}

// ── HTML page + form POST ───────────────────────────────────────────────────

#[tokio::test]
async fn kellner_page_lists_devices_and_form_saves() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;
    let bp = backpack(&app, uid, "bp-a").await;

    let resp = app
        .client()
        .get(app.url("/kellner"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("page");
    assert_eq!(resp.status(), StatusCode::OK);
    let html = resp.text().await.expect("html");
    assert!(html.contains("kellner-1"), "device listed");

    // Submit the plain form (repeated active_backpack_ids key).
    let form =
        format!("preferred_format=wav&comment_template=hello+world&active_backpack_ids={bp}");
    let resp = app
        .client()
        .post(app.url(&format!("/kellner/{did}/settings")))
        .header("Cookie", &cookie)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .expect("form post");
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let (_, v) = get(&app, Some(&cookie), did).await;
    assert_eq!(v["preferred_format"], "wav");
    assert_eq!(v["comment_template"], "hello world");
    assert_eq!(v["active_backpack_ids"], json!([bp]));
}

#[tokio::test]
async fn kellner_page_requires_session() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/kellner"))
        .send()
        .await
        .expect("page");
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(loc.contains("/login"), "redirect location: {loc}");
}

#[tokio::test]
async fn kellner_form_rejects_invalid_format() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let resp = app
        .client()
        .post(app.url(&format!("/kellner/{did}/settings")))
        .header("Cookie", &cookie)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("preferred_format=ogg")
        .send()
        .await
        .expect("form post");
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(loc, "/kellner", "flash redirect: {loc}");
    let flash = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("hub_flash="))
        .unwrap_or_default();
    assert!(flash.starts_with("hub_flash="), "flash cookie set: {loc}");

    let (_, v) = get(&app, Some(&cookie), did).await;
    assert_eq!(v["preferred_format"], "flac", "nothing written");
}

// ── #669: template reference resolution, regression, visibility ─────────────

/// The device's effective comment resolves through the single API: a set,
/// visible template body wins over the legacy free text.
#[tokio::test]
async fn resolution_prefers_template_body() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;
    let tid = template(&app, Some(uid), "My", "{key} / {bpm}", false).await;

    let (status, v) = put(
        &app,
        &cookie,
        did,
        json!({ "comment_template_id": tid, "comment_template": "{tags}" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["comment_template_id"], tid);

    let resolved = music_hub::device_settings::resolve_effective_comment(&app.pool, did, uid).await;
    assert_eq!(resolved, "{key} / {bpm}", "template body wins");

    let (body, name) =
        music_hub::device_settings::resolve_effective_comment_named(&app.pool, did, uid).await;
    assert_eq!(body, "{key} / {bpm}");
    assert_eq!(name.as_deref(), Some("My"));
}

/// A vanished (deleted) reference falls back to the legacy free text; the same
/// happens for a reference the device owner may no longer see.
#[tokio::test]
async fn resolution_falls_back_to_legacy_when_template_gone_or_invisible() {
    let app = common::spawn().await;
    let (alice, alice_cookie) = app.user_with_password("ds-alice", "1234").await;
    let (bob, _bob_cookie) = app.user_with_password("ds-bob", "1234").await;
    let did = bound_device(&app, alice, "kellner-1", &"a".repeat(64)).await;

    // (1) A template the owner set, then deleted: ON DELETE SET NULL clears the
    //     reference and the legacy free text takes over.
    let tid = template(&app, Some(alice), "Temp", "{key}", false).await;
    let (status, _) = put(
        &app,
        &alice_cookie,
        did,
        json!({ "comment_template_id": tid, "comment_template": "legacy {tags}" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        music_hub::device_settings::resolve_effective_comment(&app.pool, did, alice).await,
        "{key}"
    );
    sqlx::query("DELETE FROM hub_comment_templates WHERE id = ?1")
        .bind(tid)
        .execute(&app.pool)
        .await
        .expect("delete template");
    assert_eq!(
        music_hub::device_settings::resolve_effective_comment(&app.pool, did, alice).await,
        "legacy {tags}",
        "deleted reference falls back to legacy"
    );

    // (2) A foreign private template id stored directly is invisible -> legacy.
    let bob_private = template(&app, Some(bob), "bob-priv", "{energy}", false).await;
    set_ref(&app, did, Some(bob_private)).await;
    assert_eq!(
        music_hub::device_settings::resolve_effective_comment(&app.pool, did, alice).await,
        "legacy {tags}",
        "invisible (foreign private) reference falls back to legacy"
    );
}

/// Both stages empty resolves to the default template.
#[tokio::test]
async fn resolution_empty_falls_back_to_default() {
    let app = common::spawn().await;
    let (uid, _cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let resolved = music_hub::device_settings::resolve_effective_comment(&app.pool, did, uid).await;
    assert_eq!(resolved, music_hub::device_settings::DEFAULT_TEMPLATE);
}

/// Regression (v0.33.0): a device with only the legacy free text (no reference)
/// behaves unchanged; the stored value is never rewritten.
#[tokio::test]
async fn legacy_free_text_without_reference_unchanged() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let (status, v) = put(&app, &cookie, did, json!({ "comment_template": "{tags}" })).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(v["comment_template_id"].is_null(), "no reference");

    let stored: (Option<i64>, String) = sqlx::query_as(
        "SELECT comment_template_id, comment_template FROM hub_device_settings WHERE device_id = ?1",
    )
    .bind(did)
    .fetch_one(&app.pool)
    .await
    .expect("row");
    assert_eq!(stored, (None, "{tags}".to_string()), "value not rewritten");
    assert_eq!(
        music_hub::device_settings::resolve_effective_comment(&app.pool, did, uid).await,
        "{tags}"
    );
}

/// Visibility: a foreign **shared** template is selectable (`200`), a foreign
/// **private** one is rejected (`400`) via both the JSON PUT and the form POST.
#[tokio::test]
async fn shared_template_selectable_private_rejected() {
    let app = common::spawn().await;
    let (alice, alice_cookie) = app.user_with_password("ds-alice", "1234").await;
    let (bob, _bob_cookie) = app.user_with_password("ds-bob", "1234").await;
    let did = bound_device(&app, alice, "kellner-1", &"a".repeat(64)).await;

    let shared = template(&app, Some(bob), "bob-shared", "{tags}", true).await;
    let private = template(&app, Some(bob), "bob-private", "{tags}", false).await;

    // Shared by another user -> selectable.
    let (status, v) = put(
        &app,
        &alice_cookie,
        did,
        json!({ "comment_template_id": shared }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["comment_template_id"], shared);

    // Foreign private -> 400, unchanged.
    let (status, v) = put(
        &app,
        &alice_cookie,
        did,
        json!({ "comment_template_id": private }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
    let (_, v) = get(&app, Some(&alice_cookie), did).await;
    assert_eq!(
        v["comment_template_id"], shared,
        "rejected write leaves the ref"
    );

    // Unknown id -> 400 too.
    let (status, _) = put(
        &app,
        &alice_cookie,
        did,
        json!({ "comment_template_id": 999_999 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Form POST with the foreign private template -> flash error, no write.
    let resp = app
        .client()
        .post(app.url(&format!("/kellner/{did}/settings")))
        .header("Cookie", &alice_cookie)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "preferred_format=wav&comment_template_id={private}"
        ))
        .send()
        .await
        .expect("form post");
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let (_, v) = get(&app, Some(&alice_cookie), did).await;
    assert_eq!(
        v["comment_template_id"], shared,
        "form reject leaves the ref"
    );
    assert_eq!(v["preferred_format"], "flac", "nothing written");
}

/// The form posts the chosen template id and the page renders the select with
/// the selection + a resolution preview.
#[tokio::test]
async fn kellner_form_saves_template_and_page_previews() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;
    let tid = template(&app, Some(uid), "Mix-Key", "{key} / {bpm}", false).await;

    let resp = app
        .client()
        .post(app.url(&format!("/kellner/{did}/settings")))
        .header("Cookie", &cookie)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("preferred_format=flac&comment_template_id={tid}"))
        .send()
        .await
        .expect("form post");
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let (_, v) = get(&app, Some(&cookie), did).await;
    assert_eq!(v["comment_template_id"], tid);

    let resp = app
        .client()
        .get(app.url("/kellner"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("page");
    assert_eq!(resp.status(), StatusCode::OK);
    let html = resp.text().await.expect("html");
    assert!(
        html.contains("name=\"comment_template_id\""),
        "select rendered"
    );
    assert!(html.contains("Mix-Key"), "option rendered");
    assert!(
        html.contains("Vorschau: {key} / {bpm}"),
        "preview line: {html}"
    );
}

/// #669: an explicit clear drops the reference; the resolution chain then falls
/// back to the legacy free text and, when that is blank too, to the default.
#[tokio::test]
async fn resolution_after_clear_falls_back_to_legacy() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;
    let tid = template(&app, Some(uid), "Mix-Key", "{key} / {bpm}", false).await;

    // Set the reference and a distinct legacy free text; the template body wins.
    let (status, v) = put(
        &app,
        &cookie,
        did,
        json!({ "comment_template_id": tid, "comment_template": "legacy {tags}" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["comment_template_id"], tid);
    assert_eq!(
        music_hub::device_settings::resolve_effective_comment(&app.pool, did, uid).await,
        "{key} / {bpm}"
    );

    // Explicit clear via the JSON API.
    let (status, v) = put(
        &app,
        &cookie,
        did,
        json!({ "clear_comment_template": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(v["comment_template_id"].is_null(), "reference cleared: {v}");

    // The DB column is really NULL, not merely masked in the response.
    let stored: Option<i64> = sqlx::query_scalar(
        "SELECT comment_template_id FROM hub_device_settings WHERE device_id = ?1",
    )
    .bind(did)
    .fetch_one(&app.pool)
    .await
    .expect("row");
    assert_eq!(stored, None, "column NULL after clear");

    // Resolution falls back to the legacy free text (export + preview use it).
    assert_eq!(
        music_hub::device_settings::resolve_effective_comment(&app.pool, did, uid).await,
        "legacy {tags}",
        "legacy free text after clear"
    );

    // And with a blank legacy free text it falls through to the default.
    let (status, _) = put(&app, &cookie, did, json!({ "comment_template": "" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        music_hub::device_settings::resolve_effective_comment(&app.pool, did, uid).await,
        music_hub::device_settings::DEFAULT_TEMPLATE,
        "default when both are empty"
    );
}

/// #669: the form's `-- keine --` option posts `comment_template_id=0`, which
/// clears the stored reference (previously it kept the COALESCE value).
#[tokio::test]
async fn kellner_form_clears_reference_with_zero_option() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;
    let tid = template(&app, Some(uid), "Mix-Key", "{key} / {bpm}", false).await;

    // First select the template via the form.
    let resp = app
        .client()
        .post(app.url(&format!("/kellner/{did}/settings")))
        .header("Cookie", &cookie)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("preferred_format=flac&comment_template_id={tid}"))
        .send()
        .await
        .expect("form post");
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let (_, v) = get(&app, Some(&cookie), did).await;
    assert_eq!(v["comment_template_id"], tid);

    // Now post the "none" option (value="0") -> the reference must be dropped.
    let resp = app
        .client()
        .post(app.url(&format!("/kellner/{did}/settings")))
        .header("Cookie", &cookie)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("preferred_format=flac&comment_template_id=0")
        .send()
        .await
        .expect("form post");
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/kellner"),
        "success redirect"
    );

    let (_, v) = get(&app, Some(&cookie), did).await;
    assert!(v["comment_template_id"].is_null(), "reference cleared: {v}");
    let stored: Option<i64> = sqlx::query_scalar(
        "SELECT comment_template_id FROM hub_device_settings WHERE device_id = ?1",
    )
    .bind(did)
    .fetch_one(&app.pool)
    .await
    .expect("row");
    assert_eq!(stored, None, "column NULL after form clear");
}

/// #669: sending `clear_comment_template` together with a set id is contradictory
/// and rejected with 400; nothing is written.
#[tokio::test]
async fn clear_and_id_conflict_is_400_without_write() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("ds-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;
    let tid = template(&app, Some(uid), "Mix-Key", "{key} / {bpm}", false).await;

    // Seed a reference so we can prove the rejected write left it untouched.
    let (status, _) = put(&app, &cookie, did, json!({ "comment_template_id": tid })).await;
    assert_eq!(status, StatusCode::OK);

    let (status, v) = put(
        &app,
        &cookie,
        did,
        json!({ "clear_comment_template": true, "comment_template_id": tid }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
    assert!(
        v["error"].as_str().unwrap_or_default().contains("conflict"),
        "names the conflict: {v}"
    );

    let (_, v) = get(&app, Some(&cookie), did).await;
    assert_eq!(
        v["comment_template_id"], tid,
        "rejected write leaves the ref"
    );
}
