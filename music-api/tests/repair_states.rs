//! #699 — `repair-states` stats every path before touching a row.

mod common;

use common::*;
use music_api::db;

/// Insert a degraded row the way the pre-#699 model left it: a `path_128` on
/// disk but a non-`ready` state with a terminal outcome.
async fn seed_degraded(
    pool: &sqlx::SqlitePool,
    isrc: &str,
    path: &str,
    state: &str,
    terminal: &str,
) {
    let ts = db::now();
    sqlx::query(
        "INSERT INTO tracks
            (isrc, source, state, path_128, error, created_at, updated_at,
             terminal_state, wanted_formats, attempts)
         VALUES (?, 'deezer', ?, ?, 'track is gone (404)', ?, ?, ?, '128', 1)",
    )
    .bind(isrc)
    .bind(state)
    .bind(path)
    .bind(ts)
    .bind(ts)
    .bind(terminal)
    .execute(pool)
    .await
    .unwrap();
}

fn make_file(dir: std::path::PathBuf, name: &str) -> String {
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, b"ID3").unwrap();
    p.display().to_string()
}

#[tokio::test]
async fn repair_states_dry_run_and_apply() {
    let h = harness().await;
    let a = make_file(h.state.config.mp3_128_dir(), "R1.mp3");
    let b = make_file(h.state.config.mp3_128_dir(), "R2.mp3");
    seed_degraded(&h.state.pool, "R1", &a, "failed", "failed").await;
    seed_degraded(&h.state.pool, "R2", &b, "pending", "absent").await;

    // Dry-run: reports, changes nothing.
    let dry = db::repair_states(&h.state.pool, false).await.unwrap();
    assert_eq!(dry.checked, 2);
    assert_eq!(dry.repaired, 2);
    assert_eq!(dry.paths_missing_on_disk, 0);
    let t = db::get_track(&h.state.pool, "R1").await.unwrap().unwrap();
    assert_eq!(t.state, "failed", "dry-run must not mutate");

    // Apply: stats first, then re-derives ready (error preserved).
    let applied = db::repair_states(&h.state.pool, true).await.unwrap();
    assert_eq!(applied.repaired, 2);
    let t = db::get_track(&h.state.pool, "R1").await.unwrap().unwrap();
    assert_eq!(t.state, "ready");
    assert_eq!(
        t.error.as_deref(),
        Some("track is gone (404)"),
        "error kept"
    );
    assert_eq!(t.path_128.as_deref(), Some(a.as_str()));

    // After the repair nothing is servable-but-not-ready.
    let stats = db::storage_stats(&h.state.pool).await.unwrap();
    assert_eq!(stats.servable_but_not_ready, 0);
}

#[tokio::test]
async fn repair_states_never_updates_a_missing_file() {
    let h = harness().await;
    // A path that does not exist on disk.
    let missing = h
        .state
        .config
        .mp3_128_dir()
        .join("ghost.mp3")
        .display()
        .to_string();
    seed_degraded(&h.state.pool, "GHOST", &missing, "failed", "failed").await;

    let stats = db::repair_states(&h.state.pool, true).await.unwrap();
    assert_eq!(stats.checked, 1);
    assert_eq!(stats.repaired, 0, "no stat success → no repair");
    assert_eq!(stats.paths_missing_on_disk, 1);

    let t = db::get_track(&h.state.pool, "GHOST")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, "failed", "row left untouched");
    assert_eq!(t.error.as_deref(), Some("track is gone (404)"));
}
