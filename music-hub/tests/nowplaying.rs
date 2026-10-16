//! "Now playing" widget tests (issue #293/#294).
//!
//! The widget itself polls Spotify; integration tests cover the auth gate, the
//! scope gate (which never touches the network) and the inline tagging POST.

mod common;

use axum::Json;
use axum::extract::RawQuery;
use axum::routing::{get, post};
use music_hub::config::Config;
use serde_json::{Value, json};
use sqlx::Row;
use std::sync::{Arc, Mutex};

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

// ── playback queue ("Als Nächstes") ────────────────────────────────────────

/// One Spotify track object as it appears inside `GET /me/player/queue`.
fn queued_item(sid: &str, name: &str) -> Value {
    json!({
        "type": "track",
        "id": sid,
        "name": name,
        "artists": [{ "id": "sp-q", "name": "Queue Artist" }],
        "album": {
            "id": "alb-q", "name": "Queue Album",
            "images": [{ "url": "http://img/q" }]
        },
        "duration_ms": 245000,
        "explicit": false,
        "external_ids": { "isrc": "GB-BBB-19-00009" }
    })
}

/// Boot a mock Spotify API whose `/me/player/queue` returns `items`.
async fn mock_spotify_queue(items: Value) -> String {
    let handler = move || {
        let items = items.clone();
        async move { Json(json!({ "currently_playing": Value::Null, "queue": items })) }
    };
    let app = axum::Router::new().route("/me/player/queue", get(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Link a Spotify account whose token never needs refreshing.
async fn link_spotify(pool: &sqlx::SqlitePool, user_id: i64, scopes: &str) {
    sqlx::query(
        "INSERT INTO hub_service_accounts
             (user_id, service, access_token, refresh_token, token_expiry, scopes, authorized_at, connected_at)
         VALUES (?1, 'spotify', 'mock-access-token', 'mock-refresh-token',
                 '2099-01-01T00:00:00+00:00', ?2,
                 '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
    )
    .bind(user_id)
    .bind(scopes)
    .execute(pool)
    .await
    .unwrap();
}

/// Boot a hub whose Spotify base points at a queue mock.
async fn spawn_with_queue(items: Value) -> common::TestApp {
    let base = mock_spotify_queue(items).await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let mut cfg = Config::for_test(url);
    cfg.spotify_api_base = base;
    common::spawn_with(dir, cfg).await
}

#[tokio::test]
async fn queue_fragment_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/now-playing/queue"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn queue_fragment_prompts_reconnect_without_scope() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("zoe", "pw").await;
    let resp = app
        .client()
        .get(app.url("/now-playing/queue"))
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
}

#[tokio::test]
async fn queue_fragment_lists_items_and_links_hub_tracks() {
    let app = spawn_with_queue(json!([queued_item("queued-1", "Queued Track")])).await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    link_spotify(&app.pool, uid, "user-read-playback-state").await;

    let resp = app
        .client()
        .get(app.url("/now-playing/queue"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Queued Track"), "queue item missing: {html}");
    assert!(html.contains("Queue Artist"), "artist missing: {html}");
    assert!(
        !html.contains("Queue ist leer"),
        "must not be empty: {html}"
    );
    // The unknown item is ingested on the fly, so it renders as a hub link.
    assert!(html.contains("/track/"), "hub track link missing: {html}");
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_tracks
          WHERE service = 'spotify' AND service_track_id = 'queued-1'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "queued track should be ingested");
}

#[tokio::test]
async fn queue_fragment_renders_empty_state() {
    let app = spawn_with_queue(json!([])).await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    link_spotify(&app.pool, uid, "user-read-playback-state").await;

    let resp = app
        .client()
        .get(app.url("/now-playing/queue"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("Queue ist leer"),
        "empty state missing: {html}"
    );
}

#[tokio::test]
async fn queue_fragment_reports_spotify_errors() {
    // Point at a base that answers 500 for the queue path.
    async fn boom() -> (axum::http::StatusCode, &'static str) {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom")
    }
    let app_router = axum::Router::new().route("/me/player/queue", get(boom));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app_router).await;
    });

    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let mut cfg = Config::for_test(url);
    cfg.spotify_api_base = format!("http://{addr}");
    let app = common::spawn_with(dir, cfg).await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    link_spotify(&app.pool, uid, "user-read-playback-state").await;

    let resp = app
        .client()
        .get(app.url("/now-playing/queue"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("Spotify-Fehler"),
        "error state missing: {html}"
    );
}

// ── pushing to the queue (M3) ───────────────────────────────────────────────

/// Boot a mock Spotify API that records every `POST /me/player/queue` query and
/// answers `204` like the real one.
async fn mock_queue_post(sink: Arc<Mutex<Vec<String>>>) -> String {
    let handler = move |RawQuery(q): RawQuery| {
        let sink = sink.clone();
        async move {
            sink.lock().unwrap().push(q.unwrap_or_default());
            axum::http::StatusCode::NO_CONTENT
        }
    };
    let app = axum::Router::new().route("/me/player/queue", post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Boot a mock Spotify API whose queue POST always fails the given way.
async fn mock_queue_post_error(status: u16, reason: &'static str) -> String {
    let handler = move || async move {
        (
            axum::http::StatusCode::from_u16(status).unwrap(),
            Json(json!({ "error": { "status": status, "reason": reason } })),
        )
    };
    let app = axum::Router::new().route("/me/player/queue", post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Boot a hub pointed at `base` and return it with a user holding `scopes`.
async fn app_with_queue_post(base: String, scopes: &str) -> (common::TestApp, String) {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let mut cfg = Config::for_test(url);
    cfg.spotify_api_base = base;
    let app = common::spawn_with(dir, cfg).await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    link_spotify(&app.pool, uid, scopes).await;
    (app, cookie)
}

/// POST one track id to the add endpoint, returning `(status, HX-Trigger)`.
async fn queue_add(app: &common::TestApp, cookie: &str, track_id: i64) -> (u16, String) {
    let resp = app
        .client()
        .post(app.url(&format!("/now-playing/queue/add/{track_id}")))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let trigger = resp
        .headers()
        .get("HX-Trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, trigger)
}

#[tokio::test]
async fn queue_add_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url("/now-playing/queue/add/1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn queue_add_without_write_scope_reports_error() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("zoe", "pw").await;
    let (status, trigger) = queue_add(&app, &cookie, app.seed.t_all).await;
    assert_eq!(status, 204);
    assert!(
        toast_msg(&trigger).contains("Schreib-Scope"),
        "scope hint missing: {trigger}"
    );
    assert!(
        trigger.contains("hub:queue-changed"),
        "refresh missing: {trigger}"
    );
}

#[tokio::test]
async fn queue_add_appends_the_resolved_uri() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let base = mock_queue_post(sink.clone()).await;
    let (app, cookie) = app_with_queue_post(base, "user-modify-playback-state").await;

    let (status, trigger) = queue_add(&app, &cookie, app.seed.t_all).await;
    assert_eq!(status, 204);
    assert!(
        trigger.contains("\"kind\":\"ok\""),
        "ok toast missing: {trigger}"
    );

    let calls = sink.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "exactly one POST expected: {calls:?}");
    assert!(
        calls[0].contains("spotify%3Atrack%3At_all"),
        "uri not resolved from the hub track: {:?}",
        calls[0]
    );
}

#[tokio::test]
async fn queue_add_maps_no_active_device() {
    let base = mock_queue_post_error(404, "NO_ACTIVE_DEVICE").await;
    let (app, cookie) = app_with_queue_post(base, "user-modify-playback-state").await;
    let (status, trigger) = queue_add(&app, &cookie, app.seed.t_all).await;
    assert_eq!(status, 204, "errors must not 500: {trigger}");
    assert!(
        toast_msg(&trigger).contains("Kein aktives Spotify-Gerät"),
        "device hint missing: {trigger}"
    );
    assert!(
        trigger.contains("\"kind\":\"err\""),
        "err toast missing: {trigger}"
    );
}

#[tokio::test]
async fn queue_add_reports_missing_track() {
    // Track 0 has no hub row, hence no Spotify id.
    let base = mock_queue_post(Arc::new(Mutex::new(Vec::new()))).await;
    let (app, cookie) = app_with_queue_post(base, "user-modify-playback-state").await;
    let (status, trigger) = queue_add(&app, &cookie, 0).await;
    assert_eq!(status, 204);
    assert!(
        toast_msg(&trigger).contains("Kein Track angegeben"),
        "{trigger}"
    );
}

#[test]
fn player_error_messages_are_actionable() {
    use music_hub::nowplaying::player_error;
    assert!(player_error(404, "NO_ACTIVE_DEVICE", "").contains("Kein aktives Spotify-Gerät"));
    assert!(player_error(403, "PREMIUM_REQUIRED", "").contains("Premium"));
    assert!(player_error(403, "INSUFFICIENT_SCOPE", "").contains("neu verbinden"));
    assert!(player_error(403, "RESTRICTION_VIOLATED", "").contains("erlaubt das hier nicht"));
    assert!(player_error(429, "", "").contains("Rate-Limit"));
    assert!(player_error(500, "SERVER_ERROR", "boom").contains("SERVER_ERROR"));
}

#[test]
fn transport_parses_and_labels_its_commands() {
    use music_hub::nowplaying::Transport;
    assert_eq!(Transport::parse("next"), Some(Transport::Next));
    assert_eq!(Transport::parse("previous"), Some(Transport::Previous));
    assert_eq!(Transport::parse("play"), Some(Transport::Play));
    assert_eq!(Transport::parse("pause"), Some(Transport::Pause));
    assert_eq!(Transport::parse("rewind"), None);
    assert_eq!(Transport::parse(""), None);
}

#[tokio::test]
async fn shell_hosts_the_queue_widget_on_every_page() {
    // The queue is a shell widget (`base.html`), not a page feature — so `/now`
    // and the track page get it for free, and there is nothing to add per page.
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("zoe", "pw").await;
    for path in ["/", "/now", "/settings"] {
        let resp = app
            .client()
            .get(app.url(path))
            .header("Cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK, "{path}");
        let html = resp.text().await.unwrap();
        assert!(
            html.contains("id=\"hub-queue-host\""),
            "queue host missing on {path}"
        );
        assert!(
            html.contains("hx-get=\"/now-playing/queue\""),
            "queue host not wired on {path}"
        );
    }
}

// ── player transport (M4) ──────────────────────────────────────────────────

/// Boot a mock Spotify API that accepts **only** the given method at
/// `/me/player/{cmd}` and records what it saw. Pinning the method here is the
/// point: Spotify splits skip (`POST`) from play/pause (`PUT`), and a wrong verb
/// would 405 live without any mock noticing.
async fn mock_transport(cmd: &'static str, method: &'static str) -> (String, Arc<Mutex<u32>>) {
    let hits = Arc::new(Mutex::new(0u32));
    let seen = hits.clone();
    let handler = move || {
        let seen = seen.clone();
        async move {
            *seen.lock().unwrap() += 1;
            axum::http::StatusCode::NO_CONTENT
        }
    };
    let mut app = axum::Router::new();
    app = match method {
        "POST" => app.route(&format!("/me/player/{cmd}"), axum::routing::post(handler)),
        _ => app.route(&format!("/me/player/{cmd}"), axum::routing::put(handler)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), hits)
}

/// POST a transport command, returning `(status, HX-Trigger)`.
async fn transport(app: &common::TestApp, cookie: &str, cmd: &str) -> (u16, String) {
    let resp = app
        .client()
        .post(app.url(&format!("/now-playing/transport/{cmd}")))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let trigger = resp
        .headers()
        .get("HX-Trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, trigger)
}

#[tokio::test]
async fn transport_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url("/now-playing/transport/next"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn transport_without_write_scope_reports_error() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("zoe", "pw").await;
    let (status, trigger) = transport(&app, &cookie, "next").await;
    assert_eq!(status, 204);
    assert!(toast_msg(&trigger).contains("Schreib-Scope"), "{trigger}");
    assert!(trigger.contains("hub:now-changed"), "{trigger}");
}

#[tokio::test]
async fn transport_rejects_unknown_commands() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("zoe", "pw").await;
    let (status, trigger) = transport(&app, &cookie, "rewind").await;
    assert_eq!(status, 204);
    assert!(
        toast_msg(&trigger).contains("Unbekannter Player-Befehl"),
        "{trigger}"
    );
}

#[tokio::test]
async fn skip_uses_post_and_play_pause_use_put() {
    for (cmd, method, label) in [
        ("next", "POST", "Nächster Track"),
        ("previous", "POST", "Vorheriger Track"),
        ("play", "PUT", "Weiter abspielen"),
        ("pause", "PUT", "Pause"),
    ] {
        let (base, hits) = mock_transport(cmd, method).await;
        let (app, cookie) = app_with_queue_post(base, "user-modify-playback-state").await;
        let (status, trigger) = transport(&app, &cookie, cmd).await;
        assert_eq!(status, 204);
        assert!(
            trigger.contains("\"kind\":\"ok\""),
            "{cmd} ({method}) should succeed, got: {trigger}"
        );
        assert!(
            toast_msg(&trigger).contains(label),
            "{cmd} toast should name the action, got: {trigger}"
        );
        assert_eq!(*hits.lock().unwrap(), 1, "{cmd}: one {method} expected");
    }
}

#[tokio::test]
async fn transport_maps_no_active_device() {
    let handler = || async move {
        (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({ "error": { "status": 404, "reason": "NO_ACTIVE_DEVICE" } })),
        )
    };
    let app_router = axum::Router::new().route("/me/player/next", axum::routing::post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app_router).await;
    });
    let (app, cookie) =
        app_with_queue_post(format!("http://{addr}"), "user-modify-playback-state").await;

    let (status, trigger) = transport(&app, &cookie, "next").await;
    assert_eq!(status, 204, "must not 500: {trigger}");
    assert!(
        toast_msg(&trigger).contains("Kein aktives Spotify-Gerät"),
        "device hint missing: {trigger}"
    );
}

#[tokio::test]
async fn now_widget_offers_play_when_idle_and_pause_when_playing() {
    let idle = widget_html(None).await;
    assert!(
        idle.contains("/now-playing/transport/next"),
        "next missing: {idle}"
    );
    assert!(
        idle.contains("/now-playing/transport/play"),
        "idle must offer play: {idle}"
    );
    assert!(
        !idle.contains("/now-playing/transport/pause"),
        "idle must not offer pause: {idle}"
    );

    let playing = widget_html(Some("np-1")).await;
    assert!(
        playing.contains("/now-playing/transport/pause"),
        "playing must offer pause: {playing}"
    );
    assert!(
        !playing.contains("/now-playing/transport/play"),
        "playing must not offer play: {playing}"
    );
}

/// Render the now-playing widget against a mock Spotify that reports either a
/// specific playing track or nothing at all.
async fn widget_html(playing_sid: Option<&str>) -> String {
    let payload = match playing_sid {
        Some(sid) => json!({ "is_playing": true, "progress_ms": 1000, "item": now_item(sid) }),
        None => json!({ "is_playing": false, "item": Value::Null }),
    };
    let handler = move || {
        let payload = payload.clone();
        async move { Json(payload) }
    };
    let router = axum::Router::new().route("/me/player/currently-playing", get(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let (app, cookie) =
        app_with_queue_post(format!("http://{addr}"), "user-read-currently-playing").await;
    let resp = app
        .client()
        .get(app.url("/now-playing"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    resp.text().await.unwrap()
}

/// The toast message from an `HX-Trigger` header.
///
/// The header is plain ASCII with the text `\u`-escaped (`toast.rs`), so it must
/// be JSON-decoded — that is what the browser does — instead of string-matched.
fn toast_msg(trigger: &str) -> String {
    let v: Value = serde_json::from_str(trigger).unwrap_or(Value::Null);
    v["toast"]["msg"].as_str().unwrap_or_default().to_string()
}

// ── context playback (M4 part 2) ───────────────────────────────────────────

/// Boot a mock Spotify API that accepts `PUT /me/player/play` and records bodies.
async fn mock_play(sink: Arc<Mutex<Vec<Value>>>) -> String {
    let handler = move |Json(body): Json<Value>| {
        let sink = sink.clone();
        async move {
            sink.lock().unwrap().push(body);
            axum::http::StatusCode::NO_CONTENT
        }
    };
    let app = axum::Router::new().route("/me/player/play", axum::routing::put(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Create a tag for `uid` holding `track_ids` in the resolved-tags table.
async fn tag_with_tracks(app: &common::TestApp, uid: i64, track_ids: &[i64]) -> i64 {
    let tag_id = music_hub::tags::ensure_tag(&app.pool, uid, "playme")
        .await
        .unwrap();
    for tid in track_ids {
        sqlx::query("INSERT INTO hub_track_resolved_tags (tag_id, track_id) VALUES (?1, ?2)")
            .bind(tag_id)
            .bind(tid)
            .execute(&app.pool)
            .await
            .unwrap();
    }
    tag_id
}

/// POST a tag to the play endpoint, returning `(status, HX-Trigger)`.
async fn play_tag(app: &common::TestApp, cookie: &str, tag_id: i64) -> (u16, String) {
    let resp = app
        .client()
        .post(app.url(&format!("/now-playing/play/tag/{tag_id}")))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let trigger = resp
        .headers()
        .get("HX-Trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, trigger)
}

#[tokio::test]
async fn play_tag_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url("/now-playing/play/tag/1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn play_tag_without_write_scope_reports_error() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("zoe", "pw").await;
    let (status, trigger) = play_tag(&app, &cookie, 1).await;
    assert_eq!(status, 204);
    assert!(toast_msg(&trigger).contains("Schreib-Scope"), "{trigger}");
}

#[tokio::test]
async fn play_tag_without_spotify_tracks_reports_error() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let base = mock_play(sink.clone()).await;
    let (app, cookie) = app_with_queue_post(base, "user-modify-playback-state").await;
    let uid: i64 = sqlx::query_scalar("SELECT id FROM hub_users WHERE slug = 'zoe'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let tag_id = tag_with_tracks(&app, uid, &[]).await;

    let (status, trigger) = play_tag(&app, &cookie, tag_id).await;
    assert_eq!(status, 204);
    assert!(
        toast_msg(&trigger).contains("Keine Spotify-Tracks"),
        "empty tag must be reported: {trigger}"
    );
    assert!(sink.lock().unwrap().is_empty(), "must not call Spotify");
}

#[tokio::test]
async fn play_tag_sends_the_resolved_uris() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let base = mock_play(sink.clone()).await;
    let (app, cookie) = app_with_queue_post(base, "user-modify-playback-state").await;
    let uid: i64 = sqlx::query_scalar("SELECT id FROM hub_users WHERE slug = 'zoe'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let tag_id = tag_with_tracks(&app, uid, &[app.seed.t_all]).await;

    let (status, trigger) = play_tag(&app, &cookie, tag_id).await;
    assert_eq!(status, 204);
    assert!(trigger.contains("\"kind\":\"ok\""), "{trigger}");
    assert!(toast_msg(&trigger).contains("1 Tracks laufen"), "{trigger}");

    let calls = sink.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        1,
        "one PUT /me/player/play expected: {calls:?}"
    );
    assert_eq!(
        calls[0]["uris"][0], "spotify:track:t_all",
        "uri not resolved from the hub track: {:?}",
        calls[0]
    );
}

#[tokio::test]
async fn play_uris_caps_the_request_and_reports_truncation() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let base = mock_play(sink.clone()).await;
    let (app, _cookie) = app_with_queue_post(base, "user-modify-playback-state").await;
    let uid: i64 = sqlx::query_scalar("SELECT id FROM hub_users WHERE slug = 'zoe'")
        .fetch_one(&app.pool)
        .await
        .unwrap();

    let uris: Vec<String> = (0..150).map(|i| format!("spotify:track:x{i}")).collect();
    let sent = music_hub::nowplaying::play_uris(&app.pool, &app.cfg, uid, &uris)
        .await
        .unwrap();
    assert_eq!(
        sent,
        music_hub::nowplaying::PLAY_URIS_CAP,
        "must cap the uri list"
    );
    let calls = sink.lock().unwrap().clone();
    assert_eq!(
        calls[0]["uris"].as_array().map(|a| a.len()),
        Some(music_hub::nowplaying::PLAY_URIS_CAP)
    );
}

#[tokio::test]
async fn spotify_uris_maps_dedupes_and_skips_unknown_tracks() {
    let app = common::spawn().await;
    let uris =
        music_hub::nowplaying::spotify_uris(&app.pool, &[app.seed.t_all, app.seed.t_all, 999_999])
            .await
            .unwrap();
    assert_eq!(uris, vec!["spotify:track:t_all".to_string()]);
}

/// POST a backpack to the play endpoint, returning `(status, HX-Trigger)`.
async fn play_backpack(app: &common::TestApp, cookie: &str, backpack_id: i64) -> (u16, String) {
    let resp = app
        .client()
        .post(app.url(&format!("/now-playing/play/backpack/{backpack_id}")))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let trigger = resp
        .headers()
        .get("HX-Trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, trigger)
}

#[tokio::test]
async fn play_backpack_sends_the_resolved_uris() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let base = mock_play(sink.clone()).await;
    let (app, cookie) = app_with_queue_post(base, "user-modify-playback-state").await;
    let uid: i64 = sqlx::query_scalar("SELECT id FROM hub_users WHERE slug = 'zoe'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let tag_id = tag_with_tracks(&app, uid, &[app.seed.t_all]).await;
    let bp = music_hub::backpacks::create_backpack(&app.pool, uid, "Play BP")
        .await
        .unwrap();
    music_hub::backpacks::add_source(&app.pool, bp, music_hub::backpacks::SourceRef::Tag(tag_id))
        .await
        .unwrap();

    let (status, trigger) = play_backpack(&app, &cookie, bp).await;
    assert_eq!(status, 204);
    assert!(trigger.contains("\"kind\":\"ok\""), "{trigger}");
    let calls = sink.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "one PUT expected: {calls:?}");
    assert_eq!(calls[0]["uris"][0], "spotify:track:t_all", "{:?}", calls[0]);
}

#[tokio::test]
async fn play_backpack_of_another_user_is_refused() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let base = mock_play(sink.clone()).await;
    let (app, cookie) = app_with_queue_post(base, "user-modify-playback-state").await;
    // Owned by alice from the fixtures, not by the logged-in user.
    let bp = music_hub::backpacks::create_backpack(&app.pool, app.seed.alice, "Alice BP")
        .await
        .unwrap();

    let (status, trigger) = play_backpack(&app, &cookie, bp).await;
    assert_eq!(status, 204);
    assert!(
        toast_msg(&trigger).contains("Backpack nicht gefunden"),
        "must not leak another user's backpack: {trigger}"
    );
    assert!(
        sink.lock().unwrap().is_empty(),
        "must not call Spotify for a foreign backpack"
    );
}

#[tokio::test]
async fn play_backpack_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url("/now-playing/play/backpack/1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}
