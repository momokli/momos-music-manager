//! Integration tests for the tagging/type foundation (issue #204):
//! a tag's hard-wired `type` (via `crate::tagtypes`) and its exposure through
//! `tags::tag_detail` and the `/tags` page filter.

mod common;

use mmm_hub::tagtypes::{TagType, set as set_type};

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
async fn tag_type_is_settable_and_exposed() {
    let app = common::spawn().await;
    let alice = app.seed.alice;

    let tag = mmm_hub::tags::ensure_tag(&app.pool, alice, "Rumpelkiste")
        .await
        .unwrap();
    set_type(&app.pool, alice, tag, Some(TagType::Rumpelkiste))
        .await
        .unwrap();

    let got = mmm_hub::tagtypes::get(&app.pool, tag).await;
    assert_eq!(got, Some(TagType::Rumpelkiste));
    assert!(!TagType::Rumpelkiste.describes());

    let d = mmm_hub::tags::tag_detail(&app.pool, tag).await.unwrap();
    assert_eq!(d.type_name, "RUMPELKISTE");

    // A non-owner may not change it.
    assert!(
        set_type(&app.pool, app.seed.bob, tag, Some(TagType::Mood))
            .await
            .is_err()
    );
}

/// Issue #205: `/tags` filters server-side by one or several tag types.
#[tokio::test]
async fn tags_page_filters_by_multiple_types() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;

    let dark = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    let house = tags::ensure_tag(&app.pool, alice, "Housey").await.unwrap();
    set_type(&app.pool, alice, dark, Some(TagType::Mood))
        .await
        .unwrap();
    set_type(&app.pool, alice, house, Some(TagType::Genre))
        .await
        .unwrap();

    let cookie = app.session_cookie(alice).await;

    // Single type only shows its tag.
    let html = get(&app, "/tags?types=MOOD", &cookie).await;
    assert!(html.contains("Dark"), "mood tag missing");
    assert!(
        !html.contains("Housey"),
        "genre tag leaked into mood filter"
    );

    // Two types union their tags.
    let html = get(&app, "/tags?types=MOOD,GENRE", &cookie).await;
    assert!(
        html.contains("Dark") && html.contains("Housey"),
        "union missing"
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
        .form(&[("name", "Brandneu"), ("type", "MOOD")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let html = get(&app, "/tags", &cookie).await;
    assert!(html.contains("Brandneu"), "new tag not listed");

    // It belongs to the current user and carries the chosen type.
    let (owner, ty): (String, Option<String>) = sqlx::query_as(
        "SELECT u.slug, t.type FROM hub_tags t JOIN hub_users u ON u.id = t.owner_user_id
          WHERE t.name = 'Brandneu'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(owner, "alice");
    assert_eq!(ty.as_deref(), Some("MOOD"));
}

/// Issue #207: genre variation -> main direction (Hauptrichtung) via parent link.
#[tokio::test]
async fn genre_variation_sets_main_direction() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let psy = tags::ensure_tag(&app.pool, alice, "Psy").await.unwrap();
    set_type(&app.pool, alice, psy, Some(TagType::Genre))
        .await
        .unwrap();
    assert!(
        tags::tag_is_genre(&app.pool, psy).await,
        "Psy is a genre tag"
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

/// Issues #209 (T2-2) + #210 (T2-3): co-occurrence within _and_ across types,
/// with a min-support cut and jump/discover links.
#[tokio::test]
async fn tag_detail_shows_cooccurrence_and_jump_links() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;

    let dark = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    let moody = tags::ensure_tag(&app.pool, alice, "Moody").await.unwrap();
    let warehouse = tags::ensure_tag(&app.pool, alice, "Warehouse")
        .await
        .unwrap();
    let rare = tags::ensure_tag(&app.pool, alice, "Rare").await.unwrap();

    set_type(&app.pool, alice, dark, Some(TagType::Mood))
        .await
        .unwrap();
    set_type(&app.pool, alice, moody, Some(TagType::Mood))
        .await
        .unwrap();
    set_type(&app.pool, alice, warehouse, Some(TagType::Vibe))
        .await
        .unwrap();

    // Dark sits on t0..t7 (8 tracks).
    let mut dark_tracks = Vec::new();
    for i in 0..8 {
        let id = insert_track(&app.pool, &format!("d{i}"), "A").await;
        link(&app.pool, id, dark).await;
        dark_tracks.push(id);
    }
    // Moody (same type): shares t0..t3 (both=4) + one extra → support 5.
    for id in dark_tracks.iter().take(4) {
        link(&app.pool, *id, moody).await;
    }
    let m_extra = insert_track(&app.pool, "m9", "B").await;
    link(&app.pool, m_extra, moody).await;

    // Warehouse (cross type): shares t0..t2 (both=3) + three extra → support 6.
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
    assert!(co[0].same_type, "Moody shares the Mood type");
    assert_eq!(co[0].type_label, "Mood");
    assert!((co[0].lift - 1.8).abs() < 1e-6, "lift = 4*18/(8*5) = 1.8");
    assert_eq!(co[0].sample_track_id, dark_tracks[0]);

    assert_eq!(co[1].name, "Warehouse");
    assert_eq!(co[1].both, 3);
    assert_eq!(co[1].support, 6);
    assert!(!co[1].same_type, "Warehouse is only cross-type");
    assert_eq!(co[1].type_label, "Vibe");
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
    assert!(html.contains(">same<"), "same-type badge missing");
    assert!(!html.contains("Rare"), "below-min-support tag leaked");
}
