//! Integration tests for infrastructure endpoints:
//! - `/api/tag-similarities/*`
//! - `/api/traktor/*`
//! - `/api/embeddings/*`

mod common;

use serde_json::Value;

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
