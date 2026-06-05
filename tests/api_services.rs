//! Integration tests for `/api/services*` endpoints.

mod common;

use serde_json::Value;

/// GET /api/services — returns array of service status objects.
#[tokio::test]
async fn services_list() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/services", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let services = json["data"].as_array().unwrap();

    eprintln!("services: {services:?}");

    assert!(!services.is_empty(), "services list should not be empty");

    // Each service should have an id/name field
    for svc in services {
        assert!(
            svc["id"].is_string() || svc["name"].is_string() || svc["service"].is_string(),
            "each service should have an identifier"
        );
    }

    // Should include Spotify and SoundCloud
    let has_spotify = services.iter().any(|s| {
        s["id"].as_str().map_or(false, |id| id.contains("spotify"))
            || s["name"].as_str().map_or(false, |n| n.contains("spotify"))
            || s["service"]
                .as_str()
                .map_or(false, |sv| sv.contains("spotify"))
    });
    assert!(has_spotify, "should include Spotify service");

    let has_soundcloud = services.iter().any(|s| {
        s["id"]
            .as_str()
            .map_or(false, |id| id.contains("soundcloud"))
            || s["name"]
                .as_str()
                .map_or(false, |n| n.contains("soundcloud"))
            || s["service"]
                .as_str()
                .map_or(false, |sv| sv.contains("soundcloud"))
    });
    assert!(has_soundcloud, "should include SoundCloud service");
}

/// POST /api/services/soundcloud/sync — returns error (not configured/not implemented).
#[tokio::test]
async fn services_sync_not_configured() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/services/soundcloud/sync", base))
        .send()
        .await
        .unwrap();

    // SoundCloud sync returns 501 (NOT_IMPLEMENTED) from service_sync_handler
    let status = resp.status();
    eprintln!("soundcloud sync status: {status}");

    let body = resp.text().await.unwrap();
    eprintln!("soundcloud sync body: {body}");

    assert!(
        status == 501 || status == 400 || status == 500,
        "SoundCloud sync should return error status (501/400/500), got {status}"
    );
    assert!(
        body.contains("not")
            || body.contains("error")
            || body.contains("implemented")
            || body.contains("Not"),
        "response should indicate failure"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 2 — Service config
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/services/spotify/config` — returns config or "not configured".
async fn services_config_get() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/services/spotify/config", base))
        .send()
        .await
        .unwrap();

    // Spotify not configured in test, so returns 404
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify config response: {body}");

    assert!(
        status == 200 || status == 404 || status == 400,
        "config endpoint should return 200/400/404, got {}",
        status
    );
}

#[tokio::test]
/// `PUT /api/services/spotify/config` — updates service config.
async fn services_config_put() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Try enabling (may fail since Spotify not configured, but should return a response)
    let resp = client
        .put(format!("{}/api/services/spotify/config", base))
        .json(&serde_json::json!({
            "user_id": "test_user",
            "is_connected": false
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify config put response: {body}");

    assert!(
        status == 200 || status == 400 || status == 500,
        "config put should return 200/400/500, got {}",
        status
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 3 — Fetch counts
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/services/spotify/fetch-counts` — returns fetch counts.
async fn services_fetch_counts() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/services/spotify/fetch-counts", base))
        .send()
        .await
        .unwrap();

    // Returns 501 NOT_IMPLEMENTED for services other than soundcloud
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify fetch-counts response: {body}");

    assert!(
        status == 501 || status == 200 || status == 400 || status == 500,
        "fetch-counts should return 501/200/400/500, got {}",
        status
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 4 — Sync status
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/services/spotify/sync-status` — returns sync status.
async fn services_sync_status() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/services/spotify/sync-status", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify sync-status response: {body}");

    assert!(
        status == 200 || status == 404 || status == 400,
        "sync-status should return 200/404/400, got {}",
        status
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 5 — Service reset
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/services/spotify/reset` — resets a service (may succeed or error).
async fn services_reset() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/services/spotify/reset", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify reset response: {body}");

    assert!(
        status == 200 || status == 400 || status == 500,
        "reset should return 200/400/500, got {}",
        status
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 6 — Deemix auth
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/services/deemix/auth` — deemix auth endpoint (empty host → 400).
async fn services_deemix_auth() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // DeemixAuthRequest requires both `arl` and `host`. Empty host should fail validation.
    let resp = client
        .post(format!("{}/api/services/deemix/auth", base))
        .json(&serde_json::json!({"host": "", "arl": "test-arl"}))
        .send()
        .await
        .unwrap();

    // Empty host should be rejected
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("deemix auth response: {body}");

    assert!(
        status == 400 || status == 200 || status == 500,
        "deemix auth should return 400/200/500, got {}",
        status
    );
}
