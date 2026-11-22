//! #699 — `GET /storage/stats` exposes the two completeness counters.

mod common;

use common::*;
use music_api::db;

fn make_file(dir: std::path::PathBuf, name: &str) -> String {
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, b"ID3").unwrap();
    p.display().to_string()
}

/// Insert a row with an explicit `state` + `path_128`.
async fn seed(
    pool: &sqlx::SqlitePool,
    isrc: &str,
    state: &str,
    path128: Option<&str>,
    terminal: Option<&str>,
) {
    let ts = db::now();
    sqlx::query(
        "INSERT INTO tracks
            (isrc, source, state, path_128, created_at, updated_at, terminal_state, wanted_formats)
         VALUES (?, 'deezer', ?, ?, ?, ?, ?, '128')",
    )
    .bind(isrc)
    .bind(state)
    .bind(path128)
    .bind(ts)
    .bind(ts)
    .bind(terminal)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn storage_stats_reports_both_counters() {
    let h = harness().await;
    let on_disk = make_file(h.state.config.mp3_128_dir(), "OK.mp3");
    let missing = h
        .state
        .config
        .mp3_128_dir()
        .join("gone.mp3")
        .display()
        .to_string();

    // A healthy ready row.
    seed(&h.state.pool, "OK", "ready", Some(&on_disk), None).await;
    // Degraded: servable file but non-ready state.
    seed(
        &h.state.pool,
        "DEGR",
        "failed",
        Some(&on_disk),
        Some("failed"),
    )
    .await;
    // A ready row whose file vanished.
    seed(&h.state.pool, "MISS", "ready", Some(&missing), None).await;

    let (status, body) = get_json_authed(&h.app, "/storage/stats").await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["servableButNotReady"], 1);
    assert_eq!(body["pathsMissingOnDisk"], 1);
}

#[tokio::test]
async fn storage_stats_is_empty_when_clean() {
    let h = harness().await;
    let (status, body) = get_json_authed(&h.app, "/storage/stats").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["servableButNotReady"], 0);
    assert_eq!(body["pathsMissingOnDisk"], 0);
}
