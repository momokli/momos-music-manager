//! Integration tests for the ripeness score endpoint (issues #214/#217).

mod common;

#[tokio::test]
async fn ripeness_endpoint_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url(&format!("/api/hub/tracks/{}/ripeness", app.seed.t_all)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ripeness_scores_two_tags_in_one_group_as_150() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;

    let g = tags::create_group(&app.pool, alice, "Mood", "")
        .await
        .unwrap();
    let dark = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    let mel = tags::ensure_tag(&app.pool, alice, "Melancholisch")
        .await
        .unwrap();
    for t in [dark, mel] {
        tags::add_tag_to_group(&app.pool, alice, t, g)
            .await
            .unwrap();
        sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
            .bind(track)
            .bind(t)
            .execute(&app.pool)
            .await
            .unwrap();
    }

    let cookie = app.session_cookie(alice).await;
    let resp = app
        .client()
        .get(app.url(&format!("/api/hub/tracks/{track}/ripeness")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let v: serde_json::Value = resp.json().await.unwrap();
    let d = &v["data"];
    // Two tags in one group => 100 + 50 = 150.
    assert_eq!(d["tagScore"].as_f64(), Some(150.0), "tag score: {d}");
    let groups = d["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["tags"].as_i64(), Some(2));
    // Human tags dominate meta (tag_weight 3 > meta_weight 1).
    assert!(d["total"].as_f64().unwrap() >= 450.0);
}
