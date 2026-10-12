//! Integration tests for `/api/health` endpoint.

mod common;

use serde_json::Value;

/// GET /api/health — returns `{"status": "ok", "database": "connected"}`.
#[tokio::test]
async fn health_check() {
    let (client, base, pool) = common::spawn_test_app().await;
    // No seed data needed — health only checks DB connection
    let _ = &pool;

    let resp = client
        .get(format!("{}/api/health", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "health endpoint should return 200");

    let json: Value = resp.json().await.unwrap();

    eprintln!("health response: {json:#}");

    assert_eq!(
        json["status"].as_str().unwrap(),
        "ok",
        "status should be 'ok'"
    );
    assert!(
        json["database"].as_str().is_some(),
        "should have database field"
    );
}

/// GET /api/version — returns version string.
#[tokio::test]
async fn version_check() {
    let (client, base, pool) = common::spawn_test_app().await;
    let _ = &pool;

    let resp = client
        .get(format!("{}/api/version", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "version endpoint should return 200");

    let json: Value = resp.json().await.unwrap();
    eprintln!("version response: {json:#}");

    assert!(
        json["version"].as_str().is_some(),
        "should have version field, got: {json:#}"
    );
}
