//! Integration tests for `POST /api/hub/import/playlist` (issue #527).
//!
//! The endpoint is the Hub's only entry point for normalized playlist JSON
//! (`spelunke.playlist-import/v1`, plan §3a). It must never touch the network.

mod common;

use reqwest::StatusCode;
use serde_json::{Value, json};

fn payload(tracks: Value) -> Value {
    json!({
        "schema": "spelunke.playlist-import/v1",
        "source": "apple_music",
        "playlist": {
            "remote_id": "pl.test123",
            "name": "Testliste",
            "description": "",
            "url": "https://music.apple.com/de/playlist/test/pl.test123",
            "storefront": "de",
            "fetched_at": "2026-11-16T20:00:00Z"
        },
        "tracks": tracks
    })
}

fn track(id: &str, isrc: Option<&str>, position: i64) -> Value {
    json!({
        "remote_track_id": id,
        "position": position,
        "isrc": isrc,
        "title": format!("Track {id}"),
        "artists": "Some Artist",
        "album": "Some Album",
        "duration_ms": 208810,
        "explicit": false,
        "image_url": "https://example.test/art.jpg"
    })
}

async fn post(app: &common::TestApp, cookie: Option<&str>, body: &Value) -> (StatusCode, Value) {
    let mut req = app
        .client()
        .post(app.url("/api/hub/import/playlist"))
        .json(body);
    if let Some(c) = cookie {
        req = req.header("Cookie", c);
    }
    let resp = req.send().await.expect("POST /api/hub/import/playlist");
    let status = resp.status();
    let text = resp.text().await.expect("body");
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn count(app: &common::TestApp, sql: &str, playlist_id: i64) -> i64 {
    sqlx::query_scalar(sql)
        .bind(playlist_id)
        .fetch_one(&app.pool)
        .await
        .expect("count query")
}

#[tokio::test]
async fn import_requires_login() {
    let app = common::spawn().await;
    let (status, body) = post(&app, None, &payload(json!([]))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "login required");
}

#[tokio::test]
async fn import_rejects_wrong_schema() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("import_alice", "pw").await;
    let mut body = payload(json!([]));
    body["schema"] = json!("spelunke.playlist-import/v2");

    let (status, resp) = post(&app, Some(&cookie), &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        resp["error"].as_str().unwrap_or_default().contains("v1"),
        "error should name the expected schema: {resp}"
    );
}

#[tokio::test]
async fn import_rejects_unknown_source() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("import_alice", "pw").await;
    let mut body = payload(json!([]));
    body["source"] = json!("spotify");

    let (status, _) = post(&app, Some(&cookie), &body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "allowlist must reject services that have their own ingest path"
    );
}

#[tokio::test]
async fn import_rejects_empty_remote_id() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("import_alice", "pw").await;
    let mut body = payload(json!([]));
    body["playlist"]["remote_id"] = json!("   ");

    let (status, _) = post(&app, Some(&cookie), &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_stores_playlist_tracks_and_normalized_isrcs() {
    let app = common::spawn().await;
    let (user_id, cookie) = app.user_with_password("import_alice", "pw").await;

    let body = payload(json!([
        track("s1", Some("us-um7 2501234"), 1),
        track("s2", Some("USUM72501235"), 0),
        track("s3", None, 2),
        track("s4", Some("TOOSHORT"), 3),
    ]));

    let (status, resp) = post(&app, Some(&cookie), &body).await;
    assert_eq!(status, StatusCode::OK, "unexpected body: {resp}");
    let data = &resp["data"];
    assert_eq!(data["tracks"], 4);
    assert_eq!(data["withIsrc"], 2, "hyphens/spaces must normalize: {data}");
    assert_eq!(
        data["withoutIsrc"], 2,
        "missing + malformed count as no-ISRC"
    );
    assert_eq!(
        data["invalidIsrc"], 1,
        "malformed ISRC is counted, not fatal"
    );
    assert_eq!(data["duplicates"], 0);

    let playlist_id = data["playlistId"].as_i64().expect("playlistId");

    // Playlist row is owned by the importing user and marked as fetched.
    let (owner, service, items): (i64, String, i64) =
        sqlx::query_as("SELECT user_id, service, items_available FROM hub_playlists WHERE id = ?1")
            .bind(playlist_id)
            .fetch_one(&app.pool)
            .await
            .expect("playlist row");
    assert_eq!(owner, user_id);
    assert_eq!(service, "apple_music");
    assert_eq!(items, 1);

    // Membership is ordered by the provided position, not by array order.
    let ordered: Vec<String> = sqlx::query_scalar(
        "SELECT t.service_track_id
           FROM hub_playlist_tracks pt
           JOIN hub_tracks t ON t.id = pt.track_id
          WHERE pt.playlist_id = ?1
          ORDER BY pt.position",
    )
    .bind(playlist_id)
    .fetch_all(&app.pool)
    .await
    .expect("membership");
    assert_eq!(ordered, vec!["s2", "s1", "s3", "s4"]);

    // ISRC normalised on write.
    let isrc: Option<String> = sqlx::query_scalar(
        "SELECT isrc FROM hub_tracks WHERE service = 'apple_music' AND service_track_id = 's1'",
    )
    .fetch_one(&app.pool)
    .await
    .expect("track s1");
    assert_eq!(isrc.as_deref(), Some("USUM72501234"));

    // Track without a usable ISRC stays NULL — it is stored, just not orderable.
    let none_isrc: Option<String> = sqlx::query_scalar(
        "SELECT isrc FROM hub_tracks WHERE service = 'apple_music' AND service_track_id = 's3'",
    )
    .fetch_one(&app.pool)
    .await
    .expect("track s3");
    assert_eq!(none_isrc, None);

    // External id carries the source id + playlist url.
    let (ext_id, url): (String, Option<String>) = sqlx::query_as(
        "SELECT external_id, url FROM hub_track_external_ids
          WHERE service = 'apple_music' AND external_id = 's1'",
    )
    .fetch_one(&app.pool)
    .await
    .expect("external id");
    assert_eq!(ext_id, "s1");
    assert!(url.unwrap_or_default().contains("music.apple.com"));
}

#[tokio::test]
async fn import_is_idempotent() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("import_alice", "pw").await;
    let body = payload(json!([
        track("s1", Some("USUM72501234"), 0),
        track("s2", None, 1),
    ]));

    let (_, first) = post(&app, Some(&cookie), &body).await;
    let (_, second) = post(&app, Some(&cookie), &body).await;

    assert_eq!(first["data"]["playlistId"], second["data"]["playlistId"]);
    assert_eq!(second["data"]["tracks"], 2);

    let playlist_id = second["data"]["playlistId"].as_i64().expect("playlistId");
    assert_eq!(
        count(
            &app,
            "SELECT COUNT(*) FROM hub_playlists WHERE id = ?1",
            playlist_id
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &app,
            "SELECT COUNT(*) FROM hub_playlist_tracks WHERE playlist_id = ?1",
            playlist_id
        )
        .await,
        2,
        "membership must be replaced, not duplicated"
    );
    let tracks: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_tracks WHERE service = 'apple_music'")
            .fetch_one(&app.pool)
            .await
            .expect("track count");
    assert_eq!(
        tracks, 2,
        "tracks are upserted by (service, service_track_id)"
    );
}

#[tokio::test]
async fn import_dedupes_repeated_remote_ids() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("import_alice", "pw").await;
    let body = payload(json!([
        track("s1", Some("USUM72501234"), 0),
        track("s1", Some("USUM72501234"), 5),
        track("s2", None, 1),
    ]));

    let (status, resp) = post(&app, Some(&cookie), &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["data"]["duplicates"], 1);
    assert_eq!(resp["data"]["tracks"], 2);

    let playlist_id = resp["data"]["playlistId"].as_i64().expect("playlistId");
    let positions: Vec<i64> = sqlx::query_scalar(
        "SELECT position FROM hub_playlist_tracks WHERE playlist_id = ?1 ORDER BY position",
    )
    .bind(playlist_id)
    .fetch_all(&app.pool)
    .await
    .expect("positions");
    assert_eq!(positions, vec![0, 1], "first occurrence wins, no gaps");
}

#[tokio::test]
async fn import_does_not_clear_existing_isrc() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("import_alice", "pw").await;

    let with_isrc = payload(json!([track("s1", Some("USUM72501234"), 0)]));
    post(&app, Some(&cookie), &with_isrc).await;

    // A later delivery without an ISRC (e.g. a source that cannot resolve it)
    // must not wipe the value a previous import established.
    let without_isrc = payload(json!([track("s1", None, 0)]));
    let (status, resp) = post(&app, Some(&cookie), &without_isrc).await;
    assert_eq!(status, StatusCode::OK, "body: {resp}");

    let isrc: Option<String> = sqlx::query_scalar(
        "SELECT isrc FROM hub_tracks WHERE service = 'apple_music' AND service_track_id = 's1'",
    )
    .fetch_one(&app.pool)
    .await
    .expect("track s1");
    assert_eq!(isrc.as_deref(), Some("USUM72501234"));
}

#[tokio::test]
async fn import_is_scoped_per_user() {
    let app = common::spawn().await;
    let (alice, alice_cookie) = app.user_with_password("import_alice", "pw").await;
    let (bob, bob_cookie) = app.user_with_password("import_bob", "pw").await;

    let body = payload(json!([track("s1", Some("USUM72501234"), 0)]));
    let (_, a) = post(&app, Some(&alice_cookie), &body).await;
    let (_, b) = post(&app, Some(&bob_cookie), &body).await;

    let a_id = a["data"]["playlistId"].as_i64().expect("alice playlist");
    let b_id = b["data"]["playlistId"].as_i64().expect("bob playlist");
    assert_ne!(
        a_id, b_id,
        "the same remote id belongs to each user separately"
    );

    let alice_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_playlists WHERE user_id = ?1")
            .bind(alice)
            .fetch_one(&app.pool)
            .await
            .expect("alice playlists");
    let bob_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_playlists WHERE user_id = ?1")
        .bind(bob)
        .fetch_one(&app.pool)
        .await
        .expect("bob playlists");
    assert_eq!((alice_rows, bob_rows), (1, 1));
}

#[tokio::test]
async fn import_skips_tracks_without_remote_id() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("import_alice", "pw").await;
    let body = payload(json!([
        track("s1", Some("USUM72501234"), 0),
        { "remote_track_id": "  ", "title": "broken" },
    ]));

    let (status, resp) = post(&app, Some(&cookie), &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["data"]["skipped"], 1);
    assert_eq!(resp["data"]["tracks"], 1);
}

#[tokio::test]
async fn import_reports_nothing_touched_without_music_api_token() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("import_alice", "pw").await;
    let body = payload(json!([track("s1", Some("USUM72501234"), 0)]));

    let (_, resp) = post(&app, Some(&cookie), &body).await;
    // `touch_tracks` runs only for ISRC-bearing tracks, and `order_touched`
    // returns early without a token — so the import must not attempt network IO.
    assert_eq!(resp["data"]["touched"], 1);
    assert_eq!(resp["data"]["withIsrc"], 1);
}
