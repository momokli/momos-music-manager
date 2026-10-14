//! `hub_track_audio` read-model + vault consumption (#hub-crawl-everything).
//!
//! The hub never analyses and owns no vault: it pulls the resolved best BPM/key
//! from music-api's frozen consumer surface `GET /vault/tracks?isrcs=` and
//! stores it in the flat read-model `hub_track_audio`.

mod common;

use std::collections::HashMap;

use axum::extract::Query;
use axum::routing::get;
use axum::{Json, Router};
use music_hub::config::Config;
use serde_json::{Value, json};

/// Boot a mock music-api `/vault/tracks` endpoint (bearer auth ignored).
async fn mock_vault() -> String {
    async fn vault_tracks(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
        let isrcs = q.get("isrcs").cloned().unwrap_or_default();
        let mut tracks = Vec::new();
        for isrc in isrcs.split(',').filter(|s| !s.is_empty()) {
            if isrc == "USAAA0000009" {
                tracks.push(json!({
                    "isrc": isrc,
                    "bpm": 126.0,
                    "key": "11m",
                    "camelot": "8A",
                    "energy": 0.75,
                    "source": "essentia",
                    "updatedAt": "2026-01-01T00:00:00+00:00"
                }));
            }
        }
        Json(json!({ "tracks": tracks }))
    }
    let app = Router::new().route("/vault/tracks", get(vault_tracks));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn cfg_with(base: String, token: Option<&str>) -> Config {
    let mut cfg = Config::for_test("sqlite::memory:");
    cfg.music_api_base = base;
    cfg.music_api_token = token.map(str::to_string);
    cfg
}

#[tokio::test]
async fn upsert_keeps_existing_on_null_read() {
    let app = common::spawn().await;
    let t = app.seed.t_all;
    music_hub::analysis::upsert(
        &app.pool,
        t,
        Some(126.0),
        Some("11m"),
        Some("8A"),
        None,
        Some("essentia"),
    )
    .await
    .unwrap();
    // A later "queried, no data" read must not wipe real data.
    music_hub::analysis::upsert(&app.pool, t, None, None, None, None, None)
        .await
        .unwrap();
    let audio = music_hub::analysis::get(&app.pool, t).await.unwrap();
    assert_eq!(audio.bpm, Some(126.0));
    assert_eq!(audio.key.as_deref(), Some("11m"));
    assert_eq!(audio.source.as_deref(), Some("essentia"));
}

#[tokio::test]
async fn get_many_returns_rows() {
    let app = common::spawn().await;
    let a = app.seed.t_all;
    let b = app.seed.t_two;
    music_hub::analysis::upsert(
        &app.pool,
        a,
        Some(126.0),
        Some("11m"),
        Some("8A"),
        None,
        Some("essentia"),
    )
    .await
    .unwrap();
    music_hub::analysis::upsert(&app.pool, b, Some(120.0), None, None, None, Some("traktor"))
        .await
        .unwrap();

    let map = music_hub::analysis::get_many(&app.pool, &[a, b, app.seed.t_pl]).await;
    assert_eq!(map.len(), 2);
    assert_eq!(map[&a].bpm, Some(126.0));
    assert_eq!(map[&b].bpm, Some(120.0));
    assert_eq!(map[&b].source.as_deref(), Some("traktor"));
}

#[tokio::test]
async fn track_page_shows_resolved_bpm_and_source() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    music_hub::analysis::upsert(
        &app.pool,
        app.seed.t_all,
        Some(126.0),
        Some("11m"),
        Some("8A"),
        Some(0.75),
        Some("essentia"),
    )
    .await
    .unwrap();

    let resp = app
        .client()
        .get(app.url(&format!("/track/{}", app.seed.t_all)))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("126"), "resolved BPM shown: {html}");
    assert!(html.contains("essentia"), "BPM source shown: {html}");
}

#[tokio::test]
async fn refresh_without_token_is_noop() {
    let app = common::spawn().await;
    let cfg = cfg_with("http://127.0.0.1:1".to_string(), None);
    let n = music_hub::analysis::refresh_from_vault(&app.pool, &cfg, 10)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn refresh_fills_read_model_from_vault() {
    let base = mock_vault().await;
    let app = common::spawn().await;
    let cfg = cfg_with(base, Some("tok"));

    sqlx::query("UPDATE hub_tracks SET isrc = 'USAAA0000009' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();

    let n = music_hub::analysis::refresh_from_vault(&app.pool, &cfg, 10)
        .await
        .unwrap();
    assert_eq!(n, 1, "one vault row written");

    let audio = music_hub::analysis::get(&app.pool, app.seed.t_all)
        .await
        .unwrap();
    assert_eq!(audio.bpm, Some(126.0));
    assert_eq!(audio.key.as_deref(), Some("11m"));
    assert_eq!(audio.camelot.as_deref(), Some("8A"));
    assert_eq!(audio.energy, Some(0.75));
    assert_eq!(audio.source.as_deref(), Some("essentia"));

    // A second pass finds nothing new (the row now exists).
    let again = music_hub::analysis::refresh_from_vault(&app.pool, &cfg, 10)
        .await
        .unwrap();
    assert_eq!(again, 0);
}

#[tokio::test]
async fn refresh_does_not_poison_on_connection_failure() {
    let app = common::spawn().await;
    // Nothing listens on port 1 -> the request fails.
    let cfg = cfg_with("http://127.0.0.1:1".to_string(), Some("tok"));
    sqlx::query("UPDATE hub_tracks SET isrc = 'USAAA0000009' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();

    let n = music_hub::analysis::refresh_from_vault(&app.pool, &cfg, 10)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_track_audio WHERE track_id = ?1")
        .bind(app.seed.t_all)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "a failed request must not write a no-data row");
}

#[tokio::test]
async fn refresh_records_no_data_for_unknown_isrc() {
    let base = mock_vault().await;
    let app = common::spawn().await;
    let cfg = cfg_with(base, Some("tok"));

    sqlx::query("UPDATE hub_tracks SET isrc = 'UNKNOWN-ISRC' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();

    let n = music_hub::analysis::refresh_from_vault(&app.pool, &cfg, 10)
        .await
        .unwrap();
    assert_eq!(n, 0, "no data written for an unknown ISRC");
    // A "queried, no data" row exists so we don't re-query every pass.
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_track_audio WHERE track_id = ?1")
        .bind(app.seed.t_all)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);
    let audio = music_hub::analysis::get(&app.pool, app.seed.t_all)
        .await
        .unwrap();
    assert!(!audio.has_data());
}
