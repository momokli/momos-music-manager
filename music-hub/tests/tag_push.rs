//! Bi-way playlist push (#283): tag add/remove mirrored to the Spotify source
//! playlist against a mock API (`SPOTIFY_API_BASE`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use music_hub::config::Config;
use music_hub::db;
use music_hub::{ingest, tags};
use serde_json::{Value, json};
use sqlx::SqlitePool;

const NOW: &str = "2026-01-01T00:00:00+00:00";

#[derive(Clone, Default)]
struct Cap {
    added: Arc<Mutex<Vec<Value>>>,
    removed: Arc<Mutex<Vec<Value>>>,
    attempts: Arc<AtomicUsize>,
    fail_first_429: bool,
}

async fn add_items(
    State(cap): State<Cap>,
    Path(_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let n = cap.attempts.fetch_add(1, Ordering::SeqCst);
    if cap.fail_first_429 && n == 0 {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "0")],
            Json(json!({ "error": { "status": 429 } })),
        )
            .into_response();
    }
    cap.added.lock().unwrap().push(body);
    Json(json!({ "snapshot_id": "s" })).into_response()
}

async fn remove_items(
    State(cap): State<Cap>,
    Path(_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    cap.removed.lock().unwrap().push(body);
    Json(json!({ "snapshot_id": "s" })).into_response()
}

async fn mock_spotify(fail_first_429: bool) -> (String, Cap) {
    let cap = Cap {
        fail_first_429,
        ..Cap::default()
    };
    let app = Router::new()
        .route(
            "/playlists/{id}/items",
            post(add_items).delete(remove_items),
        )
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), cap)
}

struct Fx {
    cfg: Arc<Config>,
    pool: SqlitePool,
    cap: Cap,
    user_id: i64,
    _dir: tempfile::TempDir,
}

async fn setup(scopes: &str, fail_first_429: bool) -> Fx {
    let (base, cap) = mock_spotify(fail_first_429).await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let pool = db::connect(&url).await.unwrap();
    let mut cfg = Config::for_test(url);
    cfg.spotify_api_base = base;

    let user_id = ingest::ensure_user(&pool, "dave").await.unwrap();
    sqlx::query(
        "INSERT INTO hub_service_accounts
             (user_id, service, access_token, refresh_token, token_expiry, scopes, authorized_at, connected_at)
         VALUES (?1, 'spotify', 'tok', 'ref', '2099-01-01T00:00:00+00:00', ?2, ?3, ?3)",
    )
    .bind(user_id)
    .bind(scopes)
    .bind(NOW)
    .execute(&pool)
    .await
    .unwrap();

    Fx {
        cfg: Arc::new(cfg),
        pool,
        cap,
        user_id,
        _dir: dir,
    }
}

async fn track(pool: &SqlitePool, sid: &str) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists, first_seen_at)
         VALUES ('spotify', ?1, 'T', 'Artist', ?2) RETURNING id",
    )
    .bind(sid)
    .bind(NOW)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO hub_track_external_ids (track_id, service, external_id)
         VALUES (?1, 'spotify', ?2)",
    )
    .bind(id)
    .bind(sid)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Owned source playlist (`is_owned=1`) with the given remote id.
async fn owned_playlist(pool: &SqlitePool, user_id: i64, remote: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, is_owned)
         VALUES (?1, 'spotify', ?2, 'My Rem', 1) RETURNING id",
    )
    .bind(user_id)
    .bind(remote)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A tag opted into bi-way sync, linked as a source of `playlist_id`.
async fn synced_tag(pool: &SqlitePool, user_id: i64, playlist_id: i64, keep: bool) -> i64 {
    let tag_id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at, sync_playlist)
         VALUES (?1, 'groovy', 'Groovy', ?2, 1) RETURNING id",
    )
    .bind(user_id)
    .bind(NOW)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO hub_tag_sources (tag_id, playlist_id, user_id, service, keep_on_remove)
         VALUES (?1, ?2, ?3, 'spotify', ?4)",
    )
    .bind(tag_id)
    .bind(playlist_id)
    .bind(user_id)
    .bind(keep as i64)
    .execute(pool)
    .await
    .unwrap();
    tag_id
}

async fn push_status(pool: &SqlitePool, playlist_id: i64) -> (String, String, String) {
    sqlx::query_as::<_, (String, String, String)>(
        "SELECT COALESCE(last_push_status,''), COALESCE(last_push_action,''),
                COALESCE(last_push_error,'')
           FROM hub_playlists WHERE id = ?1",
    )
    .bind(playlist_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn tag_add_pushes_uri_to_spotify() {
    let fx = setup("playlist-modify-private", false).await;
    let pl = owned_playlist(&fx.pool, fx.user_id, "rem").await;
    let tag = synced_tag(&fx.pool, fx.user_id, pl, false).await;
    let t = track(&fx.pool, "spTRK").await;

    tags::tag_track(&fx.pool, &fx.cfg, fx.user_id, t, tag)
        .await
        .unwrap();

    let added = fx.cap.added.lock().unwrap().clone();
    assert_eq!(added.len(), 1, "one POST to Spotify");
    let uris = added[0]["uris"].as_array().unwrap();
    assert_eq!(uris[0].as_str().unwrap(), "spotify:track:spTRK");

    let (status, action, _) = push_status(&fx.pool, pl).await;
    assert_eq!(status, "ok");
    assert_eq!(action, "add");
    let linked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
    )
    .bind(pl)
    .bind(t)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(linked, 1, "hub mirror updated");
}

#[tokio::test]
async fn untag_removes_from_spotify() {
    let fx = setup("playlist-modify-private", false).await;
    let pl = owned_playlist(&fx.pool, fx.user_id, "rem").await;
    let tag = synced_tag(&fx.pool, fx.user_id, pl, false).await;
    let t = track(&fx.pool, "spTRK").await;

    tags::tag_track(&fx.pool, &fx.cfg, fx.user_id, t, tag)
        .await
        .unwrap();
    tags::untag_track(&fx.pool, &fx.cfg, fx.user_id, t, tag)
        .await
        .unwrap();

    let removed = fx.cap.removed.lock().unwrap().clone();
    assert_eq!(removed.len(), 1, "one DELETE to Spotify");
    let tracks = removed[0]["tracks"].as_array().unwrap();
    assert_eq!(tracks[0]["uri"].as_str().unwrap(), "spotify:track:spTRK");

    let (status, action, _) = push_status(&fx.pool, pl).await;
    assert_eq!(status, "ok");
    assert_eq!(action, "remove");
    let linked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
    )
    .bind(pl)
    .bind(t)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(linked, 0, "hub mirror row removed");
}

#[tokio::test]
async fn missing_scope_skips_http_call() {
    let fx = setup("user-read-private", false).await;
    let pl = owned_playlist(&fx.pool, fx.user_id, "rem").await;
    let tag = synced_tag(&fx.pool, fx.user_id, pl, false).await;
    let t = track(&fx.pool, "spTRK").await;

    tags::tag_track(&fx.pool, &fx.cfg, fx.user_id, t, tag)
        .await
        .unwrap();

    assert!(fx.cap.added.lock().unwrap().is_empty(), "no HTTP call");
    let (status, _, _) = push_status(&fx.pool, pl).await;
    assert_eq!(status, "scope_missing");
}

#[tokio::test]
async fn unowned_playlist_is_skipped() {
    let fx = setup("playlist-modify-private", false).await;
    let pl: i64 = sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, is_owned, collaborative)
         VALUES (?1, 'spotify', 'rem', 'Foreign', 0, 0) RETURNING id",
    )
    .bind(fx.user_id)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    let tag = synced_tag(&fx.pool, fx.user_id, pl, false).await;
    let t = track(&fx.pool, "spTRK").await;

    tags::tag_track(&fx.pool, &fx.cfg, fx.user_id, t, tag)
        .await
        .unwrap();

    assert!(fx.cap.added.lock().unwrap().is_empty(), "no HTTP call");
    let (status, _, _) = push_status(&fx.pool, pl).await;
    assert_eq!(status, "skipped");
}

#[tokio::test]
async fn collaborative_playlist_is_pushed() {
    let fx = setup("playlist-modify-private", false).await;
    let pl: i64 = sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, is_owned, collaborative)
         VALUES (?1, 'spotify', 'rem', 'Collab', 0, 1) RETURNING id",
    )
    .bind(fx.user_id)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    let tag = synced_tag(&fx.pool, fx.user_id, pl, false).await;
    let t = track(&fx.pool, "spTRK").await;

    tags::tag_track(&fx.pool, &fx.cfg, fx.user_id, t, tag)
        .await
        .unwrap();

    assert_eq!(fx.cap.added.lock().unwrap().len(), 1, "collab is writable");
}

#[tokio::test]
async fn keep_on_remove_archives_instead_of_removing() {
    let fx = setup("playlist-modify-private", false).await;
    let pl = owned_playlist(&fx.pool, fx.user_id, "rem").await;
    let tag = synced_tag(&fx.pool, fx.user_id, pl, true).await;
    let t = track(&fx.pool, "spTRK").await;

    tags::tag_track(&fx.pool, &fx.cfg, fx.user_id, t, tag)
        .await
        .unwrap();
    tags::untag_track(&fx.pool, &fx.cfg, fx.user_id, t, tag)
        .await
        .unwrap();

    assert!(
        fx.cap.removed.lock().unwrap().is_empty(),
        "no Spotify DELETE for keep_on_remove"
    );
    let archived: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_tag_archive WHERE tag_id = ?1 AND track_id = ?2",
    )
    .bind(tag)
    .bind(t)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(archived, 1, "link archived");
}

#[tokio::test]
async fn rate_limit_is_retried_once() {
    let fx = setup("playlist-modify-private", true).await;
    let pl = owned_playlist(&fx.pool, fx.user_id, "rem").await;
    let tag = synced_tag(&fx.pool, fx.user_id, pl, false).await;
    let t = track(&fx.pool, "spTRK").await;

    tags::tag_track(&fx.pool, &fx.cfg, fx.user_id, t, tag)
        .await
        .unwrap();

    assert_eq!(
        fx.cap.attempts.load(Ordering::SeqCst),
        2,
        "429 then one retry"
    );
    assert_eq!(fx.cap.added.lock().unwrap().len(), 1, "succeeded after retry");
    let (status, _, _) = push_status(&fx.pool, pl).await;
    assert_eq!(status, "ok");
}
