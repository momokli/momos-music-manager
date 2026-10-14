//! Integration tests for the Hub ops console (issue #350): admin gating,
//! worker pause/resume, failed-job retry and the event-log fragment.

mod common;

use reqwest::StatusCode;

async fn body(resp: reqwest::Response) -> String {
    resp.text().await.expect("response body")
}

async fn make_admin(app: &common::TestApp) {
    sqlx::query("UPDATE hub_users SET is_admin = 1 WHERE id = ?1")
        .bind(app.seed.alice)
        .execute(&app.pool)
        .await
        .unwrap();
}

/// Response header value as a `String` (empty when absent).
fn header(resp: &reqwest::Response, name: &str) -> String {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn pause_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url("/api/hub/ops/workers/spotify/pause"))
        .send()
        .await
        .expect("pause request");
    // Guard convention: redirect to /login when there is no session.
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn pause_resume_admin_gated_and_visible() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Non-admin: forbidden.
    let denied = app
        .client()
        .post(app.url("/api/hub/ops/workers/spotify/pause"))
        .header("Cookie", &cookie)
        .header("HX-Request", "true")
        .send()
        .await
        .expect("pause request");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    make_admin(&app).await;

    // Admin + htmx: 204 with a toast, and the loop reads as paused.
    let paused = app
        .client()
        .post(app.url("/api/hub/ops/workers/spotify/pause"))
        .header("Cookie", &cookie)
        .header("HX-Request", "true")
        .send()
        .await
        .expect("pause request");
    assert_eq!(paused.status(), StatusCode::NO_CONTENT);
    let toast = header(&paused, "hx-trigger");
    assert!(toast.contains("pausiert"), "toast: {toast}");

    let panel = body(
        app.client()
            .get(app.url("/api/hub/status/panel"))
            .header("Cookie", &cookie)
            .send()
            .await
            .expect("panel request"),
    )
    .await;
    assert!(panel.contains("pausiert"), "panel shows paused badge");

    // Resume flips it back.
    let resumed = app
        .client()
        .post(app.url("/api/hub/ops/workers/spotify/resume"))
        .header("Cookie", &cookie)
        .header("HX-Request", "true")
        .send()
        .await
        .expect("resume request");
    assert_eq!(resumed.status(), StatusCode::NO_CONTENT);
    let panel = body(
        app.client()
            .get(app.url("/api/hub/status/panel"))
            .header("Cookie", &cookie)
            .send()
            .await
            .expect("panel request"),
    )
    .await;
    assert!(!panel.contains("pausiert"), "paused badge cleared");
}

#[tokio::test]
async fn retry_requeues_failed_jobs() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    make_admin(&app).await;

    // One failed playlist (fail() zeroes enabled_for_fetch) and one failed likes job.
    let pid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_playlists
            (user_id, service, playlist_id, name, items_available, enabled_for_fetch, fetch_status, fetch_error)
         VALUES (?1, 'spotify', 'pl-err', 'Broken', 0, 0, 'error', 'boom')
         RETURNING id",
    )
    .bind(app.seed.alice)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO hub_service_accounts (user_id, service, likes_status, likes_error, access_token)
         VALUES (?1, 'spotify', 'error', 'boom', 'tok')",
    )
    .bind(app.seed.bob)
    .execute(&app.pool)
    .await
    .unwrap();

    let resp = app
        .client()
        .post(app.url("/api/hub/ops/retry"))
        .header("Cookie", &cookie)
        .header("HX-Request", "true")
        .send()
        .await
        .expect("retry request");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let toast = header(&resp, "hx-trigger");
    assert!(
        toast.contains("1 Playlists, 1 Likes"),
        "retry toast counts: {toast}"
    );

    // Playlist requeued and re-enabled for fetch (risk 3).
    let (status, error, enabled): (String, Option<String>, i64) = sqlx::query_as(
        "SELECT fetch_status, fetch_error, enabled_for_fetch FROM hub_playlists WHERE id = ?1",
    )
    .bind(pid)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(status, "queued");
    assert_eq!(error, None);
    assert_eq!(enabled, 1, "enabled_for_fetch reset so the worker picks it up");

    // Likes job requeued with the error cleared.
    let (likes_status, likes_error): (String, Option<String>) = sqlx::query_as(
        "SELECT likes_status, likes_error FROM hub_service_accounts WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(app.seed.bob)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(likes_status, "queued");
    assert_eq!(likes_error, None);

    // Idempotent: a second run finds nothing to do.
    let again = app
        .client()
        .post(app.url("/api/hub/ops/retry"))
        .header("Cookie", &cookie)
        .header("HX-Request", "true")
        .send()
        .await
        .expect("retry request");
    assert_eq!(again.status(), StatusCode::NO_CONTENT);
    assert!(header(&again, "hx-trigger").contains("Nichts zu tun"));
}

#[tokio::test]
async fn logs_fragment_is_guard_gated() {
    let app = common::spawn().await;

    // No session → 401 (fragment convention).
    let anon = app
        .client()
        .get(app.url("/api/hub/ops/logs"))
        .send()
        .await
        .expect("logs request");
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);

    let cookie = app.session_cookie(app.seed.alice).await;

    // Logged in but not admin → 403.
    let denied = app
        .client()
        .get(app.url("/api/hub/ops/logs"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("logs request");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    // Admin → empty state (no loops spawned in tests).
    make_admin(&app).await;
    let ok = app
        .client()
        .get(app.url("/api/hub/ops/logs"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("logs request");
    assert_eq!(ok.status(), StatusCode::OK);
    assert!(body(ok).await.contains("Keine Ereignisse."));
}
