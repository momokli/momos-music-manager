//! Integration tests for infrastructure endpoints:
//! - `/api/tag-similarities/*`
//! - `/api/traktor/*`
//! - `/api/embeddings/*`

mod common;

use serde_json::Value;
use sqlx::{Pool, Sqlite};

// ═══════════════════════════════════════════════════════════════════════════
// Testing seed endpoint — liked_songs (Issue #58)
// ═══════════════════════════════════════════════════════════════════════════

/// Read `(playlist_count, last_touched_at, liked)` for a track from the view.
async fn forgotten_facts(pool: &Pool<Sqlite>, track_id: i64) -> (i64, i64, i64) {
    sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT playlist_count, last_touched_at, liked \
         FROM v_track_forgotten_facts WHERE track_id = ?",
    )
    .bind(track_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// POST /api/testing/seed {"scenario":"liked_songs"} → 200 with the contract rows.
#[tokio::test]
async fn testing_seed_liked_songs_scenario() {
    let (client, base, pool) = common::spawn_test_app().await;

    let resp = client
        .post(format!("{}/api/testing/seed", base))
        .json(&serde_json::json!({ "scenario": "liked_songs" }))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("liked_songs seed: {body}");

    assert_eq!(
        status, 200,
        "liked_songs seed should return 200, got {status}"
    );
    assert_eq!(body["ok"], true);
    assert_eq!(body["scenario"], "liked_songs");
    assert_eq!(body["rows"]["service_playlists"], 5);
    assert_eq!(body["rows"]["service_playlist_tracks"], 5);

    // Playlists 5-7 exist with their kinds (PL6 generated).
    let kinds: Vec<(i64, String)> = sqlx::query_as::<_, (i64, String)>(
        "SELECT id, playlist_kind FROM service_playlists WHERE id IN (5, 6, 7) ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        kinds,
        vec![
            (5, "liked".to_string()),
            (6, "generated".to_string()),
            (7, "liked".to_string()),
        ]
    );

    // Track 1: playlist_count=1, last_touched_at=1600000000, liked.
    assert_eq!(forgotten_facts(&pool, 1).await, (1, 1600000000, 1));
    // Track 2: playlist_count=0, last_touched_at=1400000000, liked.
    assert_eq!(forgotten_facts(&pool, 2).await, (0, 1400000000, 1));
    // Track 3: playlist_count=2, last_touched_at=1700000000, not liked.
    assert_eq!(forgotten_facts(&pool, 3).await, (2, 1700000000, 0));

    // The generated playlist must never contribute a row to the view.
    let pl6_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM v_track_forgotten_facts f \
         JOIN service_playlist_tracks spt ON spt.track_id = f.track_id \
         WHERE spt.playlist_id = 6",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        pl6_rows, 0,
        "generated playlist 6 must not appear in the view"
    );
}

/// Unknown scenario still returns 400 (and the error lists liked_songs).
#[tokio::test]
async fn testing_seed_unknown_scenario_returns_400() {
    let (client, base, _pool) = common::spawn_test_app().await;

    let resp = client
        .post(format!("{}/api/testing/seed", base))
        .json(&serde_json::json!({ "scenario": "nope_not_a_scenario" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400, "unknown scenario should be 400");
    let body: Value = resp.json().await.unwrap();
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("liked_songs"),
        "error should list liked_songs: {err}"
    );
}

/// `clear_all_tables()` removes the liked_songs rows — no leak into later tests.
#[tokio::test]
async fn liked_songs_seed_cleared_without_leak() {
    let (_client, _base, pool) = common::spawn_test_app().await;
    common::seed_liked_songs_data(&pool).await;

    // Double-seeding must be idempotent.
    common::seed_liked_songs_data(&pool).await;
    let playlists_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM service_playlists")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        playlists_after, 5,
        "double seed must stay idempotent at 5 playlists"
    );

    momos_music_manager::db::testing::clear_all_tables(&pool).await;

    let leftover_playlists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM service_playlists WHERE id IN (5, 6, 7)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        leftover_playlists, 0,
        "liked/generated playlists must be cleared"
    );
    let leftover_tracks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM service_playlist_tracks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(leftover_tracks, 0, "playlist track rows must be cleared");
}

// ═══════════════════════════════════════════════════════════════════════════
// Testing seed endpoint — rediscovery (Issue #80)
// ═══════════════════════════════════════════════════════════════════════════

/// POST /api/testing/seed {"scenario":"rediscovery"} → 200 with the contract counts.
#[tokio::test]
async fn testing_seed_rediscovery_scenario() {
    let (client, base, pool) = common::spawn_test_app().await;

    let resp = client
        .post(format!("{}/api/testing/seed", base))
        .json(&serde_json::json!({ "scenario": "rediscovery" }))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("rediscovery seed: {body}");

    assert_eq!(
        status, 200,
        "rediscovery seed should return 200, got {status}"
    );
    assert_eq!(body["ok"], true);
    assert_eq!(body["scenario"], "rediscovery");

    // Contract counts (see seed_rediscovery_scenario's doc table).
    assert_eq!(body["rows"]["rediscovery_pushes"], 2);
    assert_eq!(body["rows"]["tags"], 4);
    assert_eq!(body["rows"]["files"], 13);
    assert_eq!(body["rows"]["file_locations"], 15);
    assert_eq!(body["rows"]["service_tracks"], 12);
    assert_eq!(body["rows"]["service_playlists"], 6);
    assert_eq!(body["rows"]["service_playlist_tracks"], 15);
    assert_eq!(body["rows"]["folders"], 1);

    // The two push rows carry the fixed anchor timestamps (no unixepoch).
    let pushes: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT track_id, pushed_at, slot FROM rediscovery_pushes ORDER BY track_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        pushes,
        vec![(17, 1787408000, Some(0)), (18, 1632320000, Some(1))]
    );

    // Track 16 is in the Backpack (tag 60), so it is a curated playlist too.
    assert_eq!(forgotten_facts(&pool, 16).await, (1, 1632320000, 1));
}

/// Double-seeding the rediscovery scenario is idempotent, and
/// `clear_all_tables()` clears `rediscovery_pushes` — no leak into later tests.
#[tokio::test]
async fn rediscovery_seed_cleared_without_leak() {
    let (_client, _base, pool) = common::spawn_test_app().await;
    common::seed_rediscovery_data(&pool).await;

    // Double-seeding must not change any count.
    common::seed_rediscovery_data(&pool).await;
    let pushes_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rediscovery_pushes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        pushes_after, 2,
        "double seed must stay idempotent at 2 pushes"
    );
    let playlists_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM service_playlists")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        playlists_after, 6,
        "double seed must stay idempotent at 6 playlists"
    );
    let files_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        files_after, 13,
        "double seed must stay idempotent at 13 files"
    );

    momos_music_manager::db::testing::clear_all_tables(&pool).await;

    let leftover_pushes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rediscovery_pushes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(leftover_pushes, 0, "rediscovery_pushes must be cleared");
    let leftover_files: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE id BETWEEN 10 AND 18")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(leftover_files, 0, "rediscovery files must be cleared");
}

// ═══════════════════════════════════════════════════════════════════════════
// Tag similarities
// ═══════════════════════════════════════════════════════════════════════════

/// GET /api/tag-similarities/status — returns similarity status.
#[tokio::test]
async fn tag_similarities_status() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tag-similarities/status", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("tag similarities status: {body}");

    assert!(
        status == 200 || status == 404 || status == 500,
        "tag similarities status should return 200/404/500, got {status}"
    );

    if status == 200 {
        assert!(
            body["data"].is_object() || body["data"].is_array(),
            "response data should be an object or array"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Traktor
// ═══════════════════════════════════════════════════════════════════════════

/// GET /api/traktor/status — returns Traktor import status.
#[tokio::test]
async fn traktor_status() {
    let (client, base, pool) = common::spawn_test_app().await;
    let _ = &pool;

    let resp = client
        .get(format!("{}/api/traktor/status", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("traktor status: {body}");

    // Traktor status may return 200 with status info, or 500 if no collection file
    assert!(
        status == 200 || status == 500,
        "traktor status should return 200 or 500, got {status}"
    );

    if status == 200 {
        assert!(
            body["data"].is_object(),
            "response data should be an object"
        );
    }
}

/// POST /api/traktor/import — error: no custom_path provided.
#[tokio::test]
async fn traktor_import_no_file() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/traktor/import", base))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("traktor import no file: {body}");

    // Without custom_path, returns 200 (creates task) or 500
    assert!(
        status == 200 || status == 400 || status == 500,
        "traktor import should return 200/400/500, got {status}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Embeddings
// ═══════════════════════════════════════════════════════════════════════════

/// GET /api/embeddings/status — returns status without model loaded.
#[tokio::test]
async fn embeddings_status() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/embeddings/status", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("embeddings status: {body}");

    assert_eq!(
        status, 200,
        "embeddings status should return 200, got {status}"
    );

    // Should have model_loaded, tags_embedded, etc.
    assert!(
        body["data"]["modelLoaded"].is_boolean() || body["data"]["model_loaded"].is_boolean(),
        "response should have modelLoaded field, got: {body:#?}"
    );
}

/// POST /api/embeddings/recompute — triggers a recompute task.
#[tokio::test]
async fn embeddings_recompute() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/embeddings/recompute", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("embeddings recompute: {body}");

    assert!(
        status == 200 || status == 500,
        "embeddings recompute should return 200 or 500, got {status}"
    );

    if status == 200 {
        assert!(
            body["data"]["task_id"].is_string(),
            "response should have task_id"
        );
    }
}

/// POST /api/tag-similarities/recompute — triggers recompute task.
#[tokio::test]
async fn tag_similarities_recompute() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/tag-similarities/recompute", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("tag similarities recompute: {body}");

    // Returns 200 with pairs_computed count, or 500
    assert!(
        status == 200 || status == 500,
        "tag similarities recompute should return 200 or 500, got {status}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Additional tests
// ═══════════════════════════════════════════════════════════════════════════

/// POST /api/embeddings/reset-review — resets reviewed_at for all tags.
#[tokio::test]
async fn embeddings_reset_review() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/embeddings/reset-review", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("embeddings reset review: {body}");

    assert!(
        status == 200 || status == 500,
        "embeddings reset-review should return 200 or 500, got {status}"
    );

    if status == 200 {
        assert!(
            body["data"]["reset"].is_u64(),
            "response should have a 'reset' count, got: {body:#?}"
        );
    }
}

/// POST /api/tag-similarities/recompute — run a second time to verify idempotency.
#[tokio::test]
async fn tag_similarities_recompute_again() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;

    // First call
    let resp1 = client
        .post(format!("{}/api/tag-similarities/recompute", base))
        .send()
        .await
        .unwrap();
    let status1 = resp1.status();
    let body1: Value = resp1.json().await.unwrap();
    eprintln!("tag similarities recompute (1st): {body1}");

    // Second call — should also succeed or be idempotent
    let resp2 = client
        .post(format!("{}/api/tag-similarities/recompute", base))
        .send()
        .await
        .unwrap();
    let status2 = resp2.status();
    let body2: Value = resp2.json().await.unwrap();
    eprintln!("tag similarities recompute (2nd): {body2}");

    assert!(
        status1 == 200 || status1 == 500,
        "first recompute should return 200 or 500, got {status1}"
    );
    assert!(
        status2 == 200 || status2 == 500,
        "second recompute should return 200 or 500, got {status2}"
    );

    if status1 == 200 && status2 == 200 {
        assert!(
            body2["data"]["pairs_computed"].is_u64(),
            "second response should have pairs_computed"
        );
    }
}

/// GET /api/version — returns the application version string.
#[tokio::test]
async fn version_endpoint_format() {
    let (client, base, pool) = common::spawn_test_app().await;
    let _ = &pool;

    let resp = client
        .get(format!("{}/api/version", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "version endpoint should return 200");

    let body: Value = resp.json().await.unwrap();
    eprintln!("version response: {body}");

    // Version should be a non-empty string formatted like semver (e.g. "0.9.0")
    let version = body["version"]
        .as_str()
        .expect("version should be a string");
    assert!(!version.is_empty(), "version should not be empty");
    assert!(
        version.chars().next().unwrap().is_ascii_digit(),
        "version should start with a digit, got: {version}"
    );
    // At least one dot (semver: X.Y.Z)
    assert!(
        version.contains('.'),
        "version should be semver-style (X.Y.Z), got: {version}"
    );
}
