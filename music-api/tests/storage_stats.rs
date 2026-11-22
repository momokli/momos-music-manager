//! #700 — `GET /storage/stats` aggregates the `track_files` catalogue.
//!
//! (#699's `servableButNotReady` counter stays as a DB-only compatibility
//! field; the filesystem-scanning `pathsMissingOnDisk` is gone — drift is now
//! surfaced by the backfill's `unmanaged` report.)

mod common;

use common::*;
use music_api::db;

async fn seed_track(
    pool: &sqlx::SqlitePool,
    isrc: &str,
    state: &str,
    path128: Option<&str>,
    wanted: &str,
) {
    let ts = db::now();
    sqlx::query(
        "INSERT INTO tracks
            (isrc, source, state, path_128, created_at, updated_at, wanted_formats)
         VALUES (?, 'deezer', ?, ?, ?, ?, ?)",
    )
    .bind(isrc)
    .bind(state)
    .bind(path128)
    .bind(ts)
    .bind(ts)
    .bind(wanted)
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_file(pool: &sqlx::SqlitePool, isrc: &str, role: &str, size: i64) {
    sqlx::query(
        "INSERT INTO track_files (isrc, role, path, size, created_at)
         VALUES (?, ?, ?, ?, '2026-11-22T00:00:00Z')",
    )
    .bind(isrc)
    .bind(role)
    .bind(format!("128/{isrc}.mp3"))
    .bind(size)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn storage_stats_reports_roles_and_gaps() {
    let h = harness().await;

    // Complete track: one 128 file for a 128 demand.
    seed_track(&h.state.pool, "OK", "ready", Some("/x/OK.mp3"), "128").await;
    seed_file(&h.state.pool, "OK", "128", 100).await;

    // Incomplete: demands flac + 128, only 128 catalogued.
    seed_track(&h.state.pool, "INC", "ready", None, "flac,128").await;
    seed_file(&h.state.pool, "INC", "128", 50).await;

    // No file rows at all.
    seed_track(&h.state.pool, "NONE", "pending", None, "128").await;

    // Degraded servable row (#699 compatibility counter).
    seed_track(&h.state.pool, "DEGR", "failed", Some("/x/DEGR.mp3"), "128").await;
    seed_file(&h.state.pool, "DEGR", "128", 25).await;

    sqlx::query("INSERT INTO track_files_unmanaged (path, reason, size) VALUES ('128/junk.mp3', 'test', 5)")
        .execute(&h.state.pool)
        .await
        .unwrap();

    let (status, body) = get_json_authed(&h.app, "/storage/stats").await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");

    assert_eq!(body["totalFiles"], 3);
    assert_eq!(body["totalBytes"], 175);
    assert_eq!(body["tracksWithoutFiles"], 1);
    assert_eq!(body["incompleteTracks"], 1);
    assert_eq!(body["unmanagedFiles"], 1);
    assert_eq!(body["servableButNotReady"], 1);

    let role = |name: &str| {
        body["roles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == name)
            .unwrap_or_else(|| panic!("role {name} missing in {body}"))
            .clone()
    };
    assert_eq!(role("128")["files"], 3);
    assert_eq!(role("128")["bytes"], 175);
    assert_eq!(role("flac")["files"], 0);
}

#[tokio::test]
async fn storage_stats_is_empty_when_clean() {
    let h = harness().await;
    let (status, body) = get_json_authed(&h.app, "/storage/stats").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["totalFiles"], 0);
    assert_eq!(body["tracksWithoutFiles"], 0);
    assert_eq!(body["incompleteTracks"], 0);
    assert_eq!(body["unmanagedFiles"], 0);
    assert_eq!(body["servableButNotReady"], 0);
    assert_eq!(body["roles"].as_array().unwrap().len(), 5);
}