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

#[tokio::test]
async fn tagging_with_a_group_places_the_tag_in_it() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/tag")))
        .header("Cookie", &cookie)
        .form(&[("name", "Dark"), ("group", "Mood")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_group_tags gt
           JOIN hub_tags t ON t.id = gt.tag_id
           JOIN hub_tag_groups g ON g.id = gt.group_id
          WHERE t.name = 'Dark' AND g.name = 'Mood'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "tag placed in the Mood group");
}

#[tokio::test]
async fn recommendations_use_relationship_signals() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let me = app.seed.alice;

    // Two tracks by the same artist; one is tagged.
    sqlx::query(
        "INSERT INTO hub_tracks (id, service, service_track_id, title, artists, album) VALUES
            (80001, 'local', 'r1', 'One', 'RelArti', 'RelAlbum'),
            (80002, 'local', 'r2', 'Two', 'RelArti', 'RelAlbum')",
    )
    .execute(&app.pool)
    .await
    .unwrap();
    let house = tags::ensure_tag(&app.pool, me, "House").await.unwrap();
    tags::tag_track(&app.pool, me, 80002, house).await.unwrap();

    // 80001 shares artist + album with the tagged 80002 -> House is recommended.
    let recs = mmm_hub::recommend::recommend_tags(&app.pool, 80001, 10).await;
    assert!(
        recs.iter().any(|r| r.name == "House"),
        "artist/album relationship should recommend House: {:?}",
        recs.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    // The recommendation explains itself (artist / album signal).
    let house = recs.iter().find(|r| r.name == "House").unwrap();
    assert!(
        !house.why().is_empty(),
        "recommendation must carry a reason"
    );
}
