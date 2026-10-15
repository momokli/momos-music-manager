//! Integration tests for the device-token-gated Traktor import route
//! (issue #364, leaf P1-4): `POST /api/hub/traktor/import`.
//!
//! Covers the seeded-NML happy path (exact playcount / rating / playlist rows),
//! idempotent re-import, and the 401 / 400 rejection cases.

mod common;

use reqwest::StatusCode;
use serde_json::Value;
use sqlx::Row;

use music_hub::devices::hash_token;

const NML: &str = include_str!("fixtures/collection.nml");

fn raw(seed: char) -> String {
    std::iter::repeat(seed).take(64).collect()
}

/// Insert a **bound** device row (paired + owned) and return its raw token.
async fn bind_device(app: &common::TestApp, user_id: i64, token: &str) {
    sqlx::query(
        "INSERT INTO hub_devices (user_id, name, token_hash, created_at, paired_at)
         VALUES (?1, 'kellner-test', ?2, ?3, ?3)",
    )
    .bind(user_id)
    .bind(hash_token(token))
    .bind("2026-01-01T00:00:00+00:00")
    .execute(&app.pool)
    .await
    .expect("insert bound device");
}

/// Insert a device row in a non-bindable state (unbound or revoked).
async fn insert_nonbindable(app: &common::TestApp, token: &str, revoked: bool) {
    let revoked_at: Option<&str> = revoked.then_some("2026-01-01T00:00:00+00:00");
    sqlx::query(
        "INSERT INTO hub_devices (user_id, name, token_hash, created_at, paired_at, revoked_at)
         VALUES (NULL, 'kellner-pending', ?1, ?2, NULL, ?3)",
    )
    .bind(hash_token(token))
    .bind("2026-01-01T00:00:00+00:00")
    .bind(revoked_at)
    .execute(&app.pool)
    .await
    .expect("insert non-bindable device");
}

async fn seed_tracks(pool: &sqlx::SqlitePool) {
    sqlx::query(
        "INSERT INTO hub_tracks (id, service, service_track_id, title, artists) VALUES
            (9101, 'local', 'tk1', 'Alpha', 'Artist A'),
            (9102, 'local', 'tk2', 'Beta',  'Artist B')",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn post_nml(app: &common::TestApp, token: Option<&str>, body: &str) -> (StatusCode, Value) {
    let mut req = app.client().post(app.url("/api/hub/traktor/import"));
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req.body(body.to_string()).send().await.expect("import request");
    let status = resp.status();
    let txt = resp.text().await.unwrap_or_default();
    (status, serde_json::from_str(&txt).unwrap_or(Value::Null))
}

// (a) happy path: seeded NML lands exact rows ────────────────────────────────

#[tokio::test]
async fn device_token_import_stores_meta_and_playlists() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    seed_tracks(&app.pool).await;
    let token = raw('a');
    bind_device(&app, alice, &token).await;

    let (status, v) = post_nml(&app, Some(&token), NML).await;
    assert_eq!(status, StatusCode::OK, "import status: {v}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["stats"]["entries"], 3);
    assert_eq!(v["stats"]["matched"], 2, "Alpha + Beta matched, Gamma not");
    assert_eq!(v["stats"]["playlists"], 3, "Collection + Peak Time + History");
    assert_eq!(v["stats"]["with_bpm"], 1);
    assert_eq!(v["stats"]["with_key"], 1);

    // Per-track meta for Alpha (id 9101).
    let r = sqlx::query(
        "SELECT play_count, last_played, rating, bpm, camelot
           FROM hub_traktor_tracks WHERE user_id = ?1 AND track_id = 9101",
    )
    .bind(alice)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(r.get::<i64, _>("play_count"), 7);
    assert_eq!(r.get::<Option<String>, _>("last_played").as_deref(), Some("2024/3/1"));
    assert_eq!(r.get::<Option<i64>, _>("rating"), Some(5), "255 -> 5 stars");
    assert_eq!(r.get::<Option<f64>, _>("bpm"), Some(124.0));
    assert_eq!(r.get::<Option<String>, _>("camelot").as_deref(), Some("8A"));

    // Beta: playcount 0, rating 0 -> none.
    let (pc, rating): (i64, Option<i64>) = sqlx::query_as(
        "SELECT play_count, rating FROM hub_traktor_tracks WHERE user_id = ?1 AND track_id = 9102",
    )
    .bind(alice)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(pc, 0);
    assert_eq!(rating, None);

    // Playlist node types.
    let types: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, node_type FROM hub_traktor_playlists WHERE user_id = ?1 ORDER BY node_type, name",
    )
    .bind(alice)
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert!(types.contains(&("Collection".into(), "collection".into())));
    assert!(types.contains(&("Peak Time".into(), "playlist".into())));
    assert!(types.contains(&("History 2024".into(), "session".into())));

    // The bound device was marked seen by the route.
    let seen: Option<String> =
        sqlx::query_scalar("SELECT last_seen_at FROM hub_devices WHERE token_hash = ?1")
            .bind(hash_token(&token))
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert!(seen.is_some(), "last_seen_at touched by the device-gated route");
}

// (b) idempotent re-import: same NML twice → stable row counts ───────────────

#[tokio::test]
async fn device_token_import_is_idempotent() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    seed_tracks(&app.pool).await;
    let token = raw('b');
    bind_device(&app, alice, &token).await;

    let (s1, v1) = post_nml(&app, Some(&token), NML).await;
    let (s2, v2) = post_nml(&app, Some(&token), NML).await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(v1["stats"], v2["stats"], "stats stable across re-import");

    let playlists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_traktor_playlists WHERE user_id = ?1")
            .bind(alice)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(playlists, 3, "re-import replaces, does not duplicate");

    let tracks: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_traktor_tracks WHERE user_id = ?1")
            .bind(alice)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(tracks, 2);
}

// (c) 401: missing / unknown / unbound / revoked tokens ─────────────────────

#[tokio::test]
async fn unauthenticated_and_unknown_tokens_get_401() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    seed_tracks(&app.pool).await;

    // No Authorization header at all.
    let (status, _) = post_nml(&app, None, NML).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Unknown token (never registered).
    let (status, _) = post_nml(&app, Some(&raw('c')), NML).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Unbound token (pending row, no owner).
    let unbound = raw('d');
    insert_nonbindable(&app, &unbound, false).await;
    let (status, _) = post_nml(&app, Some(&unbound), NML).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Revoked token (bound then revoked).
    let revoked = raw('e');
    sqlx::query(
        "INSERT INTO hub_devices (user_id, name, token_hash, created_at, paired_at, revoked_at)
         VALUES (?1, 'kellner-revoked', ?2, ?3, ?3, ?3)",
    )
    .bind(alice)
    .bind(hash_token(&revoked))
    .bind("2026-01-01T00:00:00+00:00")
    .execute(&app.pool)
    .await
    .unwrap();
    let (status, _) = post_nml(&app, Some(&revoked), NML).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// (d) 400: empty / non-UTF-8 / non-NML bodies ───────────────────────────────

#[tokio::test]
async fn malformed_bodies_get_400() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let token = raw('f');
    bind_device(&app, alice, &token).await;

    // Empty body.
    let (status, _) = post_nml(&app, Some(&token), "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Valid XML but not NML.
    let (status, _) = post_nml(&app, Some(&token), "<foo></foo>").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // NML-looking but unparseable XML.
    let (status, _) = post_nml(&app, Some(&token), "<NML><BROKEN</NML>").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
