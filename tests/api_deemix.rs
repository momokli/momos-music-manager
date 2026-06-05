//! Integration tests for `/api/services/deemix/*` endpoints.
//!
//! Tests the Deemix queue management endpoints. In the test environment,
//! there's no Deemix server running, so most endpoints return errors.
//! We verify the error responses and 404 handling.

mod common;

use serde_json::Value;

/// GET /api/services/deemix/queue — returns queue (empty or error).
#[tokio::test]
async fn deemix_queue_list() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/services/deemix/queue", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("deemix queue list: {body}");

    assert!(
        status == 200 || status == 500,
        "deemix queue list should return 200 or 500, got {status}"
    );
}

/// POST /api/services/deemix/queue — add to queue (error: no server).
#[tokio::test]
async fn deemix_queue_add_error() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/services/deemix/queue", base))
        .json(&serde_json::json!({"url": ""}))
        .send()
        .await
        .unwrap();

    // Empty URL → 400 BAD_REQUEST
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("deemix queue add error: {body}");

    assert!(
        status == 400 || status == 500,
        "deemix queue add with empty URL should return 400 or 500, got {status}"
    );
}

/// POST /api/services/deemix/queue/999/retry — retry non-existent → 404.
#[tokio::test]
async fn deemix_queue_retry_404() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/services/deemix/queue/999/retry", base))
        .send()
        .await
        .unwrap();

    // Non-existent download → 404 NOT_FOUND
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("deemix queue retry 404: {body}");

    assert_eq!(
        status, 404,
        "retry non-existent should return 404, got {status}"
    );
    assert!(
        body["error"].is_string() || body["data"].is_string(),
        "404 response should have an error or data field, got: {body}"
    );
}

/// DELETE /api/services/deemix/queue/999 — delete non-existent → 404.
#[tokio::test]
async fn deemix_queue_delete_404() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .delete(format!("{}/api/services/deemix/queue/999", base))
        .send()
        .await
        .unwrap();

    // Non-existent download → 404 NOT_FOUND
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("deemix queue delete 404: {body}");

    assert_eq!(
        status, 404,
        "delete non-existent should return 404, got {status}"
    );
    assert!(
        body["error"].is_string() || body["data"].is_string(),
        "404 response should have an error or data field, got: {body}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Validation tests
// ═══════════════════════════════════════════════════════════════════════════

/// POST /api/services/deemix/queue/{string}/retry — invalid numeric ID → 422.
#[tokio::test]
async fn deemix_queue_retry_validation() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!(
            "{}/api/services/deemix/queue/nonexistent/retry",
            base
        ))
        .send()
        .await
        .unwrap();

    // Non-numeric ID cannot be parsed as i64 → 400 Bad Request
    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    eprintln!("deemix queue retry validation status: {status}, body: {body_text:?}");

    assert!(
        status == 400 || status == 422,
        "retry with non-numeric ID should return 400 or 422, got {status}"
    );
    assert!(
        body_text.to_lowercase().contains("cannot parse")
            || body_text.to_lowercase().contains("invalid"),
        "response should indicate parsing failure, got: {body_text:?}"
    );
}

/// DELETE /api/services/deemix/queue/{string} — invalid numeric ID → 400.
#[tokio::test]
async fn deemix_queue_delete_validation() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .delete(format!("{}/api/services/deemix/queue/nonexistent", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    eprintln!("deemix queue delete validation status: {status}, body: {body_text:?}");

    assert!(
        status == 400 || status == 422,
        "delete with non-numeric ID should return 400 or 422, got {status}"
    );
    assert!(
        body_text.to_lowercase().contains("cannot parse")
            || body_text.to_lowercase().contains("invalid"),
        "response should indicate parsing failure, got: {body_text:?}"
    );
}
