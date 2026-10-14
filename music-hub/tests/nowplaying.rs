//! "Now playing" widget tests (issue #293/#294).
//!
//! The widget itself polls Spotify; integration tests cover the auth gate, the
//! scope gate (which never touches the network) and the inline tagging POST.

mod common;

use serde_json::json;
use sqlx::Row;

fn now_item(sid: &str) -> serde_json::Value {
    json!({
        "id": sid,
        "name": "Heal Me",
        "artists": [{ "name": "Rodriguez Jr." }],
        "album": { "name": "Baobab", "images": [{ "url": "http://img/x" }] },
        "duration_ms": 374000,
        "explicit": false,
        "external_ids": { "isrc": "GB-AAA-19-00001" }
    })
}

#[tokio::test]
async fn listening_page_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/listening"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn listening_page_shows_recorded_listen() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    music_hub::nowplaying::record_listen(&app.pool, uid, app.seed.t_all, 0, 180000)
        .await
        .unwrap();
    let resp = app
        .client()
        .get(app.url("/listening"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Gehört"), "heading missing");
    assert!(html.contains("Top-Tracks"), "top tracks section missing");
}

#[tokio::test]
async fn ingest_spotify_item_creates_track_and_external_id() {
    let app = common::spawn().await;
    let id = music_hub::ingest::ingest_spotify_item(&app.pool, &now_item("unknown-1"))
        .await
        .unwrap()
        .expect("track created");
    let row = sqlx::query("SELECT title, artists, album, isrc FROM hub_tracks WHERE id = ?1")
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("title"), "Heal Me");
    assert_eq!(row.get::<String, _>("artists"), "Rodriguez Jr.");
    assert_eq!(row.get::<String, _>("album"), "Baobab");
    assert_eq!(row.get::<String, _>("isrc"), "GB-AAA-19-00001");
    let ext: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_external_ids WHERE track_id = ?1 AND service = 'spotify'",
    )
    .bind(id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(ext, 1);
    // Atomic artist/album entities (plan P0).
    let links: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_track_artists WHERE track_id = ?1")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(links, 1, "one artist link expected");
    let album_id: Option<i64> = sqlx::query_scalar("SELECT album_id FROM hub_tracks WHERE id = ?1")
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert!(album_id.is_some(), "album entity not linked");
}

#[tokio::test]
async fn ingest_stores_atomic_artists_and_album() {
    let app = common::spawn().await;
    let item = json!({
        "id": "multi-1",
        "name": "Duo",
        "artists": [
            { "id": "sp-a", "name": "Alpha" },
            { "id": "sp-b", "name": "Beta" }
        ],
        "album": {
            "id": "alb-1",
            "name": "Collab",
            "album_type": "single",
            "release_date": "2024-01-01",
            "total_tracks": 2,
            "images": [{ "url": "http://img/a" }]
        },
        "duration_ms": 1000,
        "explicit": false
    });
    let id = music_hub::ingest::ingest_spotify_item(&app.pool, &item)
        .await
        .unwrap()
        .expect("track created");
    let links: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_track_artists WHERE track_id = ?1")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(links, 2, "two atomic artists expected");
    let ext: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_artist_external_ids
          WHERE service = 'spotify' AND external_id IN ('sp-a', 'sp-b')",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(ext, 2, "both spotify artist ids recorded");
    let album_ext: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_album_external_ids
          WHERE service = 'spotify' AND external_id = 'alb-1'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(album_ext, 1, "album external id recorded");
}

#[tokio::test]
async fn record_listen_coalesces_a_session() {
    let app = common::spawn().await;
    let (uid, _) = app.user_with_password("zoe", "pw").await;
    let t = app.seed.t_all;
    let other = app.seed.t_two;

    music_hub::nowplaying::record_listen(&app.pool, uid, t, 1000, 180000)
        .await
        .unwrap();
    music_hub::nowplaying::record_listen(&app.pool, uid, t, 21000, 180000)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_listening_history WHERE user_id = ?1 AND track_id = ?2",
    )
    .bind(uid)
    .bind(t)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "same track within a session coalesces");

    music_hub::nowplaying::record_listen(&app.pool, uid, other, 0, 120000)
        .await
        .unwrap();
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_listening_history WHERE user_id = ?1")
            .bind(uid)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(total, 2, "a different track starts a new row");
}

#[tokio::test]
async fn now_page_requires_login() {
    let app = common::spawn().await;
    let resp = app.client().get(app.url("/now")).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn now_page_prompts_reconnect_without_scope() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("zoe", "pw").await;
    let resp = app
        .client()
        .get(app.url("/now"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Now Playing"), "heading missing: {html}");
    assert!(
        html.contains("/api/hub/services/spotify/connect"),
        "connect link missing: {html}"
    );
}

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
