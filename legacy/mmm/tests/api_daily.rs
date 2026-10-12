//! Smoke tests for /api/daily/generate
mod common;

#[tokio::test]
async fn daily_generate_no_tags_error() {
    let (client, base, _pool) = common::spawn_test_app().await;

    let resp = client
        .post(format!("{}/api/daily/generate", base))
        .json(&serde_json::json!({"tags": [], "limit": 5}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn daily_generate_nonexistent_tag_returns_zero() {
    let (client, base, _pool) = common::spawn_test_app().await;

    let resp = client
        .post(format!("{}/api/daily/generate", base))
        .json(&serde_json::json!({
            "tags": ["nonexistent_tag"],
            "bpmMin": 0,
            "bpmMax": 300,
            "limit": 5,
            "excludeFullyTagged": false
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["data"]["trackCount"], 0);
}

#[tokio::test]
async fn daily_generate_response_includes_spotify_push_status() {
    let (client, base, pool) = common::spawn_test_app().await;
    momos_music_manager::db::testing::seed_basic_scenario(&pool).await;
    momos_music_manager::db::refresh_track_resolved_tags(&pool)
        .await
        .unwrap();

    let resp = client
        .post(format!("{}/api/daily/generate", base))
        .json(&serde_json::json!({
            "tags": ["Groovy"],
            "bpmMin": 0,
            "bpmMax": 300,
            "limit": 3,
            "excludeFullyTagged": false
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let data = &json["data"];

    // Verify all expected fields exist
    assert!(data.get("playlistId").is_some(), "missing playlistId");
    assert!(data.get("playlistName").is_some(), "missing playlistName");
    assert!(data.get("trackCount").is_some(), "missing trackCount");
    assert!(
        data.get("spotifyPushStatus").is_some(),
        "missing spotifyPushStatus"
    );

    // Verify spotifyPushStatus is a known value
    let status = data["spotifyPushStatus"].as_str().unwrap();
    assert!(
        matches!(status, "ok" | "failed" | "not_configured" | "no_tracks"),
        "unexpected spotifyPushStatus: {}",
        status
    );

    // With seed data, should find at least 1 track
    let count = data["trackCount"].as_i64().unwrap();
    assert!(
        count > 0,
        "expected at least 1 matching track, got {}",
        count
    );
}
