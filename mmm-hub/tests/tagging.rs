//! Integration tests for the tagging/group foundation (issue #204):
//! `hub_tag_groups.kind` + `role`, their backfill/inference, and exposure
//! through `list_groups_for` and `group_detail`.

mod common;

async fn get(app: &common::TestApp, path: &str, cookie: &str) -> String {
    app.client()
        .get(app.url(path))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

#[tokio::test]
async fn group_kind_and_semantic_role_are_exposed() {
    let app = common::spawn().await;
    let alice = app.seed.alice;

    // Known names get an inferred (kind, role) at creation time.
    let rumpel = mmm_hub::tags::create_group(&app.pool, alice, "Rumpelkiste", "")
        .await
        .unwrap();
    let setlist = mmm_hub::tags::create_group(&app.pool, alice, "Setlist", "")
        .await
        .unwrap();
    let mood = mmm_hub::tags::create_group(&app.pool, alice, "Mood", "")
        .await
        .unwrap();

    let groups = mmm_hub::tags::list_groups_for(&app.pool, alice).await;
    let find = |id: i64| groups.iter().find(|g| g.id == id).expect("group present");

    let g = find(rumpel);
    assert_eq!(g.kind, "sort");
    assert_eq!(g.semantic_role, "rumpelkiste");

    let g = find(setlist);
    assert_eq!(g.kind, "sort");
    assert_eq!(g.semantic_role, "setlist");

    let g = find(mood);
    assert_eq!(g.kind, "class");
    assert_eq!(g.semantic_role, "");

    // `group_detail` exposes the same fields.
    let d = mmm_hub::tags::group_detail(&app.pool, alice, mood)
        .await
        .expect("mood detail");
    assert_eq!(d.kind, "class");
    assert_eq!(d.semantic_role, "");
}

#[tokio::test]
async fn set_group_kind_role_updates_and_validates() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let g = mmm_hub::tags::create_group(&app.pool, alice, "Mood", "")
        .await
        .unwrap();

    mmm_hub::tags::set_group_kind_role(&app.pool, alice, g, "sort", "setlist")
        .await
        .unwrap();
    let d = mmm_hub::tags::group_detail(&app.pool, alice, g)
        .await
        .unwrap();
    assert_eq!(d.kind, "sort");
    assert_eq!(d.semantic_role, "setlist");

    // Unknown kind/role fall back to safe defaults.
    mmm_hub::tags::set_group_kind_role(&app.pool, alice, g, "nonsense", "bogus")
        .await
        .unwrap();
    let d = mmm_hub::tags::group_detail(&app.pool, alice, g)
        .await
        .unwrap();
    assert_eq!(d.kind, "class");
    assert_eq!(d.semantic_role, "");
}

/// Issue #205: `/tags` filters server-side by one or several tag groups.
#[tokio::test]
async fn tags_page_filters_by_multiple_groups() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;

    let mood = tags::create_group(&app.pool, alice, "Mood", "")
        .await
        .unwrap();
    let genre = tags::create_group(&app.pool, alice, "Genre", "")
        .await
        .unwrap();
    let dark = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    let house = tags::ensure_tag(&app.pool, alice, "Housey").await.unwrap();
    tags::add_tag_to_group(&app.pool, alice, dark, mood)
        .await
        .unwrap();
    tags::add_tag_to_group(&app.pool, alice, house, genre)
        .await
        .unwrap();

    let cookie = app.session_cookie(alice).await;

    // Single group only shows its tag.
    let html = get(&app, &format!("/tags?groups={mood}"), &cookie).await;
    assert!(html.contains("Dark"), "mood tag missing");
    assert!(
        !html.contains("Housey"),
        "genre tag leaked into mood filter"
    );

    // Two groups union their tags.
    let html = get(&app, &format!("/tags?groups={mood},{genre}"), &cookie).await;
    assert!(
        html.contains("Dark") && html.contains("Housey"),
        "union missing"
    );

    // Legacy single `group` param still works.
    let html = get(&app, &format!("/tags?group={genre}"), &cookie).await;
    assert!(
        html.contains("Housey") && !html.contains("Dark"),
        "legacy filter"
    );
}

/// Issue #206: create a tag directly from the tag page.
#[tokio::test]
async fn tag_create_from_tag_page() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    let resp = app
        .client()
        .post(app.url("/tags/create"))
        .header("Cookie", &cookie)
        .form(&[("name", "Brandneu"), ("group", "0")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let html = get(&app, "/tags", &cookie).await;
    assert!(html.contains("Brandneu"), "new tag not listed");

    // It belongs to the current user.
    let owner: String = sqlx::query_scalar(
        "SELECT u.slug FROM hub_tags t JOIN hub_users u ON u.id = t.owner_user_id
          WHERE t.name = 'Brandneu'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(owner, "alice");
}

/// Issue #207: genre variation -> main direction (Hauptrichtung) via parent link.
#[tokio::test]
async fn genre_variation_sets_main_direction() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let genre = tags::create_group(&app.pool, alice, "Genre", "").await.unwrap();
    let psy = tags::ensure_tag(&app.pool, alice, "Psy").await.unwrap();
    tags::add_tag_to_group(&app.pool, alice, psy, genre).await.unwrap();
    assert!(tags::tag_is_genre(&app.pool, psy).await, "Psy is in a genre group");

    let cookie = app.session_cookie(alice).await;
    let resp = app
        .client()
        .post(app.url(&format!("/tag/{psy}/parent/add")))
        .header("Cookie", &cookie)
        .form(&[("parent", "Trance")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let pid = tags::find_tag_id_by_name(&app.pool, "Trance").await.unwrap();
    let parents = tags::tag_parents_of(&app.pool, psy).await;
    assert_eq!(parents, vec![(pid, "Trance".to_string())]);

    // The detail page shows the genre-specific label + the main direction.
    let html = get(&app, &format!("/tag/{psy}"), &cookie).await;
    assert!(html.contains("Hauptrichtung"), "genre label missing");
    assert!(html.contains("Trance"), "main direction missing");
}
