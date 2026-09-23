//! Integration tests for the music-api surface on the Backpack endpoints.

mod common;

use serde_json::Value;

/// `GET /api/backpack` must always expose the `musicApi` block, even when the
/// service is not configured (the UI renders a hint from it).
#[tokio::test]
async fn backpack_status_includes_music_api_block() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{base}/api/backpack"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: Value = resp.json().await.unwrap();
    let m = &json["data"]["musicApi"];

    assert_eq!(m["configured"], Value::Bool(false), "test app is unconfigured");
    for key in ["demand", "ordered", "ready", "imported", "absent", "failed"] {
        assert!(
            m[key].as_i64().is_some(),
            "musicApi.{key} must be a number, got {:?}",
            m[key]
        );
    }
    assert_eq!(m["imported"].as_i64().unwrap(), 0);
}

/// Pulling without a configured service is a conflict, not a server error —
/// nothing is broken, there is just nothing to pull from.
#[tokio::test]
async fn backpack_pull_without_config_is_a_conflict() {
    let (client, base, _pool) = common::spawn_test_app().await;

    let resp = client
        .post(format!("{base}/api/backpack/pull"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        409,
        "unconfigured pull should be 409 Conflict"
    );
    let json: Value = resp.json().await.unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("not configured"),
        "error body should explain the conflict, got {json}"
    );
}
