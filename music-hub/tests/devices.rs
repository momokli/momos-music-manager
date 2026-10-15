//! Integration tests for the hub-side device registry (issue #363):
//! register/re-pair, the pairing approval page (single-use + expiry), session
//! gating and user isolation, revocation, and the "no token hash ever leaks"
//! guarantee.

mod common;

use reqwest::StatusCode;
use serde_json::{Value, json};

use music_hub::devices::hash_token;

fn raw(seed: char) -> String {
    std::iter::repeat(seed).take(64).collect()
}

async fn json_body(resp: reqwest::Response) -> (StatusCode, Value) {
    let status = resp.status();
    let txt = resp.text().await.expect("body");
    let v = serde_json::from_str(&txt).unwrap_or(Value::Null);
    (status, v)
}

/// Register a device with raw tokens; returns `(device_id, approval_path)`.
async fn register(app: &common::TestApp, name: &str, pairing: &str, device: &str) -> (i64, String) {
    let resp = app
        .client()
        .post(app.url("/api/hub/devices"))
        .json(&json!({ "name": name, "pairing_token": pairing, "device_token": device }))
        .send()
        .await
        .expect("register request");
    let (status, v) = json_body(resp).await;
    assert_eq!(status, StatusCode::OK, "register status: {v}");
    (
        v["device_id"].as_i64().expect("device_id"),
        v["approval_path"].as_str().expect("approval_path").to_string(),
    )
}

/// Insert a device row directly (for focused list/revoke/expiry tests).
async fn insert_device(
    app: &common::TestApp,
    user_id: Option<i64>,
    name: &str,
    token_hash: &str,
    expires_at: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_devices (user_id, name, token_hash, pending_token_hash, created_at, expires_at)
         VALUES (?1, ?2, ?3, NULL, ?4, ?5) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(token_hash)
    .bind("2026-01-01T00:00:00+00:00")
    .bind(expires_at)
    .fetch_one(&app.pool)
    .await
    .expect("insert device")
}

async fn row(app: &common::TestApp, id: i64) -> (Option<i64>, String, Option<String>, Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT user_id, token_hash, pending_token_hash, paired_at, expires_at
           FROM hub_devices WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(&app.pool)
    .await
    .expect("load row")
}

// (a) register → pending row, no secrets leaked ─────────────────────────────

#[tokio::test]
async fn register_creates_pending_row_and_leaks_no_secret() {
    let app = common::spawn().await;
    let pairing = raw('a');
    let device = raw('b');
    let (id, path) = register(&app, "kellner-1", &pairing, &device).await;

    assert!(
        path.contains(&format!("token={pairing}")),
        "approval path echoes the raw pairing token: {path}"
    );

    let (user_id, token_hash, pending, paired_at, expires_at) = row(&app, id).await;
    assert_eq!(user_id, None, "pending row has no owner");
    assert_eq!(token_hash, hash_token(&pairing), "token_hash holds pairing hash");
    assert_eq!(pending.as_deref(), Some(hash_token(&device).as_str()));
    assert_eq!(paired_at, None);
    assert!(expires_at.is_some(), "pending row has a TTL");

    // Re-fetch the raw body to prove no token/hash is echoed back.
    let resp = app
        .client()
        .post(app.url("/api/hub/devices"))
        .json(&json!({ "name": "kellner-2", "pairing_token": raw('c'), "device_token": raw('d') }))
        .send()
        .await
        .expect("register request");
    let txt = resp.text().await.expect("body");
    assert!(!txt.contains(&device), "device token must not be returned");
    assert!(!txt.contains(&raw('d')), "device token must not be returned");
    assert!(!txt.contains(&hash_token(&device)), "token hash must not be returned");
}

#[tokio::test]
async fn register_repairs_a_live_pending_row() {
    let app = common::spawn().await;
    let pairing = raw('a');
    let device = raw('b');
    let (id1, _) = register(&app, "kellner-1", &pairing, &device).await;
    // Second register for the same name within the TTL reuses the row.
    let pairing2 = raw('e');
    let device2 = raw('f');
    let (id2, _) = register(&app, "kellner-1", &pairing2, &device2).await;
    assert_eq!(id1, id2, "re-pair reuses the live pending row");
    let (_, token_hash, pending, _, _) = row(&app, id2).await;
    assert_eq!(token_hash, hash_token(&pairing2));
    assert_eq!(pending.as_deref(), Some(hash_token(&device2).as_str()));
}

#[tokio::test]
async fn register_rejects_malformed_tokens() {
    let app = common::spawn().await;
    for body in [
        json!({ "name": "x", "pairing_token": "short", "device_token": raw('b') }),
        json!({ "name": "x", "pairing_token": raw('a'), "device_token": "not-hex-zz" }),
        json!({ "name": "", "pairing_token": raw('a'), "device_token": raw('b') }),
        json!({ "name": "x" }),
    ] {
        let resp = app
            .client()
            .post(app.url("/api/hub/devices"))
            .json(&body)
            .send()
            .await
            .expect("register request");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "body: {body}");
    }
}

// (b) approval binds ────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_binds_device_to_user() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("dave", "pw").await;
    let pairing = raw('a');
    let device = raw('b');
    let (id, _) = register(&app, "kellner-1", &pairing, &device).await;

    // The page asks the question.
    let page = app
        .client()
        .get(app.url(&format!("/kellner/anheuern?token={pairing}")))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("approval GET");
    assert_eq!(page.status(), StatusCode::OK);
    let html = page.text().await.expect("html");
    assert!(html.contains("diesen kellner anheuern?"), "asks the question");
    assert!(html.contains("kellner-1"), "shows the device name");

    let bind = app
        .client()
        .post(app.url("/kellner/anheuern"))
        .header("Cookie", &cookie)
        .form(&[("token", pairing.as_str()), ("decision", "ja")])
        .send()
        .await
        .expect("approval POST");
    assert_eq!(bind.status(), StatusCode::OK);
    assert!(bind.text().await.expect("html").contains("angeheuert"));

    let (owner, token_hash, pending, paired_at, expires_at) = row(&app, id).await;
    assert_eq!(owner, Some(uid), "bound to the session user");
    assert!(paired_at.is_some(), "paired_at set");
    assert_eq!(expires_at, None, "TTL cleared after binding");
    assert!(pending.is_none(), "staging slot cleared");
    assert_eq!(token_hash, hash_token(&device), "token_hash rotated to device token");
}

// (c) pairing token is single-use ───────────────────────────────────────────

#[tokio::test]
async fn pairing_token_is_single_use() {
    let app = common::spawn().await;
    let (_uid, cookie) = app.user_with_password("dave", "pw").await;
    let pairing = raw('a');
    let (_, _) = register(&app, "kellner-1", &pairing, &raw('b')).await;

    // "Nein" does not bind; the token stays valid.
    let no = app
        .client()
        .post(app.url("/kellner/anheuern"))
        .header("Cookie", &cookie)
        .form(&[("token", pairing.as_str()), ("decision", "nein")])
        .send()
        .await
        .expect("decline");
    assert_eq!(no.status(), StatusCode::SEE_OTHER, "decline redirects home");

    let ja = app
        .client()
        .post(app.url("/kellner/anheuern"))
        .header("Cookie", &cookie)
        .form(&[("token", pairing.as_str()), ("decision", "ja")])
        .send()
        .await
        .expect("approve");
    assert_eq!(ja.status(), StatusCode::OK);

    // Replaying the link now fails the pending lookup.
    let replay = app
        .client()
        .get(app.url(&format!("/kellner/anheuern?token={pairing}")))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("replay GET");
    assert_eq!(replay.status(), StatusCode::GONE);

    let replay_post = app
        .client()
        .post(app.url("/kellner/anheuern"))
        .header("Cookie", &cookie)
        .form(&[("token", pairing.as_str()), ("decision", "ja")])
        .send()
        .await
        .expect("replay POST");
    assert_eq!(replay_post.status(), StatusCode::GONE);
}

// (d) expiry ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn expired_pending_row_is_not_bindable_and_is_swept() {
    let app = common::spawn().await;
    let (_uid, cookie) = app.user_with_password("dave", "pw").await;
    let pairing = raw('a');
    let _id = insert_device(
        &app,
        None,
        "kellner-old",
        &hash_token(&pairing),
        Some("2020-01-01T00:00:00+00:00"),
    )
    .await;

    let get = app
        .client()
        .get(app.url(&format!("/kellner/anheuern?token={pairing}")))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("expired GET");
    assert_eq!(get.status(), StatusCode::GONE, "expired token not bindable");

    // A register call sweeps the expired row.
    let _ = register(&app, "kellner-new", &raw('c'), &raw('d')).await;
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_devices WHERE name = 'kellner-old'")
            .fetch_one(&app.pool)
            .await
            .expect("count");
    assert_eq!(remaining, 0, "expired pending row was swept");
}

// (e) list is session-gated + user-isolated ─────────────────────────────────

#[tokio::test]
async fn list_requires_session_and_is_user_isolated() {
    let app = common::spawn().await;
    let anon = app
        .client()
        .get(app.url("/api/hub/devices"))
        .send()
        .await
        .expect("anon list");
    assert_eq!(anon.status(), StatusCode::SEE_OTHER, "unauthenticated → /login");
    assert_eq!(
        anon.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/login")
    );

    insert_device(&app, Some(app.seed.alice), "alice-box", "aabb", None).await;
    insert_device(&app, Some(app.seed.bob), "bob-box", "ccdd", None).await;

    let alice_cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url("/api/hub/devices"))
        .header("Cookie", &alice_cookie)
        .send()
        .await
        .expect("alice list");
    assert_eq!(resp.status(), StatusCode::OK);
    let txt = resp.text().await.expect("body");
    assert!(txt.contains("alice-box"));
    assert!(!txt.contains("bob-box"), "other user's device is absent");
    assert!(!txt.contains("token_hash"), "no token hashes in the list");
    assert!(!txt.contains("aabb"), "no token hashes in the list");
}

#[tokio::test]
async fn list_empty_for_fresh_user() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url("/api/hub/devices"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("list");
    assert_eq!(resp.status(), StatusCode::OK);
    let v: Value = resp.json().await.expect("json");
    assert_eq!(v, json!([]));
}

// (f) revoke ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn revoke_sets_timestamp_is_idempotent_and_hides_foreign_devices() {
    let app = common::spawn().await;
    let id = insert_device(&app, Some(app.seed.alice), "alice-box", "aabb", None).await;
    let alice = app.session_cookie(app.seed.alice).await;
    let bob = app.session_cookie(app.seed.bob).await;

    let first = app
        .client()
        .delete(app.url(&format!("/api/hub/devices/{id}")))
        .header("Cookie", &alice)
        .send()
        .await
        .expect("revoke");
    assert_eq!(first.status(), StatusCode::OK);
    let revoked_at: Option<String> =
        sqlx::query_scalar("SELECT revoked_at FROM hub_devices WHERE id = ?1")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .expect("revoked_at");
    assert!(revoked_at.is_some());

    // Second call: still 200, timestamp unchanged.
    let second = app
        .client()
        .delete(app.url(&format!("/api/hub/devices/{id}")))
        .header("Cookie", &alice)
        .send()
        .await
        .expect("revoke again");
    assert_eq!(second.status(), StatusCode::OK);
    let again: Option<String> =
        sqlx::query_scalar("SELECT revoked_at FROM hub_devices WHERE id = ?1")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .expect("revoked_at");
    assert_eq!(revoked_at, again, "revoke is idempotent");

    // Another user cannot revoke it (no existence leak).
    let foreign = app
        .client()
        .delete(app.url(&format!("/api/hub/devices/{id}")))
        .header("Cookie", &bob)
        .send()
        .await
        .expect("foreign revoke");
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);

    // Unknown id → 404.
    let unknown = app
        .client()
        .delete(app.url("/api/hub/devices/999999"))
        .header("Cookie", &alice)
        .send()
        .await
        .expect("unknown revoke");
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}
