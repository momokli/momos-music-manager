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

/// Insert a hub track with a given artist string, returning its id.
async fn insert_track(pool: &sqlx::SqlitePool, sid: &str, artists: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists, first_seen_at)
         VALUES ('spotify', ?1, ?1, ?2, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(sid)
    .bind(artists)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Materialize a track→tag link directly into `hub_track_resolved_tags`.
async fn link(pool: &sqlx::SqlitePool, track_id: i64, tag_id: i64) {
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track_id)
        .bind(tag_id)
        .execute(pool)
        .await
        .unwrap();
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
    let genre = tags::create_group(&app.pool, alice, "Genre", "")
        .await
        .unwrap();
    let psy = tags::ensure_tag(&app.pool, alice, "Psy").await.unwrap();
    tags::add_tag_to_group(&app.pool, alice, psy, genre)
        .await
        .unwrap();
    assert!(
        tags::tag_is_genre(&app.pool, psy).await,
        "Psy is in a genre group"
    );

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

    let pid = tags::find_tag_id_by_name(&app.pool, "Trance")
        .await
        .unwrap();
    let parents = tags::tag_parents_of(&app.pool, psy).await;
    assert_eq!(parents, vec![(pid, "Trance".to_string())]);

    // The detail page shows the genre-specific label + the main direction.
    let html = get(&app, &format!("/tag/{psy}"), &cookie).await;
    assert!(html.contains("Hauptrichtung"), "genre label missing");
    assert!(html.contains("Trance"), "main direction missing");
}

/// Issue #208 (T2-1): top artists of a tag with both percentage shares.
#[tokio::test]
async fn tag_detail_shows_top_artists() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let dark = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();

    // Alpha: 3 tagged of 5 total. Beta: 2 of 2. Gamma: 1 of 1.
    for i in 0..5 {
        let id = insert_track(&app.pool, &format!("alpha{i}"), "Alpha").await;
        if i < 3 {
            link(&app.pool, id, dark).await;
        }
    }
    for i in 0..2 {
        let id = insert_track(&app.pool, &format!("beta{i}"), "Beta").await;
        link(&app.pool, id, dark).await;
    }
    let gamma = insert_track(&app.pool, "gamma0", "Gamma").await;
    link(&app.pool, gamma, dark).await;

    let top = tags::tag_top_artists(&app.pool, dark).await;
    assert_eq!(top.len(), 3);
    // (artist, tagged, tracks_with_tag, tracks_by_artist), desc by tagged.
    assert_eq!(top[0], ("Alpha".to_string(), 3, 6, 5));
    assert_eq!(top[1], ("Beta".to_string(), 2, 6, 2));
    assert_eq!(top[2], ("Gamma".to_string(), 1, 6, 1));

    let cookie = app.session_cookie(alice).await;
    let html = get(&app, &format!("/tag/{dark}"), &cookie).await;
    assert!(html.contains("Top-Künstler"), "section missing");
    assert!(
        html.contains("Alpha") && html.contains("Beta"),
        "artists missing"
    );
    assert!(html.contains("50.0%"), "tag share (3/6) missing");
    assert!(html.contains("60.0%"), "artist share (3/5) missing");
}

/// Issues #209 (T2-2) + #210 (T2-3): co-occurrence within _and_ across groups,
/// with a min-support cut and jump/discover links.
#[tokio::test]
async fn tag_detail_shows_cooccurrence_and_jump_links() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;

    let mood_group = tags::create_group(&app.pool, alice, "Mood", "")
        .await
        .unwrap();
    let vibe_group = tags::create_group(&app.pool, alice, "Vibe", "")
        .await
        .unwrap();

    let dark = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    let moody = tags::ensure_tag(&app.pool, alice, "Moody").await.unwrap();
    let warehouse = tags::ensure_tag(&app.pool, alice, "Warehouse")
        .await
        .unwrap();
    let rare = tags::ensure_tag(&app.pool, alice, "Rare").await.unwrap();

    tags::add_tag_to_group(&app.pool, alice, dark, mood_group)
        .await
        .unwrap();
    tags::add_tag_to_group(&app.pool, alice, moody, mood_group)
        .await
        .unwrap();
    tags::add_tag_to_group(&app.pool, alice, warehouse, vibe_group)
        .await
        .unwrap();

    // Dark sits on t0..t7 (8 tracks).
    let mut dark_tracks = Vec::new();
    for i in 0..8 {
        let id = insert_track(&app.pool, &format!("d{i}"), "A").await;
        link(&app.pool, id, dark).await;
        dark_tracks.push(id);
    }
    // Moody (same group): shares t0..t3 (both=4) + one extra → support 5.
    for id in dark_tracks.iter().take(4) {
        link(&app.pool, *id, moody).await;
    }
    let m_extra = insert_track(&app.pool, "m9", "B").await;
    link(&app.pool, m_extra, moody).await;

    // Warehouse (cross group): shares t0..t2 (both=3) + three extra → support 6.
    for id in dark_tracks.iter().take(3) {
        link(&app.pool, *id, warehouse).await;
    }
    for i in 10..13 {
        let id = insert_track(&app.pool, &format!("w{i}"), "C").await;
        link(&app.pool, id, warehouse).await;
    }

    // Rare co-occurs on 4 tracks → below the min support of 5, must be dropped.
    for id in dark_tracks.iter().take(4) {
        link(&app.pool, *id, rare).await;
    }

    // N = 6 seeded + 8 dark + 1 moody-extra + 3 warehouse-extra = 18.
    let co = tags::tag_cooccurrence(&app.pool, dark, 5).await;
    assert_eq!(co.len(), 2, "only Moody + Warehouse pass min support");

    assert_eq!(co[0].name, "Moody");
    assert_eq!(co[0].both, 4);
    assert_eq!(co[0].support, 5);
    assert!(co[0].same_group, "Moody shares the Mood group");
    assert_eq!(co[0].group, "Mood");
    assert!((co[0].lift - 1.8).abs() < 1e-6, "lift = 4*18/(8*5) = 1.8");
    assert_eq!(co[0].sample_track_id, dark_tracks[0]);

    assert_eq!(co[1].name, "Warehouse");
    assert_eq!(co[1].both, 3);
    assert_eq!(co[1].support, 6);
    assert!(!co[1].same_group, "Warehouse is only cross-group");
    assert_eq!(co[1].group, "Vibe");
    assert!(
        (co[1].lift - 1.125).abs() < 1e-6,
        "lift = 3*18/(8*6) = 1.125"
    );
    assert_eq!(co[1].sample_track_id, dark_tracks[0]);

    let cookie = app.session_cookie(alice).await;
    let html = get(&app, &format!("/tag/{dark}"), &cookie).await;
    assert!(html.contains("Kommt oft mit"), "section missing");
    assert!(
        html.contains(&format!("/tag/{moody}")),
        "tag jump link missing"
    );
    assert!(html.contains("/overlap?tag=Moody"), "overlap link missing");
    assert!(
        html.contains(&format!("/digging?seed={}", dark_tracks[0])),
        "digging link missing"
    );
    assert!(html.contains(">same<"), "same-group badge missing");
    assert!(!html.contains("Rare"), "below-min-support tag leaked");
}
