//! "Now playing" widget tests (issue #293/#294).
//!
//! The widget itself polls Spotify; integration tests cover the auth gate, the
//! scope gate (which never touches the network) and the inline tagging POST.

mod common;

use sqlx::Row;

#[tokio::test]
async fn now_playing_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/now-playing"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn now_playing_prompts_reconnect_without_scope() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("zoe", "pw").await;
    let resp = app
        .client()
        .get(app.url("/now-playing"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("neu verbinden"),
        "should prompt to reconnect: {html}"
    );
    assert!(
        html.contains("/api/hub/services/spotify/connect"),
        "should link the connect endpoint: {html}"
    );
}

#[tokio::test]
async fn now_tag_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url("/now-playing/tag"))
        .form(&[("track_id", "1"), ("name", "x")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn now_tag_creates_and_links_tag() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    let track = app.seed.t_all;

    let resp = app
        .client()
        .post(app.url("/now-playing/tag"))
        .header("Cookie", &cookie)
        .form(&[
            ("track_id", track.to_string()),
            ("name", "nowtag".to_string()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    // htmx toast trigger present on success.
    assert!(resp.headers().contains_key("HX-Trigger"));

    let tag_id: i64 =
        sqlx::query("SELECT id FROM hub_tags WHERE owner_user_id = ?1 AND name = 'nowtag'")
            .bind(uid)
            .fetch_one(&app.pool)
            .await
            .expect("tag created")
            .get("id");

    let linked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_tag_manual WHERE track_id = ?1 AND tag_id = ?2",
    )
    .bind(track)
    .bind(tag_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(linked, 1, "tag should be linked to the track");
}

#[tokio::test]
async fn now_tag_without_hub_track_reports_error() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;

    let resp = app
        .client()
        .post(app.url("/now-playing/tag"))
        .header("Cookie", &cookie)
        .form(&[("track_id", "0"), ("name", "nope")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_tags WHERE owner_user_id = ?1")
        .bind(uid)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "no tag should be created without a hub track");
}
