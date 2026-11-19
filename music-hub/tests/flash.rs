//! Flash-cookie behaviour for full-page POSTs (issue #622).
//!
//! htmx actions keep their `HX-Trigger` toast; a full-page form post carries the
//! message in the short-lived `hub_flash` cookie. The middleware consumes it
//! once (rendering the toast) and clears it, so a reload shows no toast.

mod common;

use axum::http::StatusCode;

/// All `hub_flash` Set-Cookie values on a response.
fn flash_cookies(resp: &reqwest::Response) -> Vec<String> {
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter(|v| v.starts_with("hub_flash="))
        .map(|v| v.to_string())
        .collect()
}

/// The first `hub_flash` Set-Cookie value (empty when none was set).
fn first_flash(resp: &reqwest::Response) -> String {
    flash_cookies(resp).into_iter().next().unwrap_or_default()
}

/// htmx POST: toast rides `HX-Trigger`, no flash cookie is set.
#[tokio::test]
async fn htmx_post_uses_hx_trigger_and_sets_no_flash_cookie() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .post(app.url("/music-api/refresh"))
        .header("Cookie", &cookie)
        .header("HX-Request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let trigger = resp
        .headers()
        .get("hx-trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(trigger.contains("\"toast\""), "hx-trigger toast: {trigger}");
    assert!(
        flash_cookies(&resp).is_empty(),
        "htmx must not set a flash cookie"
    );
}

/// Full-page POST: 303 + flash cookie; the following GET renders it and clears
/// the cookie.
#[tokio::test]
async fn full_page_post_sets_flash_then_get_consumes_and_clears() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .post(app.url("/music-api/refresh"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/music-api");
    let set = first_flash(&resp);
    assert!(set.starts_with("hub_flash="), "flash cookie set: {set}");
    // `hub_flash=<value>` for the follow-up GET.
    let kv = set.split(';').next().unwrap().to_string();

    let page = app
        .client()
        .get(app.url("/music-api"))
        .header("Cookie", format!("{cookie}; {kv}"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let clear = first_flash(&page);
    assert!(clear.starts_with("hub_flash=;"), "cookie cleared: {clear}");
    assert!(clear.contains("Max-Age=0"), "cleared immediately: {clear}");
    let body = page.text().await.unwrap();
    assert!(
        body.contains("id=\"hub-flash-init\""),
        "flash block rendered"
    );
    assert!(
        body.contains("data-msg=\"music-api nicht konfiguriert\""),
        "flash text rendered"
    );
}

/// A plain reload (no flash cookie) shows no toast and sets no clear header.
#[tokio::test]
async fn reload_without_flash_cookie_shows_no_toast() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let page = app
        .client()
        .get(app.url("/music-api"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(
        flash_cookies(&page).is_empty(),
        "no clear cookie without a flash cookie"
    );
    let body = page.text().await.unwrap();
    assert!(
        !body.contains("id=\"hub-flash-init\""),
        "no toast on a plain reload"
    );
}
