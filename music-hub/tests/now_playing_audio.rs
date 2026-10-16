//! Now-playing BPM/key must come from the **same live vault source** as the
//! track page (#466) — not from the lagging `hub_track_audio` read-model.
//!
//! Regression: the widget used to show „— BPM" while `/track/{id}` showed the
//! vault's value, because the widget read the read-model and the page read the
//! vault. Here the read-model is deliberately left **empty** and only the vault
//! has data — the widget must still show the value.

mod common;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use music_hub::config::Config;
use serde_json::{Value, json};

const ISRC: &str = "USAAA0000009";

/// Boot a mock Spotify serving one playing track on `/me/player/currently-playing`.
async fn mock_spotify() -> String {
    async fn currently_playing() -> Json<Value> {
        Json(json!({
            "is_playing": true,
            "progress_ms": 170000,
            "item": {
                "id": "sp-1",
                "name": "Heal Me",
                "artists": [{ "name": "Rodriguez Jr." }],
                "album": { "name": "Baobab", "images": [{ "url": "http://img/x" }] },
                "duration_ms": 374000,
                "explicit": false,
                "external_ids": { "isrc": ISRC }
            }
        }))
    }
    let app = Router::new().route("/me/player/currently-playing", get(currently_playing));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Boot a mock music-api `/vault/track/{isrc}` returning a resolved BPM/key.
async fn mock_vault_track() -> String {
    async fn vault_track(Path(isrc): Path<String>) -> Response {
        if isrc != ISRC {
            return StatusCode::NOT_FOUND.into_response();
        }
        Json(json!({
            "isrc": isrc,
            "resolved": {
                "bpm": 126.0, "key": "11m", "camelot": "8A", "energy": 0.75, "source": "essentia"
            }
        }))
        .into_response()
    }
    let app = Router::new().route("/vault/track/{isrc}", get(vault_track));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn read_model_rows(app: &common::TestApp) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM hub_track_audio")
        .fetch_one(&app.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn now_playing_shows_vault_bpm_even_when_read_model_is_empty() {
    let spotify = mock_spotify().await;
    let music_api = mock_vault_track().await;

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_test(format!("sqlite:{}/hub.db", dir.path().display()));
    cfg.spotify_api_base = spotify;
    cfg.music_api_base = music_api;
    cfg.music_api_token = Some("tok".to_string());
    let app = common::spawn_with(dir, cfg).await;

    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    sqlx::query(
        "INSERT INTO hub_service_accounts
             (user_id, service, access_token, refresh_token, token_expiry, scopes)
         VALUES (?1, 'spotify', 'tok', 'ref', '2099-01-01T00:00:00+00:00',
                 'user-read-currently-playing')",
    )
    .bind(uid)
    .execute(&app.pool)
    .await
    .unwrap();

    // The playing track is in the hub (spotify id + ISRC), so no on-the-fly ingest.
    sqlx::query(
        "UPDATE hub_tracks SET service = 'spotify', service_track_id = 'sp-1', isrc = ?1
          WHERE id = ?2",
    )
    .bind(ISRC)
    .bind(app.seed.t_all)
    .execute(&app.pool)
    .await
    .unwrap();

    // The regression case: the read-model has no row at all.
    assert_eq!(read_model_rows(&app).await, 0, "read-model must start empty");

    let html = app
        .client()
        .get(app.url("/now-playing"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(html.contains("126 BPM"), "vault BPM missing in widget: {html}");
    assert!(html.contains("11m"), "vault key missing in widget: {html}");
    // The widget must not silently fill the read-model as a side effect.
    assert_eq!(
        read_model_rows(&app).await,
        0,
        "widget must not write the read-model"
    );
}
