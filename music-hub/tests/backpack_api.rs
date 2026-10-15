//! Integration tests for the device-token-gated backpack read API (issue #367):
//! auth (401 without/with an unknown token), the exact list/export JSON shape,
//! owner scoping (foreign → 404) and the empty-backpack → `[]` case.

mod common;

use music_hub::backpacks::{self, SourceRef};
use music_hub::devices::hash_token;
use reqwest::StatusCode;
use serde_json::{Value, json};

const ALICE_TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Insert a **bound** device owned by `user_id` whose device token hashes to
/// `token_hash`; returns the device id.
async fn bound_device(app: &common::TestApp, user_id: i64, name: &str, raw_token: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_devices (user_id, name, token_hash, created_at, paired_at)
         VALUES (?1, ?2, ?3, ?4, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(hash_token(raw_token))
    .bind("2026-01-01T00:00:00+00:00")
    .fetch_one(&app.pool)
    .await
    .expect("insert bound device")
}

/// Insert a hub track; returns its id.
async fn track(app: &common::TestApp, sid: &str, isrc: &str, title: &str, artists: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, isrc, title, artists, first_seen_at)
         VALUES ('spotify', ?1, ?2, ?3, ?4, ?5) RETURNING id",
    )
    .bind(sid)
    .bind(isrc)
    .bind(title)
    .bind(artists)
    .bind("2026-01-01T00:00:00+00:00")
    .fetch_one(&app.pool)
    .await
    .expect("insert track")
}

/// Insert a tag owned by `user_id`; returns its id.
async fn tag(app: &common::TestApp, user_id: i64, slug: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, ?2, ?3, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(slug)
    .bind(name)
    .bind("2026-01-01T00:00:00+00:00")
    .fetch_one(&app.pool)
    .await
    .expect("insert tag")
}

async fn resolve_tag(app: &common::TestApp, track_id: i64, tag_id: i64) {
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track_id)
        .bind(tag_id)
        .execute(&app.pool)
        .await
        .expect("insert resolved tag");
}

async fn audio(
    app: &common::TestApp,
    track_id: i64,
    bpm: Option<f64>,
    key: Option<&str>,
    energy: Option<f64>,
) {
    sqlx::query(
        "INSERT INTO hub_track_audio (track_id, bpm, musical_key, energy) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(track_id)
    .bind(bpm)
    .bind(key)
    .bind(energy)
    .execute(&app.pool)
    .await
    .expect("insert audio");
}

async fn json_body(resp: reqwest::Response) -> (StatusCode, Value) {
    let status = resp.status();
    let txt = resp.text().await.expect("body");
    let v = serde_json::from_str(&txt).unwrap_or(Value::Null);
    (status, v)
}

// ── auth ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn missing_or_unknown_token_is_401() {
    let app = common::spawn().await;

    // No Authorization header at all.
    let (status, v) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks"))
            .send()
            .await
            .expect("anon list"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(v.get("error").is_some(), "error object: {v}");

    // Unknown (never-registered) token.
    let (status, v) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks"))
            .header("Authorization", "Bearer deadbeef")
            .send()
            .await
            .expect("unknown list"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(v.get("error").is_some());

    // Same for the export route.
    let (status, _) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks/1/export"))
            .send()
            .await
            .expect("anon export"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unbound_pending_token_is_401() {
    let app = common::spawn().await;
    // A pending (unbound) row: user_id NULL, no paired_at.
    sqlx::query(
        "INSERT INTO hub_devices (user_id, name, token_hash, created_at, expires_at)
         VALUES (NULL, 'pending', ?1, ?2, ?3)",
    )
    .bind(hash_token("pending-token"))
    .bind("2026-01-01T00:00:00+00:00")
    .bind("2099-01-01T00:00:00+00:00")
    .execute(&app.pool)
    .await
    .expect("insert pending");

    let (status, _) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks"))
            .header("Authorization", "Bearer pending-token")
            .send()
            .await
            .expect("pending list"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── list ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_returns_only_owned_backpacks() {
    let app = common::spawn().await;
    bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;

    let bp_active = backpacks::create_backpack(&app.pool, app.seed.alice, "Reise")
        .await
        .expect("create");
    let bp_idle = backpacks::create_backpack(&app.pool, app.seed.alice, "Chill")
        .await
        .expect("create");
    backpacks::set_active(&app.pool, app.seed.alice, bp_active)
        .await
        .expect("set active");
    // A backpack owned by bob must not leak into alice's list.
    backpacks::create_backpack(&app.pool, app.seed.bob, "Bob Only")
        .await
        .expect("create bob");

    let (status, v) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks"))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("list"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        v,
        json!({ "backpacks": [
            { "id": bp_active, "name": "Reise", "is_active": true },
            { "id": bp_idle, "name": "Chill", "is_active": false },
        ]}),
        "exact list shape: {v}"
    );
}

// ── export ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn export_has_exact_shape_with_tags_and_audio() {
    let app = common::spawn().await;
    bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;

    let t1 = track(&app, "s1", "DEAAA0000001", "Alpha", "Aaa").await;
    let t2 = track(&app, "s2", "DEAAA0000002", "Beta", "Bbb").await;

    // Tags on t1 in non-alphabetical insert order; API must sort by name.
    let tag_zeta = tag(&app, app.seed.alice, "zeta", "Zeta").await;
    let tag_alpha = tag(&app, app.seed.alice, "alpha", "Alpha").await;
    resolve_tag(&app, t1, tag_zeta).await;
    resolve_tag(&app, t1, tag_alpha).await;

    audio(&app, t1, Some(128.0), Some("A-Minor"), Some(7.0)).await;
    // t2 has no audio row at all -> bpm/key/energy null.

    let bp = backpacks::create_backpack(&app.pool, app.seed.alice, "Mix")
        .await
        .expect("create");
    backpacks::add_source(&app.pool, bp, SourceRef::Track(t1))
        .await
        .expect("add t1");
    backpacks::add_source(&app.pool, bp, SourceRef::Track(t2))
        .await
        .expect("add t2");

    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("export"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        v,
        json!({
            "backpack": { "id": bp, "name": "Mix" },
            "tracks": [
                {
                    "track_id": t1,
                    "isrc": "DEAAA0000001",
                    "title": "Alpha",
                    "artists": "Aaa",
                    "tags": ["Alpha", "Zeta"],
                    "bpm": 128.0,
                    "key": "A-Minor",
                    "energy": 7.0,
                },
                {
                    "track_id": t2,
                    "isrc": "DEAAA0000002",
                    "title": "Beta",
                    "artists": "Bbb",
                    "tags": [],
                    "bpm": null,
                    "key": null,
                    "energy": null,
                },
            ]
        }),
        "exact export shape: {v}"
    );
}

#[tokio::test]
async fn empty_backpack_exports_empty_tracks() {
    let app = common::spawn().await;
    bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;
    let bp = backpacks::create_backpack(&app.pool, app.seed.alice, "Leer")
        .await
        .expect("create");

    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("export"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "empty backpack is not a 500: {v}");
    assert_eq!(
        v,
        json!({ "backpack": { "id": bp, "name": "Leer" }, "tracks": [] })
    );
}

#[tokio::test]
async fn foreign_and_unknown_backpack_are_404() {
    let app = common::spawn().await;
    bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;

    let bob_bp = backpacks::create_backpack(&app.pool, app.seed.bob, "Bob Only")
        .await
        .expect("create bob");

    // Alice's device cannot read bob's backpack (no existence leak).
    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bob_bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("foreign export"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v, json!({ "error": "unknown backpack" }));

    // Unknown id -> same 404.
    let (status, v) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks/999999/export"))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("unknown export"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v, json!({ "error": "unknown backpack" }));
}
