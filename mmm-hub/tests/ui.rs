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
    // The fixture playlist shows up, with both owner columns.
    assert!(html.contains("Deep House"));
    assert!(html.contains("Besitzer"));
    assert!(html.contains("Alice"), "spotify owner missing from table");
    assert!(html.contains("@alice"), "hub user column missing");
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
async fn overlap_page_renders_shared_data() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/overlap"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Entdecken"));
    // Fixture track shared by all three users.
    assert!(html.contains("Shared Anthem"));
    assert!(html.contains("@alice") && html.contains("@bob"));
}

#[tokio::test]
async fn settings_page_renders() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/settings"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Einstellungen"));
    assert!(html.contains("Spotify"));
    assert!(html.contains("/user/alice"));
}

#[tokio::test]
async fn playlist_filter_is_server_side() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // All playlists include the fixture playlist.
    let all = app
        .client()
        .get(app.url("/me/playlists?filter=all"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(body(all).await.contains("Deep House"));

    // No fixture playlist has an error, so the error filter yields no rows.
    let err = app
        .client()
        .get(app.url("/me/playlists?filter=error"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    let html = body(err).await;
    assert!(!html.contains("Deep House"));
    assert!(html.contains("Keine Playlists in dieser Ansicht"));
}

#[tokio::test]
async fn toggle_returns_row_fragment_for_htmx() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .post(app.url(&format!("/api/hub/playlists/{}/toggle", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .header("HX-Request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.trim_start().starts_with("<tr"), "expected a row fragment");
    assert!(html.contains("Deep House"));
}

#[tokio::test]
async fn toggle_without_htmx_redirects() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .post(app.url(&format!("/api/hub/playlists/{}/toggle", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/me/playlists");
}

#[tokio::test]
async fn sync_html_swaps_for_htmx_and_redirects_otherwise() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let htmx = app
        .client()
        .post(app.url("/api/hub/services/spotify/sync-html"))
        .header("Cookie", &cookie)
        .header("HX-Request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(htmx.status(), reqwest::StatusCode::OK);
    assert!(body(htmx).await.contains("synchronisiert"));

    let plain = app
        .client()
        .post(app.url("/api/hub/services/spotify/sync-html"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(plain.status(), reqwest::StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn track_page_distinguishes_owned_and_followed_playlists() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // t_pl is in alice's OWN "Deep House" and bob's FOLLOWED "Deep House".
    let resp = app
        .client()
        .get(app.url(&format!("/track/{}", app.seed.t_pl)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Eigene Playlists"), "own section missing");
    assert!(html.contains("Gefolgte Playlists"), "followed section missing");
    assert!(html.contains("hub-badge-ok"), "own badge missing");
    assert!(html.contains("hub-badge-no"), "followed badge missing");
    // Guard against unrendered askama placeholders leaking as literal text.
    assert!(!html.contains("{u."), "unrendered askama placeholder leaked");
}

#[tokio::test]
async fn playlist_detail_shows_both_owners() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url(&format!("/playlist/{}", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("@alice"), "hub user missing");
    assert!(html.contains("Spotify-Besitzer: Alice"), "spotify owner missing");
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
