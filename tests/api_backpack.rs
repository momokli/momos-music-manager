//! Integration tests for the Backpack transport endpoints.
//!
//! Covers `GET /api/backpack` and `POST /api/backpack/push` — including the
//! dry-run path, which must not require Spotify credentials and must not write.

mod common;

use serde_json::Value;

/// Seed a playlist subscription + backpack tag so the set is non-empty.
async fn seed_backpack(pool: &sqlx::Pool<sqlx::Sqlite>) {
    common::seed_basic_data(pool).await;

    // Subscribe playlist 'Groovy' (holds track 1/2 in the basic scenario…).
    sqlx::query(
        "INSERT INTO playlist_subscriptions (service, playlist_id, is_active)
         SELECT 'spotify', playlist_id, 1 FROM service_playlists LIMIT 1",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn backpack_status_reports_set_size() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_backpack(&pool).await;

    let resp = client
        .get(format!("{base}/api/backpack"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    assert!(data["trackCount"].as_i64().unwrap() >= 0);
    assert!(data["fileCount"].as_i64().unwrap() >= 0);
    assert_eq!(data["dirty"], Value::Bool(false));
    assert_eq!(data["pushPending"], Value::Bool(false));
    assert!(data["playlistUrl"].is_null());
    // No playlist/signature yet → not in sync (the badge's source of truth).
    assert_eq!(data["inSync"], Value::Bool(false));
}

#[tokio::test]
async fn backpack_push_dry_run_never_writes() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_backpack(&pool).await;

    let resp = client
        .post(format!("{base}/api/backpack/push"))
        .json(&serde_json::json!({"dryRun": true, "force": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "dry run must not need Spotify credentials");

    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    assert_eq!(data["dryRun"], Value::Bool(true));
    assert_eq!(data["updated"], Value::Bool(false));
    assert_eq!(data["deemixSubmitted"], Value::Bool(false));

    // No persisted transport state, no deemix row.
    let sig: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'backpack.signature'")
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(sig.is_none(), "dry run must not persist a signature");

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deemix_downloads WHERE is_backpack = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "dry run must not submit to deemix");
}

#[tokio::test]
async fn backpack_push_without_spotify_reports_config_error() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_backpack(&pool).await;

    // The test app has no stored Spotify tokens → a real push cannot run.
    let resp = client
        .post(format!("{base}/api/backpack/push"))
        .json(&serde_json::json!({"force": true, "submitToDeemix": false}))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_server_error(),
        "expected 5xx when Spotify is not configured, got {}",
        resp.status()
    );
}
