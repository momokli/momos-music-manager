//! Ops status surface tests (issue #353): `GET /api/hub/status` shape/defaults,
//! session gating, liveness regression and the admin-only HTML panel.

mod common;

use serde_json::Value;

async fn body(resp: reqwest::Response) -> String {
    resp.text().await.expect("response body")
}

#[tokio::test]
async fn status_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/api/hub/status"))
        .send()
        .await
        .expect("status request");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn status_shape_and_defaults() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/api/hub/status"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("status request");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: Value = resp.json().await.expect("status json");

    let data = &body["data"];
    assert!(data["status"].is_string(), "data.status is a string");
    assert!(data["version"].is_string());
    assert!(data["gitSha"].is_string());
    assert!(data["describe"].is_string());
    assert!(data["startedAt"].is_string(), "startedAt stamped by registry");
    assert!(data["uptimeSecs"].is_number());

    // All five Hub loops are registered and reported.
    let workers = data["hub"]["workers"].as_array().expect("workers array");
    assert_eq!(workers.len(), 5, "five loops reported");
    for w in workers {
        assert!(w["name"].is_string(), "loop name");
        assert!(w["lastWorked"].is_boolean());
        assert!(w["iterations"].is_number());
        assert!(w.get("lastRunAt").is_some(), "lastRunAt key present");
        assert!(w.get("lastError").is_some(), "lastError key present");
    }

    // Backlog numbers are present and numeric.
    let backlog = &data["hub"]["backlog"];
    for k in [
        "featuresPending",
        "featureRequests",
        "genresPending",
        "crawlPending",
        "curationDue",
        "likesQueued",
        "playlistsPending",
    ] {
        assert!(backlog[k].is_number(), "{k} is a number");
    }

    // Tests have no music-api token → complete block, configured/reachable false.
    let m = &data["musicApi"];
    assert_eq!(m["configured"], Value::Bool(false));
    assert_eq!(m["reachable"], Value::Bool(false));
    assert_eq!(m["worker"]["ok"], Value::Bool(false));
    assert_eq!(m["queue"]["counts"], Value::Array(vec![]));
    assert_eq!(m["queue"]["depth"], Value::from(0));
}

#[tokio::test]
async fn health_unchanged() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/api/hub/health"))
        .send()
        .await
        .expect("health request");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: Value = resp.json().await.expect("health json");
    assert_eq!(body["data"]["status"], "ok");
}

#[tokio::test]
async fn status_panel_admin_only() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Non-admin: forbidden.
    let denied = app
        .client()
        .get(app.url("/api/hub/status/panel"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("panel request");
    assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);

    // Admin: renders the fragment.
    sqlx::query("UPDATE hub_users SET is_admin = 1 WHERE id = ?1")
        .bind(app.seed.alice)
        .execute(&app.pool)
        .await
        .unwrap();
    let ok = app
        .client()
        .get(app.url("/api/hub/status/panel"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("panel request");
    assert_eq!(ok.status(), reqwest::StatusCode::OK);
    let html = body(ok).await;
    assert!(html.contains("Hub-Worker"), "panel shows the worker block");
}
