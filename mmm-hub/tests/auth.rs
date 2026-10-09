//! Web auth integration tests (issue #131).
//!
//! Proves the login → session → guarded-route chain, plus the signup flow,
//! the wrong-password path, and case-insensitive usernames. The harness client
//! does not follow redirects, so `Set-Cookie` and `Location` are readable.

mod common;

use reqwest::StatusCode;

const SESSION_COOKIE: &str = "hub_session";

/// Pull the `hub_session=<token>` pair out of a `Set-Cookie` header value.
fn session_cookie_from(resp: &reqwest::Response) -> String {
    let raw = resp
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .expect("Set-Cookie header present")
        .to_str()
        .expect("Set-Cookie is ASCII");
    assert!(
        raw.starts_with(&format!("{SESSION_COOKIE}=")),
        "unexpected Set-Cookie: {raw}"
    );
    raw.split(';').next().unwrap().to_string()
}

/// Unauthenticated dashboard access redirects to the login page.
#[tokio::test]
async fn dashboard_without_cookie_redirects_to_login() {
    let app = common::spawn().await;

    let resp = app.client().get(app.url("/")).send().await.unwrap();

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

/// Signup issues a session cookie that grants access to the dashboard, and
/// `/api/hub/me` reports the freshly-created user.
#[tokio::test]
async fn signup_sets_session_and_unlocks_dashboard_and_me() {
    let app = common::spawn().await;
    // Registration is closed by default; open it for this test.
    mmm_hub::settings::set(&app.pool, "registration_open", "1")
        .await
        .unwrap();

    let resp = app
        .client()
        .post(app.url("/signup"))
        .form(&[("username", "dave"), ("password", "secretpw")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = session_cookie_from(&resp);

    // Dashboard now renders (200) instead of redirecting.
    let resp = app
        .client()
        .get(app.url("/"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The guarded JSON endpoint identifies the signed-up user.
    let resp = app
        .client()
        .get(app.url("/api/hub/me"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["slug"], "dave");
}

/// The guarded endpoint rejects requests with no session cookie.
#[tokio::test]
async fn me_without_cookie_is_unauthorized() {
    let app = common::spawn().await;

    let resp = app
        .client()
        .get(app.url("/api/hub/me"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// A wrong password re-renders the login form (200), not a redirect.
#[tokio::test]
async fn login_with_wrong_password_rerenders_form() {
    let app = common::spawn().await;
    let _ = app
        .user_with_password("erin", "correct-horse")
        .await;

    let resp = app
        .client()
        .post(app.url("/login"))
        .form(&[("username", "erin"), ("password", "wrong")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get(reqwest::header::SET_COOKIE).is_none(),
        "a failed login must not set a session cookie"
    );
}

/// Usernames are matched case-insensitively on login.
#[tokio::test]
async fn login_username_is_case_insensitive() {
    let app = common::spawn().await;
    let _ = app
        .user_with_password("CarolTest", "carolpass")
        .await;

    let resp = app
        .client()
        .post(app.url("/login"))
        .form(&[("username", "caroltest"), ("password", "carolpass")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = session_cookie_from(&resp);

    let resp = app
        .client()
        .get(app.url("/api/hub/me"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["slug"], "CarolTest");
}
