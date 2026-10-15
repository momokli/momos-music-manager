//! music-api → hub metadata backfill (#354).
//!
//! The hub reads BPM/key from music-api's ISRC authority (`GET /metadata?isrcs=`)
//! and UPSERTs it into `hub_track_features` (`source='music-api'`), never
//! poisoning rows on a transient outage.

mod common;

use std::collections::HashMap;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use music_hub::config::Config;
use serde_json::{Value, json};

/// Boot a mock music-api `/metadata` endpoint (bearer auth ignored). Only
/// `USAAA0000009` has data; every other ISRC comes back in `missing`.
async fn mock_metadata() -> String {
    async fn metadata(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
        let isrcs = q.get("isrcs").cloned().unwrap_or_default();
        let mut items = Vec::new();
        let mut missing = Vec::new();
        for isrc in isrcs.split(',').filter(|s| !s.is_empty()) {
            if isrc == "USAAA0000009" {
                items.push(json!({
                    "isrc": isrc,
                    "bpm": 126.0,
                    "musicalKey": "Am",
                    "camelot": "8A",
                    "source": "essentia",
                    "updatedAt": 1700000000
                }));
            } else {
                missing.push(isrc);
            }
        }
        Json(json!({ "items": items, "missing": missing }))
    }
    let app = Router::new().route("/metadata", get(metadata));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Boot a mock music-api `/metadata` endpoint that always answers `code`.
async fn mock_metadata_status(code: u16) -> String {
    let status = StatusCode::from_u16(code).unwrap();
    let app = Router::new().route("/metadata", get(move || async move { status }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Boot a mock music-api `/metadata` endpoint answering a non-JSON body.
async fn mock_metadata_garbage() -> String {
    async fn garbage() -> &'static str {
        "<html>not json</html>"
    }
    let app = Router::new().route("/metadata", get(garbage));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Boot a mock music-api `/metadata` endpoint that knows nothing at all.
async fn mock_metadata_empty() -> String {
    async fn empty() -> Json<Value> {
        Json(json!({ "items": [], "missing": [] }))
    }
    let app = Router::new().route("/metadata", get(empty));
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

// ── music_api::metadata_range (client contract) ────────────────────────────

#[tokio::test]
async fn metadata_range_reads_items_and_missing() {
    let base = mock_metadata().await;
    let cfg = cfg_with(base, Some("tok"));
    let (items, missing) = music_hub::music_api::metadata_range(
        &cfg,
        &["USAAA0000009".to_string(), "USAAA0000010".to_string()],
    )
    .await
    .expect("successful reply is Some");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].isrc, "USAAA0000009");
    assert_eq!(items[0].bpm, Some(126.0));
    assert_eq!(items[0].musical_key(), Some("Am"));
    assert_eq!(items[0].camelot.as_deref(), Some("8A"));
    assert_eq!(missing, vec!["USAAA0000010".to_string()]);
}

#[tokio::test]
async fn metadata_range_empty_input_is_empty_some() {
    // No network needed, and no token required for the empty short-circuit.
    let cfg = cfg_with("http://127.0.0.1:1".to_string(), Some("tok"));
    let (items, missing) = music_hub::music_api::metadata_range(&cfg, &[])
        .await
        .expect("empty input is Some");
    assert!(items.is_empty());
    assert!(missing.is_empty());
}

#[tokio::test]
async fn metadata_range_none_without_token() {
    let base = mock_metadata().await;
    let cfg = cfg_with(base, None);
    assert!(
        music_hub::music_api::metadata_range(&cfg, &["USAAA0000009".to_string()])
            .await
            .is_none()
    );
}

#[tokio::test]
async fn metadata_range_none_on_server_error() {
    let base = mock_metadata_status(500).await;
    let cfg = cfg_with(base, Some("tok"));
    assert!(
        music_hub::music_api::metadata_range(&cfg, &["USAAA0000009".to_string()])
            .await
            .is_none(),
        "500 must be None, not an empty Some"
    );
}

#[tokio::test]
async fn metadata_range_none_on_malformed_body() {
    let base = mock_metadata_garbage().await;
    let cfg = cfg_with(base, Some("tok"));
    assert!(
        music_hub::music_api::metadata_range(&cfg, &["USAAA0000009".to_string()])
            .await
            .is_none()
    );
}

#[tokio::test]
async fn metadata_range_empty_reply_is_some_empty() {
    let base = mock_metadata_empty().await;
    let cfg = cfg_with(base, Some("tok"));
    let (items, missing) = music_hub::music_api::metadata_range(
        &cfg,
        &["USAAA0000009".to_string()],
    )
    .await
    .expect("empty payload is Some");
    assert!(items.is_empty());
    assert!(missing.is_empty());
}

// ── backfill semantics ─────────────────────────────────────────────────────

#[tokio::test]
async fn backfill_transient_failure_does_not_poison() {
    let base = mock_metadata_status(500).await;
    let app = common::spawn().await;
    let cfg = cfg_with(base, Some("tok"));

    sqlx::query("UPDATE hub_tracks SET isrc = 'USAAA0000009' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();

    let n = music_hub::features::backfill_from_music_api(&app.pool, &cfg, 10)
        .await
        .unwrap();
    assert_eq!(n, 0, "transient failure writes nothing");
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_features WHERE track_id = ?1",
    )
    .bind(app.seed.t_all)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(rows, 0, "no check marker written on a transient failure");
}

#[tokio::test]
async fn backfill_without_token_is_noop() {
    let app = common::spawn().await;
    let cfg = cfg_with("http://127.0.0.1:1".to_string(), None);
    let n = music_hub::features::backfill_from_music_api(&app.pool, &cfg, 10)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn backfill_writes_hit_and_marks_miss() {
    let base = mock_metadata().await;
    let app = common::spawn().await;
    let cfg = cfg_with(base, Some("tok"));

    // One ISRC the authority knows, one it does not.
    sqlx::query("UPDATE hub_tracks SET isrc = 'USAAA0000009' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE hub_tracks SET isrc = 'USAAA0000010' WHERE id = ?1")
        .bind(app.seed.t_two)
        .execute(&app.pool)
        .await
        .unwrap();

    let n = music_hub::features::backfill_from_music_api(&app.pool, &cfg, 10)
        .await
        .unwrap();
    assert_eq!(n, 1, "one data-contributing row");

    // Hit: BPM + camelot + derived pitch/mode, source='music-api'.
    let row = sqlx::query(
        "SELECT bpm, camelot, key_pitch, key_mode, source, metadata_checked_at
           FROM hub_track_features WHERE track_id = ?1",
    )
    .bind(app.seed.t_all)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    use sqlx::Row;
    assert_eq!(row.get::<Option<f64>, _>("bpm"), Some(126.0));
    assert_eq!(row.get::<Option<String>, _>("camelot").as_deref(), Some("8A"));
    assert_eq!(row.get::<Option<i64>, _>("key_pitch"), Some(9));
    assert_eq!(row.get::<Option<i64>, _>("key_mode"), Some(0));
    assert_eq!(row.get::<Option<String>, _>("source").as_deref(), Some("music-api"));
    assert!(row.get::<Option<String>, _>("metadata_checked_at").is_some());

    // Miss: only the check marker, no data.
    let miss = sqlx::query(
        "SELECT found, bpm, camelot, metadata_checked_at
           FROM hub_track_features WHERE track_id = ?1",
    )
    .bind(app.seed.t_two)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(miss.get::<i64, _>("found"), 0);
    assert_eq!(miss.get::<Option<f64>, _>("bpm"), None);
    assert!(miss.get::<Option<String>, _>("camelot").is_none());
    assert!(miss.get::<Option<String>, _>("metadata_checked_at").is_some());
}

#[tokio::test]
async fn backfill_preserves_unrelated_columns_on_hit() {
    let base = mock_metadata().await;
    let app = common::spawn().await;
    let cfg = cfg_with(base, Some("tok"));

    sqlx::query("UPDATE hub_tracks SET isrc = 'USAAA0000009' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();
    // A selected row (no bpm/camelot) that already carries ReccoBeats energy.
    // The metadata UPSERT must fill bpm/camelot without touching `energy`.
    sqlx::query(
        "INSERT INTO hub_track_features (track_id, found, energy, source, fetched_at)
         VALUES (?1, 1, 0.5, 'reccobeats', '2026-01-01T00:00:00+00:00')",
    )
    .bind(app.seed.t_all)
    .execute(&app.pool)
    .await
    .unwrap();

    let n = music_hub::features::backfill_from_music_api(&app.pool, &cfg, 10)
        .await
        .unwrap();
    assert_eq!(n, 1);

    let row = sqlx::query(
        "SELECT bpm, camelot, energy FROM hub_track_features WHERE track_id = ?1",
    )
    .bind(app.seed.t_all)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    use sqlx::Row;
    assert_eq!(row.get::<Option<f64>, _>("bpm"), Some(126.0), "bpm filled");
    assert_eq!(row.get::<Option<String>, _>("camelot").as_deref(), Some("8A"), "key filled");
    assert_eq!(row.get::<Option<f64>, _>("energy"), Some(0.5), "energy untouched");
}

#[tokio::test]
async fn backfill_recheck_ttl_skips_fresh_misses() {
    let base = mock_metadata().await;
    let app = common::spawn().await;
    let cfg = cfg_with(base, Some("tok"));

    sqlx::query("UPDATE hub_tracks SET isrc = 'USAAA0000010' WHERE id = ?1")
        .bind(app.seed.t_two)
        .execute(&app.pool)
        .await
        .unwrap();

    // First pass records a fresh "checked, no data" marker.
    assert_eq!(
        music_hub::features::backfill_from_music_api(&app.pool, &cfg, 10)
            .await
            .unwrap(),
        0
    );
    // Second pass finds nothing new (the marker is within the TTL).
    assert_eq!(
        music_hub::features::backfill_from_music_api(&app.pool, &cfg, 10)
            .await
            .unwrap(),
        0
    );
    let seen: i64 = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM hub_track_features WHERE track_id = ?1",
    )
    .bind(app.seed.t_two)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(seen, 1, "marker row exists exactly once");
}
