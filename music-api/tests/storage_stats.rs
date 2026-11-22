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
    // Six roles since #702 added `archived`.
    assert_eq!(body["roles"].as_array().unwrap().len(), 6);
}

#[tokio::test]
async fn storage_stats_counts_orphan_candidates_without_deleting() {
    let h = harness().await;

    // Valid 64-char lowercase hex keys; `path_for` only needs the layout.
    let healthy = "11".repeat(32);
    let file_only = "22".repeat(32);
    let row_only = "33".repeat(32);

    // Healthy pair: file + row.
    let healthy_path = h.state.store.path_for(&healthy);
    std::fs::create_dir_all(healthy_path.parent().unwrap()).unwrap();
    std::fs::write(&healthy_path, b"ok").unwrap();

    // File with no row.
    let file_only_path = h.state.store.path_for(&file_only);
    std::fs::create_dir_all(file_only_path.parent().unwrap()).unwrap();
    std::fs::write(&file_only_path, b"orphan").unwrap();

    for (hash, size) in [(&healthy, 2_i64), (&row_only, 5_i64)] {
        sqlx::query("INSERT INTO store_objects (hash, size, created_at) VALUES (?, ?, 0)")
            .bind(hash)
            .bind(size)
            .execute(&h.state.pool)
            .await
            .unwrap();
    }

    let (status, body) = get_json_authed(&h.app, "/storage/stats").await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["orphanCandidates"]["filesWithoutRow"], 1);
    assert_eq!(body["orphanCandidates"]["rowsWithoutFile"], 1);
    assert_eq!(body["orphanCandidates"]["total"], 2);

    // Read-only: nothing was deleted or rewritten.
    assert!(file_only_path.exists(), "census must never delete files");
    assert!(healthy_path.exists());
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM store_objects")
        .fetch_one(&h.state.pool)
        .await
        .unwrap();
    assert_eq!(rows, 2, "census must never delete rows");
}
