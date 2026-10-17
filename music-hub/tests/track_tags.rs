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
    // The redirect carries the (default) scope so the toggle does not reset.
    assert_eq!(
        resp.headers()["location"],
        format!("/track/{track}?scope=all")
    );

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
    assert!(html.contains("data-taginput"), "tag input missing");

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
    use music_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;

    let dark = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    let mel = tags::ensure_tag(&app.pool, alice, "Melancholisch")
        .await
        .unwrap();
    // t_all has Dark; t_two has Dark + Melancholisch -> Melancholisch is
    // recommended for t_all (via the shared Dark tag).
    tags::tag_track(&app.pool, &app.cfg, alice, app.seed.t_all, dark)
        .await
        .unwrap();
    tags::tag_track(&app.pool, &app.cfg, alice, app.seed.t_two, dark)
        .await
        .unwrap();
    tags::tag_track(&app.pool, &app.cfg, alice, app.seed.t_two, mel)
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
async fn tagging_with_a_type_sets_the_type() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/tag")))
        .header("Cookie", &cookie)
        .form(&[("name", "Dark"), ("type", "MOOD")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let ty: Option<String> = sqlx::query_scalar("SELECT type FROM hub_tags WHERE name = 'Dark'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(ty.as_deref(), Some("MOOD"), "tag gets the Mood type");
}

#[tokio::test]
async fn recommendations_use_relationship_signals() {
    use music_hub::tags;
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
    tags::tag_track(&app.pool, &app.cfg, me, 80002, house).await.unwrap();

    // 80001 shares artist + album with the tagged 80002 -> House is recommended.
    let recs = music_hub::recommend::recommend_tags(
        &app.pool,
        80001,
        10,
        me,
        music_hub::recommend::RecScope::All,
    )
    .await;
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

#[tokio::test]
async fn recommendations_exclude_setlist_and_rumpelkiste() {
    use music_hub::tags;
    use music_hub::tagtypes::{TagType, set as set_type};
    let app = common::spawn().await;
    let me = app.seed.alice;

    sqlx::query(
        "INSERT INTO hub_tracks (id, service, service_track_id, title, artists, album) VALUES
            (81001, 'local', 's1', 'One', 'SetArti', 'SetAlbum'),
            (81002, 'local', 's2', 'Two', 'SetArti', 'SetAlbum')",
    )
    .execute(&app.pool)
    .await
    .unwrap();

    let s_tag = tags::ensure_tag(&app.pool, me, "setlist25").await.unwrap();
    let h_tag = tags::ensure_tag(&app.pool, me, "House").await.unwrap();
    set_type(&app.pool, me, s_tag, Some(TagType::Setlist))
        .await
        .unwrap();
    set_type(&app.pool, me, h_tag, Some(TagType::Mood))
        .await
        .unwrap();

    // Related track (same artist) carries both tags.
    tags::tag_track(&app.pool, &app.cfg, me, 81002, s_tag).await.unwrap();
    tags::tag_track(&app.pool, &app.cfg, me, 81002, h_tag).await.unwrap();

    let recs = music_hub::recommend::recommend_tags(
        &app.pool,
        81001,
        20,
        me,
        music_hub::recommend::RecScope::All,
    )
    .await;
    let names: Vec<&str> = recs.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&"House"),
        "Mood tag should be recommended: {names:?}"
    );
    assert!(
        !names.contains(&"setlist25"),
        "setlist tag must NOT be recommended: {names:?}"
    );
}

/// The track view surfaces the cluster signal and the scope toggle; `?scope=all`
/// widens visibility to another owner's cluster sibling.
#[tokio::test]
async fn track_view_shows_cluster_rec_and_scope_toggle() {
    use music_hub::tags;
    use music_hub::tagtypes::{TagType, set as set_type};
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let bob = app.seed.bob;

    let dark = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    let duester = tags::ensure_tag(&app.pool, bob, "Düster").await.unwrap();
    set_type(&app.pool, alice, dark, Some(TagType::Mood))
        .await
        .unwrap();
    set_type(&app.pool, bob, duester, Some(TagType::Mood))
        .await
        .unwrap();
    let cid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tag_clusters (tag_type, label, created_at, updated_at)
         VALUES ('MOOD', 'dark', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    for t in [dark, duester] {
        sqlx::query(
            "INSERT INTO hub_tag_cluster_members (cluster_id, tag_id, added_at)
             VALUES (?1, ?2, '2026-01-01T00:00:00+00:00')",
        )
        .bind(cid)
        .bind(t)
        .execute(&app.pool)
        .await
        .unwrap();
    }
    tags::tag_track(&app.pool, &app.cfg, alice, app.seed.t_all, dark)
        .await
        .unwrap();

    let cookie = app.session_cookie(alice).await;

    // Default (no params) = `own` (#267): Bob's cluster sibling is out of scope.
    let default_html = app
        .client()
        .get(app.url(&format!("/track/{}", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        default_html.contains("Empfohlen"),
        "recommendation section missing"
    );
    assert!(
        !default_html.contains("Düster"),
        "default scope must be `own` and hide another owner's cluster sibling"
    );
    assert!(default_html.contains("Meine Tags"), "scope toggle missing");
    assert!(default_html.contains("scope=mine"), "mine toggle link missing");

    // Explicit legacy `?scope=all` widens to Bob's cluster sibling + reason.
    let html = app
        .client()
        .get(app.url(&format!("/track/{}?scope=all", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        html.contains("gleicher Cluster"),
        "cluster reason label missing"
    );
    assert!(html.contains("Düster"), "cluster sibling not shown");

    // Explicit mine: Bob's sibling disappears.
    let mine_html = app
        .client()
        .get(app.url(&format!("/track/{}?scope=mine", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !mine_html.contains("Düster"),
        "mine scope must hide another owner's tag"
    );
}

/// Mutating tags from the track view must carry the active recommendation scope
/// in the redirect, so the toggle does not jump back to "all".
#[tokio::test]
async fn track_tag_redirect_preserves_scope() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/tag")))
        .header("Cookie", &cookie)
        .form(&[("name", "Dark"), ("scope", "mine")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()["location"],
        format!("/track/{track}?scope=mine")
    );

    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/untag")))
        .header("Cookie", &cookie)
        .form(&[("name", "Dark"), ("scope", "mine")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()["location"],
        format!("/track/{track}?scope=mine")
    );
}

/// The rendered track view embeds the active scope as a hidden field on every
/// mutating form, so tag/untag round-trips keep the chosen scope.
#[tokio::test]
async fn track_view_emits_active_scope() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let html = app
        .client()
        .get(app.url(&format!("/track/{}?scope=mine", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        html.contains("data-scope=\"mine\""),
        "track view must embed the active scope for mutations"
    );
}
