//! Harness self-check: the app boots, migrations run, fixtures seed, and the
//! public health endpoint answers without a session.

mod common;

#[tokio::test]
async fn boot_and_health() {
    let app = common::spawn().await;

    let resp = app
        .client()
        .get(app.url("/api/hub/health"))
        .send()
        .await
        .expect("health request");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.expect("health json");
    assert_eq!(body["data"]["status"], "ok");

    // Fixtures landed.
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_users")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(users, 3);
}
