//! SoundCloud ingest integration tests against a mock api-v2 server.
//!
//! The api-v2 base is injectable (`ingest_soundcloud_public_with_api_base`), so
//! we boot a tiny in-process Axum server that serves the profile hydration
//! payload (client_id + user id) plus the resolve / playlists / likes endpoints,
//! then assert the resulting DB state. Fully offline.

use axum::response::Html;
use axum::routing::get;
use axum::{Json, Router};
use mmm_hub::{db, ingest};
use serde_json::{Value, json};

const CLIENT_ID: &str = "vI5BsvpTIlavDLl7RDbbcFAPg8kls8Bg";
const USER_ID: i64 = 35227519;

/// The profile page as SoundCloud serves it: a `window.__sc_hydration` array
/// carrying the public `apiClient` id and the owner's user id.
fn hydration_html() -> String {
    format!(
        r#"<html><head><script>window.__sc_hydration = [
            {{"hydratable":"apiClient","data":{{"id":"{CLIENT_ID}"}}}},
            {{"hydratable":"user","data":{{"id":{USER_ID},"permalink":"momokli"}}}}
        ];</script></head><body></body></html>"#
    )
}

fn sc_track(id: i64, title: &str) -> Value {
    json!({
        "kind": "track",
        "id": id,
        "title": title,
        "duration": 180000,
        "genre": "Techno",
        "artwork_url": "https://i1.sndcdn.com/artworks-cover-large.jpg",
        "created_at": "2026-01-01T00:00:00Z",
        "user": { "username": "Test Uploader" },
        "publisher_metadata": { "artist": "Test Uploader", "explicit": false }
    })
}

fn playlist_json() -> Value {
    json!({
        "kind": "playlist",
        "id": 2218790846i64,
        "title": "discover",
        "description": "my set",
        "track_count": 2,
        "tracks": [ sc_track(1962679219, "Track One"), sc_track(1816713384, "Track Two") ]
    })
}

fn like_json(id: i64, title: &str) -> Value {
    json!({
        "created_at": "2026-01-01T00:00:00Z",
        "kind": "like",
        "track": sc_track(id, title)
    })
}

/// Boot a mock SoundCloud (profile pages + api-v2) and return its base URL.
async fn mock_soundcloud() -> String {
    async fn profile() -> Html<String> {
        Html(hydration_html())
    }
    async fn resolve() -> Json<Value> {
        Json(playlist_json())
    }
    async fn playlists() -> Json<Value> {
        Json(json!({ "collection": [playlist_json()], "next_href": null }))
    }
    async fn likes() -> Json<Value> {
        Json(json!({
            "collection": [ like_json(1962679219, "Track One"), like_json(1816713384, "Track Two") ],
            "next_href": null
        }))
    }

    let app = Router::new()
        .route("/momokli/sets/discover", get(profile))
        .route("/momokli/sets", get(profile))
        .route("/momokli/likes", get(profile))
        .route("/resolve", get(resolve))
        .route("/users/{id}/playlists", get(playlists))
        .route("/users/{id}/likes", get(likes));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

struct Fixture {
    base: String,
    pool: sqlx::SqlitePool,
    _dir: tempfile::TempDir,
}

async fn setup() -> Fixture {
    let base = mock_soundcloud().await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let pool = db::connect(&url).await.unwrap();
    Fixture {
        base,
        pool,
        _dir: dir,
    }
}

async fn count(pool: &sqlx::SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn single_set_ingests_playlist_and_tracks() {
    let fx = setup().await;
    let url = format!("{}/momokli/sets/discover", fx.base);

    let summary = ingest::ingest_soundcloud_public_with_api_base(&fx.pool, "momo", &url, &fx.base)
        .await
        .unwrap();

    assert_eq!(summary.playlists, 1);
    assert_eq!(summary.owned_with_items, 1);
    assert_eq!(summary.memberships, 2);

    assert_eq!(
        count(
            &fx.pool,
            "SELECT COUNT(*) FROM hub_tracks WHERE service = 'soundcloud'"
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &fx.pool,
            "SELECT COUNT(*) FROM hub_playlists WHERE service = 'soundcloud'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&fx.pool, "SELECT COUNT(*) FROM hub_playlist_tracks").await,
        2
    );

    // Rich metadata made it through (duration + uploader + artwork).
    let (title, artists, duration, image): (String, String, i64, String) = sqlx::query_as(
        "SELECT title, artists, duration_ms, image_url FROM hub_tracks
          WHERE service_track_id = '1962679219'",
    )
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(title, "Track One");
    assert_eq!(artists, "Test Uploader");
    assert_eq!(duration, 180000);
    assert!(image.contains("artwork"));

    // Playlist metadata.
    let (name, track_count, items): (String, i64, i64) = sqlx::query_as(
        "SELECT name, track_count, items_available FROM hub_playlists WHERE playlist_id = '2218790846'",
    )
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(name, "discover");
    assert_eq!(track_count, 2);
    assert_eq!(items, 1);
}

#[tokio::test]
async fn likes_ingest_populates_liked_tracks() {
    let fx = setup().await;
    let url = format!("{}/momokli/likes", fx.base);

    let summary = ingest::ingest_soundcloud_public_with_api_base(&fx.pool, "momo", &url, &fx.base)
        .await
        .unwrap();

    assert_eq!(summary.liked_tracks, 2);
    assert_eq!(
        count(&fx.pool, "SELECT COUNT(*) FROM hub_liked_tracks").await,
        2
    );
    assert_eq!(
        count(
            &fx.pool,
            "SELECT COUNT(*) FROM hub_tracks WHERE service = 'soundcloud'"
        )
        .await,
        2
    );
}

#[tokio::test]
async fn all_sets_ingest_iterates_playlists() {
    let fx = setup().await;
    let url = format!("{}/momokli/sets", fx.base);

    let summary = ingest::ingest_soundcloud_public_with_api_base(&fx.pool, "momo", &url, &fx.base)
        .await
        .unwrap();

    assert_eq!(summary.playlists, 1);
    assert_eq!(summary.memberships, 2);
    assert_eq!(
        count(
            &fx.pool,
            "SELECT COUNT(*) FROM hub_playlists WHERE service = 'soundcloud'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn re_ingest_is_idempotent() {
    let fx = setup().await;
    let url = format!("{}/momokli/sets/discover", fx.base);

    ingest::ingest_soundcloud_public_with_api_base(&fx.pool, "momo", &url, &fx.base)
        .await
        .unwrap();
    ingest::ingest_soundcloud_public_with_api_base(&fx.pool, "momo", &url, &fx.base)
        .await
        .unwrap();

    assert_eq!(
        count(
            &fx.pool,
            "SELECT COUNT(*) FROM hub_tracks WHERE service = 'soundcloud'"
        )
        .await,
        2
    );
    assert_eq!(
        count(&fx.pool, "SELECT COUNT(*) FROM hub_playlist_tracks").await,
        2
    );
    assert_eq!(
        count(
            &fx.pool,
            "SELECT COUNT(*) FROM hub_playlists WHERE service = 'soundcloud'"
        )
        .await,
        1
    );
}
