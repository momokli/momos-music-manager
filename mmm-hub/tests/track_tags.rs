//! Integration tests for the track view as core element: direct tagging,
//! player availability, and recommended tags (issues: track tagging).

mod common;

#[tokio::test]
async fn track_tag_endpoint_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url(&format!("/track/{}/tag", app.seed.t_all)))
        .form(&[("name", "Dark")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn tagging_from_track_view_persists() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/tag")))
        .header("Cookie", &cookie)
        .form(&[("name", "Dark")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], format!("/track/{track}"));

    // The track page now shows the tag.
    let html = app
        .client()
        .get(app.url(&format!("/track/{track}")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Dark"), "tag not shown on track page");
    assert!(html.contains("Taggen"), "tag form missing");

    // Untag removes it again.
    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/untag")))
        .header("Cookie", &cookie)
        .form(&[("name", "Dark")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let manual: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_track_tag_manual WHERE track_id = ?1")
            .bind(track)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(manual, 0, "manual link removed");
}

#[tokio::test]
async fn recommended_tags_appear_from_cooccurrence() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;

    let dark = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    let mel = tags::ensure_tag(&app.pool, alice, "Melancholisch")
        .await
        .unwrap();
    // t_all has Dark; t_two has Dark + Melancholisch -> Melancholisch is
    // recommended for t_all (via the shared Dark tag).
    tags::tag_track(&app.pool, alice, app.seed.t_all, dark)
        .await
        .unwrap();
    tags::tag_track(&app.pool, alice, app.seed.t_two, dark)
        .await
        .unwrap();
    tags::tag_track(&app.pool, alice, app.seed.t_two, mel)
        .await
        .unwrap();

    let cookie = app.session_cookie(alice).await;
    let html = app
        .client()
        .get(app.url(&format!("/track/{}", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Empfohlen"), "recommendation section missing");
    assert!(html.contains("Melancholisch"), "recommended tag missing");
}
