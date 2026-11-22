//! Magic-link password reset: `/forgot` → mail → `/reset`.
//!
//! The harness installs a `RecordingMailer`, so the link is asserted from the
//! message the hub would have delivered — no SMTP server needed. Single-use,
//! expiry, purpose binding and session invalidation are all covered here.

mod common;

use reqwest::StatusCode;

/// Pull `token=…` out of the mailed link.
fn token_from(mail_body: &str) -> String {
    mail_body
        .split_once("token=")
        .map(|(_, rest)| {
            rest.trim()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
        })
        .filter(|t| !t.is_empty())
        .expect("mailed body contains a token")
}

async fn set_email(app: &common::TestApp, user_id: i64, email: &str) {
    sqlx::query("UPDATE hub_users SET email = ?2 WHERE id = ?1")
        .bind(user_id)
        .bind(email)
        .execute(&app.pool)
        .await
        .expect("set email");
}

async fn post_forgot(app: &common::TestApp, email: &str) -> reqwest::Response {
    app.client()
        .post(app.url("/forgot"))
        .form(&[("email", email)])
        .send()
        .await
        .unwrap()
}

async fn post_reset(
    app: &common::TestApp,
    token: &str,
    password: &str,
    confirm: &str,
) -> reqwest::Response {
    app.client()
        .post(app.url("/reset"))
        .form(&[
            ("token", token),
            ("new_password", password),
            ("confirm", confirm),
        ])
        .send()
        .await
        .unwrap()
}

/// Known and unknown addresses get the same status and the same redirect target.
#[tokio::test]
async fn forgot_answers_identically_for_known_and_unknown_addresses() {
    let app = common::spawn().await;
    let (uid, _) = app.user_with_password("dave", "oldpassword").await;
    set_email(&app, uid, "dave1@example.com").await;

    let known = post_forgot(&app, "dave1@example.com").await;
    assert_eq!(known.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&known), Some("/forgot"));

    let unknown = post_forgot(&app, "nobody1@example.com").await;
    assert_eq!(unknown.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&unknown), Some("/forgot"));
}

/// A reset request for a real address mails a link whose token works end-to-end.
#[tokio::test]
async fn full_reset_flow_with_mailed_link() {
    let app = common::spawn().await;
    let (uid, first_session) = app.user_with_password("dave", "oldpassword").await;
    set_email(&app, uid, "flow2@example.com").await;
    let second_session = app.session_cookie(uid).await;

    post_forgot(&app, "flow2@example.com").await;
    let body = app.mail.last_body().expect("one mail was sent");
    assert!(body.contains("30 Minuten"), "TTL is stated in the mail");
    let token = token_from(&body);

    // The link renders a form (and does not consume the token yet).
    let page = app
        .client()
        .get(app.url(&format!("/reset?token={token}")))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.text().await.unwrap().contains("name=\"new_password\""));

    let resp = post_reset(&app, &token, "brandnewpass", "brandnewpass").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), Some("/login"));

    // Credentials rotated …
    assert!(!logs_in(&app, "dave", "oldpassword").await);
    assert!(logs_in(&app, "dave", "brandnewpass").await);
    // … and every old session is dead (a reset is not a change).
    assert_eq!(me_status(&app, &first_session).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        me_status(&app, &second_session).await,
        StatusCode::UNAUTHORIZED
    );
}

/// The same link cannot be used twice.
#[tokio::test]
async fn reset_token_is_single_use() {
    let app = common::spawn().await;
    let (uid, _) = app.user_with_password("dave", "oldpassword").await;
    set_email(&app, uid, "dave3@example.com").await;

    post_forgot(&app, "dave3@example.com").await;
    let token = token_from(&app.mail.last_body().unwrap());

    let first = post_reset(&app, &token, "brandnewpass", "brandnewpass").await;
    assert_eq!(location(&first), Some("/login"));

    let again = post_reset(&app, &token, "anotherpass1", "anotherpass1").await;
    assert_eq!(again.status(), StatusCode::OK, "re-rendered, not consumed");
    let html = again.text().await.unwrap();
    assert!(html.contains("ungültig"), "should report the dead link");
    assert!(!logs_in(&app, "dave", "anotherpass1").await);
}

/// A wrong form (too short / mismatch) must not burn the token.
#[tokio::test]
async fn reset_keeps_the_token_usable_after_a_form_error() {
    let app = common::spawn().await;
    let (uid, _) = app.user_with_password("dave", "oldpassword").await;
    set_email(&app, uid, "dave4@example.com").await;

    post_forgot(&app, "dave4@example.com").await;
    let token = token_from(&app.mail.last_body().unwrap());

    let short = post_reset(&app, &token, "short", "short").await;
    assert_eq!(short.status(), StatusCode::OK);
    assert!(short.text().await.unwrap().contains("mindestens"));

    let mismatch = post_reset(&app, &token, "brandnewpass", "otherpass1").await;
    assert_eq!(mismatch.status(), StatusCode::OK);

    // Still redeemable.
    let ok = post_reset(&app, &token, "brandnewpass", "brandnewpass").await;
    assert_eq!(location(&ok), Some("/login"));
    assert!(logs_in(&app, "dave", "brandnewpass").await);
}

/// Unknown addresses also mint a token (so the work — and thus the timing — is
/// the same), but nothing is mailed.
#[tokio::test]
async fn unknown_address_mints_an_unusable_token_but_sends_nothing() {
    let app = common::spawn().await;

    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_email_tokens")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    post_forgot(&app, "nobody5@example.com").await;
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_email_tokens")
        .fetch_one(&app.pool)
        .await
        .unwrap();

    assert_eq!(after, before + 1, "token is minted either way");
    assert_eq!(app.mail.count(), 0, "but no mail leaves the hub");
    let unbound: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_email_tokens WHERE user_id IS NULL")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(unbound, 1);
}

/// Tokens are bound to their purpose and to the address they were minted for.
#[tokio::test]
async fn tokens_are_bound_to_purpose_and_address() {
    let app = common::spawn().await;
    let (uid, _) = app.user_with_password("dave", "oldpassword").await;
    set_email(&app, uid, "dave6@example.com").await;

    // A `register` token must not work as a reset token.
    let register_token = music_hub::mail_tokens::issue(
        &app.pool,
        Some(uid),
        "dave6@example.com",
        music_hub::mail_tokens::PURPOSE_REGISTER,
        chrono::Duration::days(1),
        None,
    )
    .await
    .unwrap();
    let wrong_purpose = post_reset(&app, &register_token, "brandnewpass", "brandnewpass").await;
    assert_eq!(wrong_purpose.status(), StatusCode::OK);
    assert!(!logs_in(&app, "dave", "brandnewpass").await);

    // A link mailed to the old address dies when the address changes.
    post_forgot(&app, "dave6@example.com").await;
    let token = token_from(&app.mail.last_body().unwrap());
    set_email(&app, uid, "dave6@example.org").await;
    let stale = post_reset(&app, &token, "brandnewpass", "brandnewpass").await;
    assert_eq!(stale.status(), StatusCode::OK);
    assert!(stale.text().await.unwrap().contains("passt nicht mehr"));
}

/// An expired token is rejected, even though it was never used.
#[tokio::test]
async fn expired_token_is_rejected() {
    let app = common::spawn().await;
    let (uid, _) = app.user_with_password("dave", "oldpassword").await;
    set_email(&app, uid, "dave7@example.com").await;

    sqlx::query(
        "INSERT INTO hub_email_tokens
             (user_id, email, purpose, token_hash, expires_at, created_at)
         VALUES (?1, 'dave7@example.com', 'reset', ?2, '2020-01-01T00:00:00+00:00',
                 '2020-01-01T00:00:00+00:00')",
    )
    .bind(uid)
    .bind(music_hub::mail_tokens::hash_token("expired-token"))
    .execute(&app.pool)
    .await
    .unwrap();

    let page = app
        .client()
        .get(app.url("/reset?token=expired-token"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.text().await.unwrap().contains("ungültig"));

    let resp = post_reset(&app, "expired-token", "brandnewpass", "brandnewpass").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!logs_in(&app, "dave", "brandnewpass").await);
}

/// The second request for the same address inside the throttle window is a
/// silent no-op (no second mail, same answer).
#[tokio::test]
async fn repeated_forgot_requests_are_throttled() {
    let app = common::spawn().await;
    let (uid, _) = app.user_with_password("dave", "oldpassword").await;
    set_email(&app, uid, "dave8@example.com").await;

    let first = post_forgot(&app, "dave8@example.com").await;
    let second = post_forgot(&app, "dave8@example.com").await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    assert_eq!(second.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&first), location(&second));
    assert_eq!(app.mail.count(), 1, "throttled request mails nothing");
}

/// Garbage in the address field is rejected before any token is minted.
#[tokio::test]
async fn malformed_address_is_rejected() {
    let app = common::spawn().await;
    let resp = post_forgot(&app, "definitely-not-an-address").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(app.mail.count(), 0);

    let tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_email_tokens")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(tokens, 0);
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn location(resp: &reqwest::Response) -> Option<&str> {
    resp.headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
}

async fn logs_in(app: &common::TestApp, slug: &str, password: &str) -> bool {
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
