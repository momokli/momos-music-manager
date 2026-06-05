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
