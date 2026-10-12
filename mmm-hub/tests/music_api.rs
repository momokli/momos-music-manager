//! Integration tests for the native music-api ops console (#native integration).

mod common;

async fn get(app: &common::TestApp, cookie: &str, path: &str) -> String {
    let resp = app
        .client()
        .get(app.url(path))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "GET {path}");
    resp.text().await.unwrap()
}

#[tokio::test]
async fn ops_page_and_fragments_render() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // No music-api token in tests -> page shows the "not configured" hint.
    let html = get(&app, &cookie, "/music-api").await;
    assert!(html.contains("music-api"), "page title missing");
    assert!(
        html.contains("nicht konfiguriert") || html.contains("Live-Queue"),
        "expected the configured or unconfigured branch"
    );

    // Fragments render even without a token (empty states).
    let queue = get(&app, &cookie, "/music-api/queue").await;
    assert!(
        queue.contains("Queue leer"),
        "empty queue fragment: {queue}"
    );
    let logs = get(&app, &cookie, "/music-api/logs").await;
    assert!(logs.contains("Keine Ereignisse"), "empty logs fragment");
}

#[tokio::test]
async fn ops_fragments_require_login() {
    let app = common::spawn().await;
    // Page redirects to login; fragments answer 401.
    let page = app
        .client()
        .get(app.url("/music-api"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), reqwest::StatusCode::SEE_OTHER);
    for path in [
        "/music-api/queue",
        "/music-api/logs",
        "/music-api/worker",
        "/music-api/orders",
        "/track/1/music-api",
    ] {
        let resp = app.client().get(app.url(path)).send().await.unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "{path} should require login"
        );
    }
}

#[tokio::test]
async fn worker_and_orders_fragments_render() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // No token -> worker reads as running, orders list is empty.
    let worker = get(&app, &cookie, "/music-api/worker").await;
    assert!(worker.contains("Worker"), "worker fragment: {worker}");
    let orders = get(&app, &cookie, "/music-api/orders").await;
    assert!(orders.contains("Orders"), "orders fragment: {orders}");
}

#[tokio::test]
async fn track_panel_renders_for_seeded_track() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let path = format!("/track/{}/music-api", app.seed.t_all);
    let html = get(&app, &cookie, &path).await;
    assert!(html.contains("music-api"), "panel rendered: {html}");
}

#[tokio::test]
async fn action_requires_login() {
    let app = common::spawn().await;
    for path in [
        "/music-api/worker/pause",
        "/music-api/worker/resume",
        "/music-api/order/cancel",
        "/music-api/order/prioritize",
    ] {
        let resp = app
            .client()
            .post(app.url(path))
            .form(&[("id", "x")])
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::SEE_OTHER,
            "{path} should redirect to login"
        );
    }
}
