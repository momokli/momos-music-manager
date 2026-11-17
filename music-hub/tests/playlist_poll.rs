//! Playlist listing poll (#506): keeps `hub_playlists.snapshot_id` fresh
//! without a user-triggered "Fetch".
//!
//! Each test drives one `playlist_listing_pass` against a mock Spotify API —
//! the background loops themselves are only started by `main`.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use music_hub::config::Config;
use music_hub::worker::{playlist_listing_pass, PlaylistPollOutcome};
use serde_json::{Value, json};

const NOW: &str = "2026-01-01T00:00:00+00:00";

#[derive(Clone, Default)]
struct Hits(Arc<Mutex<Vec<String>>>);

impl Hits {
    fn paths(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Clone)]
struct Mock {
    hits: Hits,
    limited: bool,
}

async fn me(State(m): State<Mock>, uri: Uri) -> Json<Value> {
    m.hits.0.lock().unwrap().push(uri.path().to_string());
    Json(json!({ "id": "remote-1", "display_name": "Alice" }))
}

async fn listing(State(m): State<Mock>, uri: Uri) -> impl IntoResponse {
    m.hits.0.lock().unwrap().push(uri.path().to_string());
    if m.limited {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "0")],
            Json(json!({ "error": { "status": 429, "message": "rate limited" } })),
        )
            .into_response();
    }
    Json(json!({
        "items": [{
            "id": "pl-1",
            "name": "Mix",
            "snapshot_id": "new-snap",
            "owner": { "id": "remote-1", "display_name": "Alice" },
            "collaborative": false,
            "items": { "total": 2 }
        }],
        "next": null
    }))
    .into_response()
}

/// Boot a mock Spotify API (`/me` + `/me/playlists`) and return `(base, hits)`.
async fn mock_spotify(rate_limited_listing: bool) -> (String, Hits) {
    let hits = Hits::default();
    let mock = Mock {
        hits: hits.clone(),
        limited: rate_limited_listing,
    };
    let app = Router::new()
        .route("/me", get(me))
        .route("/me/playlists", get(listing))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), hits)
}

async fn setup(spotify_base: &str) -> (tempfile::TempDir, Config, sqlx::SqlitePool) {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let pool = music_hub::db::connect(&url).await.expect("connect + migrate");
    let mut cfg = Config::for_test(url);
    cfg.spotify_api_base = spotify_base.to_string();
    (dir, cfg, pool)
}

async fn link_token(pool: &sqlx::SqlitePool, slug: &str, token: Option<&str>) -> i64 {
    let user_id = music_hub::ingest::ensure_user(pool, slug).await.unwrap();
    sqlx::query(
        "INSERT INTO hub_service_accounts
             (user_id, service, access_token, refresh_token, token_expiry, scopes,
              authorized_at, connected_at)
         VALUES (?1, 'spotify', ?2, NULL, '2099-01-01T00:00:00+00:00', 'test', ?3, ?3)",
    )
    .bind(user_id)
    .bind(token)
    .bind(NOW)
    .execute(pool)
    .await
    .unwrap();
    user_id
}

async fn seed_playlist(pool: &sqlx::SqlitePool, user_id: i64, snapshot: &str) {
    sqlx::query(
        "INSERT INTO hub_playlists
             (user_id, service, playlist_id, name, snapshot_id, items_available,
              last_synced_snapshot, enabled_for_fetch, track_count)
         VALUES (?1, 'spotify', 'pl-1', 'Mix', ?2, 1, ?2, 1, 2)",
    )
    .bind(user_id)
    .bind(snapshot)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn poll_updates_snapshot_id() {
    let (base, hits) = mock_spotify(false).await;
    let (_dir, cfg, pool) = setup(&base).await;
    let user_id = link_token(&pool, "alice", Some("mock-access-token")).await;
    seed_playlist(&pool, user_id, "old-snap").await;

    let out = playlist_listing_pass(&pool, &cfg).await.expect("pass ok");
    let PlaylistPollOutcome::Ok(stats) = out else {
        panic!("expected Ok, got {out:?}");
    };
    assert_eq!(stats.users, 1);
    assert_eq!(stats.requests, 2, "profile + one listing page");
    assert_eq!(stats.updated, 1);

    let snapshot: Option<String> =
        sqlx::query_scalar("SELECT snapshot_id FROM hub_playlists WHERE playlist_id = 'pl-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(snapshot.as_deref(), Some("new-snap"));

    // Listing-only: no `/playlists/{id}/items` request.
    let paths = hits.paths();
    assert_eq!(paths, vec!["/me".to_string(), "/me/playlists".to_string()]);
}

#[tokio::test]
async fn rate_limit_backs_off_without_retry_storm() {
    let (base, hits) = mock_spotify(true).await;
    let (_dir, cfg, pool) = setup(&base).await;
    let user_id = link_token(&pool, "alice", Some("mock-access-token")).await;
    seed_playlist(&pool, user_id, "old-snap").await;

    let out = playlist_listing_pass(&pool, &cfg).await.expect("pass ok");
    match out {
        PlaylistPollOutcome::RateLimited { retry_after, stats } => {
            assert_eq!(retry_after, Some(0));
            assert_eq!(stats.requests, 2, "profile + exactly one listing attempt");
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }

    // One profile call and exactly one listing call — no retry loop.
    let listing_hits = hits
        .paths()
        .iter()
        .filter(|p| p.as_str() == "/me/playlists")
        .count();
    assert_eq!(listing_hits, 1, "no retry storm");
    assert_eq!(hits.paths().len(), 2);

    // Nothing was updated on the rate-limited pass.
    let snapshot: Option<String> =
        sqlx::query_scalar("SELECT snapshot_id FROM hub_playlists WHERE playlist_id = 'pl-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(snapshot.as_deref(), Some("old-snap"));
}

#[tokio::test]
async fn user_without_token_makes_no_request() {
    let (base, hits) = mock_spotify(false).await;
    let (_dir, cfg, pool) = setup(&base).await;
    link_token(&pool, "notoken", None).await;

    let out = playlist_listing_pass(&pool, &cfg).await.expect("pass ok");
    let PlaylistPollOutcome::Ok(stats) = out else {
        panic!("expected Ok, got {out:?}");
    };
    assert_eq!(stats.users, 0);
    assert_eq!(stats.requests, 0);
    assert_eq!(stats.updated, 0);
    assert!(hits.paths().is_empty(), "no HTTP request without a token");
}