//! Integration tests for `/api/dump` and `/api/restore` endpoints.

mod common;

/// GET /api/dump — returns 200 with Content-Type: application/json and valid JSON body.
#[tokio::test]
async fn dump_download() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/dump", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "dump should return 200");

    // Check Content-Type
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("json"),
        "content-type should contain 'json', got: '{content_type}'"
    );

    // Body should be valid JSON
    let body_text = resp.text().await.unwrap();
    eprintln!("dump body (first 200 chars): {}", &body_text[..body_text.len().min(200)]);

    let parsed: serde_json::Value =
        serde_json::from_str(&body_text).expect("dump body should be valid JSON");
    assert!(parsed.is_object(), "dump should be a JSON object");
}

/// POST /api/restore (without ?confirm=true) — returns 400.
#[tokio::test]
async fn restore_no_confirm() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Send a POST without ?confirm=true
    let resp = client
        .post(format!("{}/api/restore", base))
        .send()
        .await
        .unwrap();

    // restore_handler returns 400 when confirm is not true
    assert_eq!(
        resp.status(),
        400,
        "restore without confirm should return 400"
    );

    let body = resp.text().await.unwrap();
    eprintln!("restore response: {body}");
}
