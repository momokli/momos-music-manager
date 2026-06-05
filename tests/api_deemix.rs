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
