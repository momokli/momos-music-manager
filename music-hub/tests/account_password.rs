//! Password hygiene: self-service password change (`/account/password`) and the
//! opportunistic bcrypt-cost upgrade on login (issue #556 groundwork).
//!
//! The harness client does not follow redirects, so a `303` plus `Location`
//! proves the PRG flow, and `Set-Cookie` proves whether a session was issued.

mod common;

use reqwest::StatusCode;

/// Guarded pages redirect unauthenticated visitors instead of rendering.
#[tokio::test]
async fn password_change_requires_a_session() {
    let app = common::spawn().await;

    let resp = app
        .client()
        .get(app.url("/account/password"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/login")
    );

    let resp = app
        .client()
        .post(app.url("/account/password"))
        .form(&[
            ("current", "whatever"),
            ("new_password", "newpassword"),
            ("confirm", "newpassword"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

/// A wrong current password changes nothing: the old password keeps working.
#[tokio::test]
async fn password_change_rejects_a_wrong_current_password() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("dave", "oldpassword").await;

    let resp = change(&app, &cookie, "not-my-password", "newpassword", "newpassword").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // Old password still valid, new one not.
    assert!(login_sets_cookie(&app, "dave", "oldpassword").await);
    assert!(!login_sets_cookie(&app, "dave", "newpassword").await);
}

/// Validation failures never touch the stored hash.
#[tokio::test]
async fn password_change_validates_length_and_confirmation() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("dave", "oldpassword").await;

    // Too short.
    let resp = change(&app, &cookie, "oldpassword", "short", "short").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // Confirmation mismatch.
    let resp = change(&app, &cookie, "oldpassword", "newpassword", "newpassword2").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    // Unchanged password.
    let resp = change(&app, &cookie, "oldpassword", "oldpassword", "oldpassword").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    assert!(login_sets_cookie(&app, "dave", "oldpassword").await);
}

/// Success rotates the hash, keeps the acting session and ends every other one.
#[tokio::test]
async fn password_change_ends_other_sessions_and_keeps_the_current_one() {
    let app = common::spawn().await;
    let (uid, cookie_a) = app.user_with_password("dave", "oldpassword").await;
    let cookie_b = app.session_cookie(uid).await;

    let resp = change(&app, &cookie_a, "oldpassword", "newpassword", "newpassword").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/account/password")
    );

    // The session that made the change survives.
    assert_eq!(me_status(&app, &cookie_a).await, StatusCode::OK);
    // The other one is gone.
    assert_eq!(
        me_status(&app, &cookie_b).await,
        StatusCode::UNAUTHORIZED
    );

    // Credentials rotated: old password dead, new one live.
    assert!(!login_sets_cookie(&app, "dave", "oldpassword").await);
    assert!(login_sets_cookie(&app, "dave", "newpassword").await);

    let hash: String = sqlx::query_scalar("SELECT password_hash FROM hub_users WHERE id = ?1")
        .bind(uid)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        music_hub::auth::hash_cost(&hash),
        Some(music_hub::auth::BCRYPT_COST),
        "changed passwords must be written with the current work factor"
    );
}

/// A hash below the current work factor is upgraded during login — the only
/// moment the plaintext is available. Login itself must not break.
#[tokio::test]
async fn login_upgrades_a_stale_hash() {
    let app = common::spawn().await;
    let (uid, _) = app.user_with_password("erin", "correct-horse").await;

    let before: String = sqlx::query_scalar("SELECT password_hash FROM hub_users WHERE id = ?1")
        .bind(uid)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        music_hub::auth::hash_cost(&before),
        Some(4),
        "fixture is written with a deliberately weak cost"
    );

    assert!(login_sets_cookie(&app, "erin", "correct-horse").await);

    let after: String = sqlx::query_scalar("SELECT password_hash FROM hub_users WHERE id = ?1")
        .bind(uid)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        music_hub::auth::hash_cost(&after),
        Some(music_hub::auth::BCRYPT_COST)
    );
    // Still the same password.
    assert!(login_sets_cookie(&app, "erin", "correct-horse").await);
}

/// The page renders inside the shell for a signed-in user.
#[tokio::test]
async fn password_page_renders_for_a_session() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("dave", "oldpassword").await;

    let resp = app
        .client()
        .get(app.url("/account/password"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("name=\"new_password\""), "form is rendered");
    assert!(
        html.contains("name=\"current\""),
        "current password is required"
    );
}

/// The settings page (the only place users look) links to the change form.
#[tokio::test]
async fn settings_page_links_to_the_password_form() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("dave", "oldpassword").await;

    let html = app
        .client()
        .get(app.url("/settings"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        html.contains("href=\"/account/password\""),
        "settings must link to the password page"
    );
}

// ── helpers ─────────────────────────────────────────────────────────────────

async fn change(
    app: &common::TestApp,
    cookie: &str,
    current: &str,
    new_password: &str,
    confirm: &str,
) -> reqwest::Response {
    app.client()
        .post(app.url("/account/password"))
        .header(reqwest::header::COOKIE, cookie)
        .form(&[
            ("current", current),
            ("new_password", new_password),
            ("confirm", confirm),
        ])
        .send()
        .await
        .unwrap()
}

/// `true` when the login issued a session cookie.
async fn login_sets_cookie(app: &common::TestApp, slug: &str, password: &str) -> bool {
    let resp = app
        .client()
        .post(app.url("/login"))
        .form(&[("username", slug), ("password", password)])
        .send()
        .await
        .unwrap();
    resp.headers()
        .get(reqwest::header::SET_COOKIE)
        .is_some()
}

async fn me_status(app: &common::TestApp, cookie: &str) -> StatusCode {
    app.client()
        .get(app.url("/api/hub/me"))
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .status()
}
