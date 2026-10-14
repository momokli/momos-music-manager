//! Reproduce the reported tag-queue filter behaviour (feedback round).
//!
//! Each filter parameter must actually change the row set / order. These tests
//! drive the real router and assert the seeded fixtures.

mod common;

use music_hub::db::testing::Seed;

async fn queue_html(app: &common::TestApp, cookie: &str, qs: &str) -> String {
    let resp = app
        .client()
        .get(app.url(&format!("/tag-queue{qs}")))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "GET /tag-queue{qs}");
    resp.text().await.unwrap()
}

/// The seeded track ids that appear as rows, in render order.
fn row_ids(html: &str, seed: &Seed) -> Vec<i64> {
    let all = [
        seed.t_all,
        seed.t_two,
        seed.t_pl,
        seed.t_alice,
        seed.t_bob,
        seed.t_carol,
    ];
    let mut hits: Vec<(usize, i64)> = all
        .into_iter()
        .filter_map(|id| {
            html.find(&format!("hx-get=\"/tag-queue/{id}\""))
                .map(|pos| (pos, id))
        })
        .collect();
    hits.sort();
    hits.into_iter().map(|(_, id)| id).collect()
}

#[tokio::test]
async fn search_matches_title_and_artist() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // By title.
    let html = queue_html(&app, &cookie, "?q=Shared").await;
    assert_eq!(
        row_ids(&html, &app.seed),
        vec![app.seed.t_all],
        "title search must find 'Shared Anthem'"
    );

    // By artist.
    let html = queue_html(&app, &cookie, "?q=Aaa").await;
    assert_eq!(
        row_ids(&html, &app.seed),
        vec![app.seed.t_all],
        "artist search must find 'Aaa'"
    );
}

#[tokio::test]
async fn max_ripeness_filters_rows() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Every seeded track has title+artist meta, so ripeness > 0 → max=0 hides all.
    let html = queue_html(&app, &cookie, "?max=0").await;
    assert!(
        row_ids(&html, &app.seed).is_empty(),
        "max=0 must hide every track (all have meta)"
    );

    // A generous max keeps everything.
    let html = queue_html(&app, &cookie, "?max=100000").await;
    assert_eq!(
        row_ids(&html, &app.seed).len(),
        6,
        "max=100000 must keep all six tracks"
    );
}

#[tokio::test]
async fn untagged_filter_excludes_tagged_tracks() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    // Nothing tagged yet → all six are "untagged".
    let html = queue_html(&app, &cookie, "?untagged=1").await;
    assert_eq!(row_ids(&html, &app.seed).len(), 6);

    // Tag one track → it drops out of the untagged set.
    let tag = music_hub::tags::ensure_tag(&app.pool, alice, "Dark")
        .await
        .unwrap();
    music_hub::tags::tag_track(&app.pool, alice, app.seed.t_all, tag)
        .await
        .unwrap();

    let html = queue_html(&app, &cookie, "?untagged=1").await;
    let ids = row_ids(&html, &app.seed);
    assert!(
        !ids.contains(&app.seed.t_all),
        "a tagged track must not appear under 'nur ungetaggt' (got {ids:?})"
    );
}

#[tokio::test]
async fn mine_filter_keeps_only_my_playlist_tracks() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Alice's playlists contain t_all + t_pl.
    let html = queue_html(&app, &cookie, "?mine=1").await;
    let ids = row_ids(&html, &app.seed);
    assert_eq!(
        ids,
        vec![app.seed.t_all, app.seed.t_pl],
        "'nur meine Playlists' must keep exactly alice's playlist tracks"
    );
}

#[tokio::test]
async fn sort_by_title_and_artist_orders_rows() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let html = queue_html(&app, &cookie, "?sort=title").await;
    assert_eq!(
        row_ids(&html, &app.seed),
        vec![
            app.seed.t_alice, // Alice Only
            app.seed.t_bob,   // Bob Only
            app.seed.t_carol, // Carol Only
            app.seed.t_pl,    // Playlist Only
            app.seed.t_all,   // Shared Anthem
            app.seed.t_two,   // Two Users
        ],
        "sort=title must order by title"
    );

    let html = queue_html(&app, &cookie, "?sort=artist").await;
    assert_eq!(
        row_ids(&html, &app.seed),
        vec![
            app.seed.t_all,   // Aaa
            app.seed.t_two,   // Bbb
            app.seed.t_pl,    // Ccc
            app.seed.t_alice, // Ddd
            app.seed.t_bob,   // Eee
            app.seed.t_carol, // Fff
        ],
        "sort=artist must order by artist"
    );
}
