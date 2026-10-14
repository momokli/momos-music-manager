//! Download-priority orders + bounded touch-cascade (#hub-crawl-everything).

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::Path;
use axum::routing::{get, post};
use axum::{Json, Router};
use music_hub::config::Config;
use serde_json::{Value, json};

const NOW: &str = "2026-01-01T00:00:00+00:00";

fn cfg_with(base: String, token: Option<&str>) -> Config {
    let mut cfg = Config::for_test("sqlite::memory:");
    cfg.music_api_base = base;
    cfg.music_api_token = token.map(str::to_string);
    cfg
}

/// Boot a mock music-api `/orders` that records every request body.
async fn mock_orders(sink: Arc<Mutex<Vec<Value>>>) -> String {
    let handler = move |Json(body): Json<Value>| {
        let sink = sink.clone();
        async move {
            sink.lock().unwrap().push(body);
            Json(json!({ "orderId": "o1" }))
        }
    };
    let app = Router::new().route("/orders", post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Boot a mock Spotify API serving one artist's albums + top-tracks and one
/// album's tracks.
async fn mock_spotify_crawl() -> String {
    async fn artist_albums(Path(_id): Path<String>) -> Json<Value> {
        Json(json!({
            "items": [{
                "id": "alb-1", "name": "Album One", "album_type": "album",
                "release_date": "2024-01-01", "total_tracks": 1,
                "images": [{ "url": "http://img/alb" }]
            }],
            "next": null
        }))
    }
    async fn artist_top(Path(_id): Path<String>) -> Json<Value> {
        Json(json!({
            "tracks": [{
                "type": "track", "id": "trk-top", "name": "Top Track",
                "artists": [{ "id": "sp-art", "name": "Artist" }],
                "duration_ms": 2000, "explicit": false
            }]
        }))
    }
    async fn album_tracks(Path(_id): Path<String>) -> Json<Value> {
        Json(json!({
            "items": [{
                "type": "track", "id": "trk-a1", "name": "Track A1",
                "artists": [{ "id": "sp-art", "name": "Artist" }],
                "duration_ms": 1000, "explicit": false
            }],
            "next": null
        }))
    }
    let app = Router::new()
        .route("/artists/{id}/albums", get(artist_albums))
        .route("/artists/{id}/top-tracks", get(artist_top))
        .route("/albums/{id}/tracks", get(album_tracks));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Boot a mock Spotify API that fails every request (HTTP 500).
async fn mock_spotify_error() -> String {
    async fn boom() -> (axum::http::StatusCode, &'static str) {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom")
    }
    let app = Router::new().route("/artists/{id}/albums", get(boom));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Link a Spotify account whose token never needs refreshing.
async fn link_account(pool: &sqlx::SqlitePool, slug: &str) {
    let user_id = music_hub::ingest::ensure_user(pool, slug).await.unwrap();
    sqlx::query(
        "INSERT INTO hub_service_accounts
             (user_id, service, access_token, refresh_token, token_expiry, scopes, authorized_at, connected_at)
         VALUES (?1, 'spotify', 'mock-access-token', 'mock-refresh-token', '2099-01-01T00:00:00+00:00',
                 'test', ?2, ?2)",
    )
    .bind(user_id)
    .bind(NOW)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn priorities_reflect_likes_and_playlists() {
    let app = common::spawn().await;
    let ids = [
        app.seed.t_all,
        app.seed.t_two,
        app.seed.t_pl,
        app.seed.t_alice,
        app.seed.t_bob,
        app.seed.t_carol,
    ];
    let p = music_hub::crawl::priorities(&app.pool, &ids).await;
    // t_all: liked by 3 + in alice's playlist -> 1 + 1 = 2.
    assert_eq!(p[&app.seed.t_all], 2);
    // t_two: liked by 2 -> 1.
    assert_eq!(p[&app.seed.t_two], 1);
    // t_pl: in alice's + bob's playlists (no like) -> 0 + 2 = 2.
    assert_eq!(p[&app.seed.t_pl], 2);
    // t_alice: liked only -> 1.
    assert_eq!(p[&app.seed.t_alice], 1);
    // t_bob: liked only -> 1.
    assert_eq!(p[&app.seed.t_bob], 1);
    // t_carol: liked + in carol's playlist -> 2.
    assert_eq!(p[&app.seed.t_carol], 2);
}

#[tokio::test]
async fn order_without_token_is_noop() {
    let app = common::spawn().await;
    let cfg = cfg_with("http://127.0.0.1:1".to_string(), None);
    assert_eq!(
        music_hub::crawl::order_touched(&app.pool, &cfg, &[app.seed.t_all])
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        music_hub::crawl::order_pending(&app.pool, &cfg, 100)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        music_hub::crawl::touch_cascade(&app.pool, &cfg, 10)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn order_touched_sends_priority_and_records_state() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let base = mock_orders(sink.clone()).await;
    let app = common::spawn().await;
    let cfg = cfg_with(base, Some("tok"));

    sqlx::query("UPDATE hub_tracks SET isrc = 'ISRC-ALL' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();

    let n = music_hub::crawl::order_touched(&app.pool, &cfg, &[app.seed.t_all])
        .await
        .unwrap();
    assert_eq!(n, 1);

    let calls = sink.lock().unwrap();
    assert_eq!(calls.len(), 1, "one order group");
    assert_eq!(calls[0]["priority"], 2, "liked + one playlist = 2");
    assert_eq!(calls[0]["items"][0]["isrc"], "ISRC-ALL");
    drop(calls);

    let p: i64 = sqlx::query_scalar("SELECT priority FROM hub_order_state WHERE track_id = ?1")
        .bind(app.seed.t_all)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(p, 2, "order state recorded");
}

#[tokio::test]
async fn order_pending_is_idempotent() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let base = mock_orders(sink.clone()).await;
    let app = common::spawn().await;
    let cfg = cfg_with(base, Some("tok"));

    sqlx::query("UPDATE hub_tracks SET isrc = 'ISRC-ALL' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();

    let first = music_hub::crawl::order_pending(&app.pool, &cfg, 100)
        .await
        .unwrap();
    assert_eq!(
        first, 1,
        "only the ISRC-bearing referenced track is ordered"
    );
    let second = music_hub::crawl::order_pending(&app.pool, &cfg, 100)
        .await
        .unwrap();
    assert_eq!(second, 0, "unchanged priority re-orders nothing");
}

#[tokio::test]
async fn touch_cascade_records_errors_for_retry() {
    let base = mock_spotify_error().await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let mut cfg = Config::for_test(url);
    cfg.spotify_api_base = base;
    let app = common::spawn_with(dir, cfg).await;
    link_account(&app.pool, "dave").await;
    music_hub::artists::upsert_artist_by_spotify(&app.pool, "sp-art", "Artist")
        .await
        .unwrap();

    let used = music_hub::crawl::touch_cascade(&app.pool, &app.cfg, 10)
        .await
        .unwrap();
    assert_eq!(used, 0, "a failed request spends no budget");

    let (state, attempts): (String, i64) = sqlx::query_as(
        "SELECT state, attempts FROM hub_crawl_state WHERE kind = 'artist' AND ref = 'sp-art'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(state, "error");
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn touch_cascade_creates_albums_and_tracks() {
    let base = mock_spotify_crawl().await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let mut cfg = Config::for_test(url);
    cfg.spotify_api_base = base;
    let app = common::spawn_with(dir, cfg).await;
    link_account(&app.pool, "dave").await;

    // A touched artist with a Spotify id.
    music_hub::artists::upsert_artist_by_spotify(&app.pool, "sp-art", "Artist")
        .await
        .unwrap();

    let used = music_hub::crawl::touch_cascade(&app.pool, &app.cfg, 10)
        .await
        .unwrap();
    assert_eq!(used, 3, "artist + top-tracks + album request");

    // The album and its track were created.
    let albums: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_albums WHERE title = 'Album One'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(albums, 1);
    let tracks: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_tracks WHERE service = 'spotify' AND service_track_id = 'trk-a1'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(tracks, 1);

    // The artist's top-track (no album) was ingested too.
    let top: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_tracks WHERE service = 'spotify' AND service_track_id = 'trk-top'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(top, 1, "top-track ingested");

    // The crawl frontier is recorded (artist + artist_top + album done).
    let done: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_crawl_state WHERE state = 'done'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(done, 3);

    // A second pass spends nothing (TTL not expired).
    let again = music_hub::crawl::touch_cascade(&app.pool, &app.cfg, 10)
        .await
        .unwrap();
    assert_eq!(again, 0, "TTL'd crawl does not repeat");
}

/// Link `track_id` to a fresh Spotify artist and return the artist's local id.
async fn link_spotify_artist(pool: &sqlx::SqlitePool, track_id: i64, sid: &str) -> i64 {
    music_hub::artists::upsert_artist_by_spotify(pool, sid, "Touch Artist")
        .await
        .unwrap();
    let artist_id: i64 = sqlx::query_scalar(
        "SELECT artist_id FROM hub_artist_external_ids
          WHERE service = 'spotify' AND external_id = ?1",
    )
    .bind(sid)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO hub_track_artists (track_id, artist_id, position) VALUES (?1, ?2, 0)
         ON CONFLICT(track_id, artist_id) DO NOTHING",
    )
    .bind(track_id)
    .bind(artist_id)
    .execute(pool)
    .await
    .unwrap();
    artist_id
}

#[tokio::test]
async fn touch_tracks_orders_and_seeds_artist_frontier() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let base = mock_orders(sink.clone()).await;
    let app = common::spawn().await;
    let cfg = cfg_with(base, Some("tok"));

    sqlx::query("UPDATE hub_tracks SET isrc = 'ISRC-TOUCH' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();
    link_spotify_artist(&app.pool, app.seed.t_all, "sp-touch").await;

    music_hub::crawl::touch_tracks(&app.pool, &cfg, &[app.seed.t_all])
        .await
        .unwrap();

    // Ordered at music-api (immediate).
    {
        let calls = sink.lock().unwrap();
        assert_eq!(calls.len(), 1, "one order group");
        assert_eq!(calls[0]["items"][0]["isrc"], "ISRC-TOUCH");
    }

    // Both frontier kinds seeded `pending` for the track's artist.
    for kind in ["artist", "artist_top"] {
        let state: String = sqlx::query_scalar(
            "SELECT state FROM hub_crawl_state WHERE kind = ?1 AND ref = 'sp-touch'",
        )
        .bind(kind)
        .fetch_one(&app.pool)
        .await
        .unwrap();
        assert_eq!(state, "pending", "{kind} frontier seeded pending");
    }
}

#[tokio::test]
async fn seed_artist_frontier_does_not_reset_done() {
    let app = common::spawn().await;
    link_spotify_artist(&app.pool, app.seed.t_all, "sp-done").await;

    // The artist's album crawl already finished.
    sqlx::query(
        "INSERT INTO hub_crawl_state (kind, ref, state, attempts, crawled_at, created_at, updated_at)
         VALUES ('artist', 'sp-done', 'done', 1, ?1, ?1, ?1)",
    )
    .bind(NOW)
    .execute(&app.pool)
    .await
    .unwrap();

    music_hub::crawl::seed_artist_frontier(&app.pool, &[app.seed.t_all])
        .await
        .unwrap();

    // The existing `done` row is left untouched (no re-crawl before TTL).
    let state: String = sqlx::query_scalar(
        "SELECT state FROM hub_crawl_state WHERE kind = 'artist' AND ref = 'sp-done'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(state, "done", "existing done row is not reset");

    // The absent `artist_top` row is seeded `pending`.
    let top: String = sqlx::query_scalar(
        "SELECT state FROM hub_crawl_state WHERE kind = 'artist_top' AND ref = 'sp-done'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(top, "pending");
}
