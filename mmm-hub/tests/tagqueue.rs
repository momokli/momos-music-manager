//! Integration tests for the tag queue (issues #219/#220) + direct track tagging.

mod common;

#[tokio::test]
async fn tag_queue_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/tag-queue"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn tag_queue_renders_for_logged_in_user() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url("/tag-queue"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Tag-Queue"), "heading missing");
    assert!(html.contains("Ripeness"), "ripeness column missing");
    assert!(html.contains("max Ripeness"), "threshold filter missing");
}

#[tokio::test]
async fn detail_partial_renders_track() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url(&format!("/tag-queue/{}", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Taggen"), "tag form missing");
}

#[tokio::test]
async fn tagging_a_track_creates_a_manual_link_and_resolves() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    // Pre-create the tag so we tag an existing tag.
    let tag = mmm_hub::tags::ensure_tag(&app.pool, alice, "Dark")
        .await
        .unwrap();

    // Direct call surfaces any rebuild error (the HTTP path ignores it).
    mmm_hub::tags::tag_track(&app.pool, alice, track, tag)
        .await
        .expect("tag_track");

    let resp = app
        .client()
        .post(app.url(&format!("/tag-queue/{track}/tag")))
        .header("Cookie", &cookie)
        .form(&[("name", "Dark")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Dark"), "detail should show the new tag");

    let manual: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_tag_manual WHERE track_id = ?1 AND tag_id = ?2 AND user_id = ?3",
    )
    .bind(track)
    .bind(tag)
    .bind(alice)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(manual, 1, "manual link created");

    let resolved: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_resolved_tags WHERE track_id = ?1 AND tag_id = ?2",
    )
    .bind(track)
    .bind(tag)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(resolved, 1, "rebuild folded the manual link in");
}

#[tokio::test]
async fn stream_without_isrc_is_bad_request() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url(&format!("/track/{}/stream", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tag_queue_sort_is_selectable() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let html = app
        .client()
        .get(app.url("/tag-queue?sort=title"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Sortierung"), "sort control missing");
    assert!(
        html.contains("value=\"title\" selected"),
        "selected sort not reflected"
    );
}

#[tokio::test]
async fn tag_queue_tolerates_empty_max_param() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url("/tag-queue?q=&max=&sort=ripeness-desc"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
}
