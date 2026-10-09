//! UI shell + navigation integration tests (issue #159).
//!
//! Verifies every shell page renders through the unified topbar: guarded pages
//! redirect to `/login` without a session, and authenticated pages carry the
//! nav, the user slug and an `aria-current` marker on the active item.

mod common;

async fn body(resp: reqwest::Response) -> String {
    resp.text().await.expect("response body")
}

#[tokio::test]
async fn dashboard_requires_login() {
    let app = common::spawn().await;
    let resp = app.client().get(app.url("/")).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn guarded_shell_pages_require_login() {
    let app = common::spawn().await;
    for path in ["/me/playlists", "/sql", "/search"] {
        let resp = app.client().get(app.url(path)).send().await.unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::SEE_OTHER,
            "{path} should redirect when logged out"
        );
    }
}

#[tokio::test]
async fn dashboard_renders_shell_and_active_nav() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;

    // Shell: brand, nav items, user menu.
    assert!(html.contains("MMM Hub"), "brand missing");
    assert!(html.contains("Übersicht"), "nav item missing");
    assert!(html.contains("/me/playlists"), "playlists nav missing");
    assert!(html.contains("@alice"), "user slug missing");
    assert!(html.contains("Logout"), "logout missing");
    // The dashboard item is the active one.
    assert!(
        html.contains("aria-current=\"page\""),
        "active nav marker missing"
    );
}

#[tokio::test]
async fn playlists_page_renders() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/me/playlists"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Meine Playlists"));
    assert!(html.contains("aria-current=\"page\""));
    // The fixture playlist shows up.
    assert!(html.contains("Deep House"));
}

#[tokio::test]
async fn track_page_renders_in_shell() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url(&format!("/track/{}", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Shared Anthem"));
    assert!(html.contains("MMM Hub"), "shell missing on track page");
    assert!(html.contains("@alice"));
}

#[tokio::test]
async fn login_page_renders_without_session() {
    let app = common::spawn().await;
    let resp = app.client().get(app.url("/login")).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("action=\"/login\""));
    assert!(html.contains("Registrieren"));
}

#[tokio::test]
async fn no_shell_page_returns_500() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    for path in ["/", "/me/playlists", "/sql", "/search?q=shared", "/login"] {
        let resp = app
            .client()
            .get(app.url(path))
            .header("Cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert!(
            resp.status().is_success() || resp.status().is_redirection(),
            "{path} returned {}",
            resp.status()
        );
    }
}
