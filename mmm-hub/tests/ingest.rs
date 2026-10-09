//! Ingest integration tests (issue #139) against a mock Spotify API.
//!
//! The Spotify Web API base is injectable (`Config::spotify_api_base`), so we
//! boot a tiny in-process Axum server, seed a linked account whose token never
//! needs refreshing, run [`mmm_hub::ingest::ingest_user`], and assert the
//! resulting DB state — including cross-user track dedup.

use std::sync::Arc;

use axum::extract::Path;
use axum::routing::get;
use axum::{Json, Router};
use mmm_hub::config::Config;
use mmm_hub::{db, ingest};
use serde_json::{Value, json};

const NOW: &str = "2026-01-01T00:00:00+00:00";

fn track(id: &str, name: &str, isrc: &str) -> Value {
    json!({
        "type": "track",
        "id": id,
        "name": name,
        "is_local": false,
        "artists": [{ "name": "Test Artist" }],
        "album": { "name": "Test Album", "images": [{ "url": "http://img/cover" }] },
        "duration_ms": 180000,
        "explicit": false,
        "external_ids": { "isrc": isrc }
    })
}

fn saved(id: &str, name: &str, isrc: &str) -> Value {
    json!({ "track": track(id, name, isrc), "added_at": NOW })
}

fn item(id: &str, name: &str, isrc: &str) -> Value {
    json!({ "item": track(id, name, isrc), "added_at": NOW })
}

/// Boot a mock Spotify API and return its base URL.
async fn mock_spotify() -> String {
    async fn me() -> Json<Value> {
        Json(json!({ "id": "dave", "display_name": "Dave" }))
    }
    async fn tracks() -> Json<Value> {
        Json(json!({
            "items": [ saved("trk-l1", "Liked One", "ISRC-L1"), saved("trk-l2", "Liked Two", "ISRC-L2") ],
            "next": null
        }))
    }
    async fn playlists() -> Json<Value> {
        Json(json!({
            "items": [
                { "id": "pl-owned", "name": "Owned", "description": "mine",
                  "collaborative": false, "owner": { "id": "dave" },
                  "snapshot_id": "s1", "items": { "total": 2 } },
                { "id": "pl-followed", "name": "Followed", "description": "theirs",
                  "collaborative": false, "owner": { "id": "someone-else" },
                  "snapshot_id": "s2", "items": { "total": 5 } }
            ],
            "next": null
        }))
    }
    async fn playlist_items(Path(id): Path<String>) -> Json<Value> {
        let items = match id.as_str() {
            "pl-owned" => vec![item("trk-p1", "Playlist One", "ISRC-P1"), item("trk-l1", "Liked One", "ISRC-L1")],
            _ => vec![],
        };
        Json(json!({ "items": items, "next": null }))
    }

    let app = Router::new()
        .route("/me", get(me))
        .route("/me/tracks", get(tracks))
        .route("/me/playlists", get(playlists))
        .route("/playlists/{id}/items", get(playlist_items));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Link a Spotify account whose access token is valid far into the future, so
/// no token-refresh round-trip is needed.
async fn link_account(pool: &sqlx::SqlitePool, slug: &str) {
    let user_id = ingest::ensure_user(pool, slug).await.unwrap();
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

struct Fixture {
    cfg: Arc<Config>,
    pool: sqlx::SqlitePool,
    _dir: tempfile::TempDir,
}

async fn setup() -> Fixture {
    let base = mock_spotify().await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let pool = db::connect(&url).await.unwrap();
    let mut cfg = Config::for_test(url);
    cfg.spotify_api_base = base;
    Fixture {
        cfg: Arc::new(cfg),
        pool,
        _dir: dir,
    }
}

async fn count(pool: &sqlx::SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn ingest_classifies_playlists_and_dedups_tracks() {
    let fx = setup().await;
    link_account(&fx.pool, "dave").await;

    let summary = ingest::ingest_user(&fx.pool, &fx.cfg, "dave").await.unwrap();

    assert_eq!(summary.playlists, 2);
    assert_eq!(summary.followed, 1);
    assert_eq!(summary.owned_with_items, 1);
    assert_eq!(summary.liked_tracks, 2);
    assert_eq!(summary.memberships, 2);

    // 3 distinct tracks: trk-l1 (liked + playlisted, one row), trk-l2, trk-p1.
    assert_eq!(count(&fx.pool, "SELECT COUNT(*) FROM hub_tracks").await, 3);

    // Owned playlist got items; followed stayed metadata-only.
    let owned_items: i64 = sqlx::query_scalar(
        "SELECT items_available FROM hub_playlists WHERE playlist_id = 'pl-owned'",
    )
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    let followed_items: i64 = sqlx::query_scalar(
        "SELECT items_available FROM hub_playlists WHERE playlist_id = 'pl-followed'",
    )
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(owned_items, 1);
    assert_eq!(followed_items, 0);

    assert_eq!(count(&fx.pool, "SELECT COUNT(*) FROM hub_liked_tracks").await, 2);
    assert_eq!(count(&fx.pool, "SELECT COUNT(*) FROM hub_playlist_tracks").await, 2);
}

#[tokio::test]
async fn second_user_shares_tracks_without_duplicating_rows() {
    let fx = setup().await;
    link_account(&fx.pool, "dave").await;
    link_account(&fx.pool, "erin").await;

    ingest::ingest_user(&fx.pool, &fx.cfg, "dave").await.unwrap();
    ingest::ingest_user(&fx.pool, &fx.cfg, "erin").await.unwrap();

    // Same global track rows for both users — no duplication.
    assert_eq!(count(&fx.pool, "SELECT COUNT(*) FROM hub_tracks").await, 3);

    // Every track is now present for both users.
    let shared = count(
        &fx.pool,
        "SELECT COUNT(*) FROM hub_v_shared_tracks WHERE user_count = 2",
    )
    .await;
    assert_eq!(shared, 3);

    // The two linked users collapse to one pair.
    assert_eq!(
        count(&fx.pool, "SELECT COUNT(*) FROM hub_v_user_overlap").await,
        1
    );
}

#[tokio::test]
async fn re_ingest_replaces_likes_idempotently() {
    let fx = setup().await;
    link_account(&fx.pool, "dave").await;

    ingest::ingest_user(&fx.pool, &fx.cfg, "dave").await.unwrap();
    ingest::ingest_user(&fx.pool, &fx.cfg, "dave").await.unwrap();

    assert_eq!(count(&fx.pool, "SELECT COUNT(*) FROM hub_liked_tracks").await, 2);
    assert_eq!(count(&fx.pool, "SELECT COUNT(*) FROM hub_tracks").await, 3);
    assert_eq!(
        count(&fx.pool, "SELECT COUNT(*) FROM hub_playlist_tracks").await,
        2
    );
}

#[tokio::test]
async fn ingesting_unknown_user_without_account_fails() {
    let fx = setup().await;
    // No account linked for "ghost".
    let err = ingest::ingest_user(&fx.pool, &fx.cfg, "ghost")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no linked Spotify account"));
    let _ = &fx; // keep tempdir alive
}
