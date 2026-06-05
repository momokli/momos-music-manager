//! Integration tests for Spotify sync endpoints.
//!
//! All endpoints check if Spotify is configured (via `is_spotify_configured()`).
//! In the test environment, Spotify is NOT configured, so all endpoints return
//! error responses (400 BAD_REQUEST).

mod common;

use serde_json::Value;

/// POST /api/services/spotify/sync/playlists — error (Spotify not configured).
#[tokio::test]
async fn spotify_sync_playlists_error() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/services/spotify/sync/playlists", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify sync playlists error: {body}");

    assert_eq!(
        status, 400,
        "sync playlists without config should return 400, got {status}"
    );
    assert!(
        body["data"]
            .as_str()
            .map_or(false, |s| s.contains("not configured"))
            || body["error"]
                .as_str()
                .map_or(false, |s| s.contains("not configured")),
        "response should indicate not configured, got: {body}"
    );
}

/// POST /api/services/spotify/sync/new-playlists — error (Spotify not configured).
#[tokio::test]
async fn spotify_sync_new_playlists_error() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/services/spotify/sync/new-playlists", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify sync new playlists error: {body}");

    assert_eq!(
        status, 400,
        "sync new playlists without config should return 400, got {status}"
    );
    assert!(
        body["data"]
            .as_str()
            .map_or(false, |s| s.contains("not configured"))
            || body["error"]
                .as_str()
                .map_or(false, |s| s.contains("not configured")),
        "response should indicate not configured"
    );
}

/// POST /api/services/spotify/sync/playlists/batch — error (Spotify not configured).
#[tokio::test]
async fn spotify_sync_playlists_batch_error() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // BatchSyncRequest requires `mode` field. Endpoint checks config first.
    let resp = client
        .post(format!(
            "{}/api/services/spotify/sync/playlists/batch",
            base
        ))
        .json(&serde_json::json!({"mode": "stale"}))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    eprintln!("spotify sync batch status: {status}");

    // The response may or may not have a JSON body
    let body_text = resp.text().await.unwrap_or_default();
    eprintln!("spotify sync batch body: {body_text:?}");

    assert_eq!(
        status, 400,
        "sync playlists batch without config should return 400, got {status}"
    );
    assert!(
        body_text.to_lowercase().contains("not configured"),
        "response should indicate not configured"
    );
}

/// POST /api/services/spotify/sync/tracks — error (Spotify not configured).
#[tokio::test]
async fn spotify_sync_tracks_error() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/services/spotify/sync/tracks", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify sync tracks error: {body}");

    assert_eq!(
        status, 400,
        "sync tracks without config should return 400, got {status}"
    );
    assert!(
        body["data"]
            .as_str()
            .map_or(false, |s| s.contains("not configured"))
            || body["error"]
                .as_str()
                .map_or(false, |s| s.contains("not configured")),
        "response should indicate not configured"
    );
}

/// POST /api/services/spotify/refresh-playlist/1 — error (Spotify not configured).
#[tokio::test]
async fn spotify_refresh_playlist_error() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/services/spotify/refresh-playlist/1", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify refresh playlist error: {body}");

    assert_eq!(
        status, 400,
        "refresh playlist without config should return 400, got {status}"
    );
    assert!(
        body["data"]
            .as_str()
            .map_or(false, |s| s.contains("not configured"))
            || body["error"]
                .as_str()
                .map_or(false, |s| s.contains("not configured")),
        "response should indicate not configured"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Additional tests
// ═══════════════════════════════════════════════════════════════════════════

/// DELETE /api/services/spotify/sync/{task_id} — cancel non-existent task.
#[tokio::test]
async fn spotify_sync_task_cancel() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .delete(format!(
            "{}/api/services/spotify/sync/nonexistent-task-id",
            base
        ))
        .send()
        .await
        .unwrap();

    // Non-existent task → error (probably 500 since cancel_task returns error)
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify sync cancel: {body}");

    assert!(
        status == 404 || status == 500,
        "cancel non-existent task should return 404 or 500, got {status}"
    );
}

/// POST /api/services/spotify/refresh-playlist/9999 — non-existent playlist_id (no config).
#[tokio::test]
async fn spotify_refresh_playlist_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!(
            "{}/api/services/spotify/refresh-playlist/9999",
            base
        ))
        .send()
        .await
        .unwrap();

    // Spotify not configured → 400 even with non-existent playlist ID
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify refresh playlist 9999: {body}");

    assert_eq!(
        status, 400,
        "refresh-playlist/9999 without config should return 400, got {status}"
    );
    assert!(
        body["data"]
            .as_str()
            .map_or(false, |s| s.contains("not configured"))
            || body["error"]
                .as_str()
                .map_or(false, |s| s.contains("not configured")),
        "response should indicate not configured, got: {body}"
    );
}

/// POST /api/services/spotify/sync — full sync error (Spotify not configured).
#[tokio::test]
async fn spotify_sync_full_error() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/services/spotify/sync", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("spotify sync full error: {body}");

    assert_eq!(
        status, 400,
        "sync full without config should return 400, got {status}"
    );
    assert!(
        body["data"]
            .as_str()
            .map_or(false, |s| s.contains("not configured"))
            || body["error"]
                .as_str()
                .map_or(false, |s| s.contains("not configured")),
        "response should indicate not configured, got: {body}"
    );
}
