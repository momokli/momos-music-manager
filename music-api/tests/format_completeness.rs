//! #699 — format completeness instead of a quality rank.
//!
//! Paths are additive, a servable row is never degraded, and demand is a format
//! *set*. `tracks.state` is derived by the single writer `db::recompute_state`.
//! (#699 grep evidence: `grep -rn "UPDATE tracks" music-api/src | grep "state ="`
//! matches only `db::recompute_state`.)

mod common;

use common::*;
use music_api::{db, models, work};

fn tmp_set(h: &Harness, name: &str, dir: std::path::PathBuf, bytes: &[u8]) -> String {
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    let _ = h;
    p.display().to_string()
}

/// Acceptance: a FLAC track ordered as 128 stays `ready` and spawns no download.
#[tokio::test]
async fn flac_track_ordered_as_128_stays_ready() {
    let h = harness().await;
    let _ = create_order_typed(&h.app, &["FLAC128"], "library", "flac").await;
    let flac = tmp_set(&h, "FLAC128.flac", h.state.config.flac_dir(), b"fLaC");
    db::mark_ready(&h.state.pool, "FLAC128", "flac", Some(&flac), None, None)
        .await
        .unwrap();
    let t = db::get_track(&h.state.pool, "FLAC128")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, "ready");

    // Ordering the same track as 128 must not re-arm it (FLAC covers 128).
    let _ = create_order_typed(&h.app, &["FLAC128"], "toys", "mp3-128").await;
    let t = db::get_track(&h.state.pool, "FLAC128")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        t.state, "ready",
        "FLAC track stays ready when 128 is ordered"
    );
    // Demand is still recorded additively.
    let wf = t.wanted_formats();
    assert!(wf.iter().any(|f| f == "flac") && wf.iter().any(|f| f == "128"));

    let items = work::items(&h.state.pool, Some("download"), 20)
        .await
        .unwrap();
    assert!(
        !items
            .iter()
            .any(|i| i.reference == "FLAC128" && i.state == "queued"),
        "no download item for a covered format: {items:?}"
    );
}

/// Acceptance: a failed FLAC attempt on a 128-track keeps it `ready`, records
/// the error and leaves `path_128` untouched.
#[tokio::test]
async fn terminal_flac_failure_keeps_ready_128() {
    let h = harness().await;
    let _ = create_order_typed(&h.app, &["UP128"], "toys", "mp3-128").await;
    let p128 = tmp_set(&h, "UP128.mp3", h.state.config.mp3_128_dir(), b"ID3");
    db::mark_ready(&h.state.pool, "UP128", "mp3-128", None, None, Some(&p128))
        .await
        .unwrap();
    assert_eq!(
        db::get_track(&h.state.pool, "UP128")
            .await
            .unwrap()
            .unwrap()
            .state,
        "ready"
    );

    // The library wants FLAC → a real upgrade → re-armed.
    let _ = create_order_typed(&h.app, &["UP128"], "library", "flac").await;
    assert_eq!(
        db::get_track(&h.state.pool, "UP128")
            .await
            .unwrap()
            .unwrap()
            .state,
        "pending",
        "an upgrade re-arms the download"
    );

    // The FLAC attempt fails terminally.
    db::mark_terminal(
        &h.state.pool,
        "UP128",
        "failed",
        "track UP128 is gone (404)",
    )
    .await
    .unwrap();
    let t = db::get_track(&h.state.pool, "UP128")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, "ready", "a servable row is never degraded");
    assert_eq!(t.error.as_deref(), Some("track UP128 is gone (404)"));
    assert_eq!(
        t.path_128.as_deref(),
        Some(p128.as_str()),
        "path_128 unchanged"
    );
}

/// Acceptance: a second `mark_ready` at another quality never clears a path.
#[tokio::test]
async fn mark_ready_is_additive() {
    let h = harness().await;
    let _ = create_order_typed(&h.app, &["ADD1"], "library", "flac").await;
    let flac = tmp_set(&h, "ADD1.flac", h.state.config.flac_dir(), b"fLaC");
    db::mark_ready(&h.state.pool, "ADD1", "flac", Some(&flac), None, None)
        .await
        .unwrap();
    let p320 = tmp_set(&h, "ADD1.mp3", h.state.config.mp3_320_dir(), b"ID3");
    // A later, derived 320 must not wipe the FLAC path (COALESCE).
    db::mark_ready(&h.state.pool, "ADD1", "mp3-320", None, Some(&p320), None)
        .await
        .unwrap();

    let t = db::get_track(&h.state.pool, "ADD1").await.unwrap().unwrap();
    assert_eq!(
        t.path_flac.as_deref(),
        Some(flac.as_str()),
        "FLAC path kept"
    );
    assert_eq!(t.path_320.as_deref(), Some(p320.as_str()));
    assert_eq!(t.state, "ready");
    // Display tier follows the best format on disk.
    assert_eq!(t.quality_tier.as_deref(), Some("flac"));
}

/// Acceptance: `wanted_formats` is the additive union over orders.
#[tokio::test]
async fn wanted_formats_merged_additively() {
    let h = harness().await;
    let _ = create_order_typed(&h.app, &["MERGE1"], "toys", "mp3-128").await;
    let _ = create_order_typed(&h.app, &["MERGE1"], "library", "flac").await;
    let t = db::get_track(&h.state.pool, "MERGE1")
        .await
        .unwrap()
        .unwrap();
    let wf = t.wanted_formats();
    assert!(wf.iter().any(|f| f == "128"), "128 kept: {wf:?}");
    assert!(wf.iter().any(|f| f == "flac"), "flac added: {wf:?}");
    assert_eq!(
        t.wanted_quality.as_deref(),
        Some("flac"),
        "highest tier kept"
    );
}

/// Acceptance: after a terminal failure the same order does not re-arm again.
#[tokio::test]
async fn no_rearm_after_terminal_failure() {
    let h = harness().await;
    let _ = create_order_typed(&h.app, &["NOARM"], "library", "flac").await;
    db::mark_terminal(
        &h.state.pool,
        "NOARM",
        "failed",
        "track NOARM is gone (403)",
    )
    .await
    .unwrap();
    assert_eq!(
        db::get_track(&h.state.pool, "NOARM")
            .await
            .unwrap()
            .unwrap()
            .state,
        "failed"
    );

    // Re-ordering the same (already-requested) format must not re-arm it.
    let _ = create_order_typed(&h.app, &["NOARM"], "library", "flac").await;
    let t = db::get_track(&h.state.pool, "NOARM")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        t.state, "failed",
        "a terminally failed format is not re-armed"
    );
    assert_eq!(t.error.as_deref(), Some("track NOARM is gone (403)"));
}

/// Regression (Verifier F1): a download resumed after `schedule_retry` must be
/// `downloading` again. `schedule_retry` sets `next_attempt_at`; `mark_downloading`
/// has to clear it, otherwise `recompute_state` keeps the row `pending` (inflight
/// cap / lease reaper / download sync all key off `state='downloading'`).
#[tokio::test]
async fn retried_download_is_downloading() {
    let h = harness().await;
    let _ = create_order_typed(&h.app, &["RETRYDL"], "library", "flac").await;

    // A transient failure schedules a retry → `next_attempt_at` is set.
    let policy = db::RetryPolicy {
        max_attempts: 3,
        base_secs: 60,
        max_secs: 1800,
    };
    let retryable = db::schedule_retry(&h.state.pool, "RETRYDL", "boom", &policy, db::now())
        .await
        .unwrap();
    assert!(retryable, "first transient failure is retryable");
    let t = db::get_track(&h.state.pool, "RETRYDL")
        .await
        .unwrap()
        .unwrap();
    assert!(t.next_attempt_at.is_some(), "retry is scheduled");
    assert_eq!(t.state, "pending");

    // The retry is picked up: it must become `downloading`, not stay `pending`.
    db::mark_downloading(&h.state.pool, "RETRYDL", "uuid-retry", db::now() + 60)
        .await
        .unwrap();
    let t = db::get_track(&h.state.pool, "RETRYDL")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, "downloading", "resumed download is downloading");
    assert_eq!(
        t.next_attempt_at, None,
        "mark_downloading clears the scheduled retry"
    );
    assert!(t.lease_expires_at.is_some(), "lease is held");
}

/// Coverage helper sanity: a lossless/higher format covers a lower one, never
/// the other way round.
#[test]
fn coverage_is_directional() {
    assert!(models::format_covers("flac", "128"));
    assert!(models::format_covers("flac", "flac"));
    assert!(models::format_covers("320", "128"));
    assert!(!models::format_covers("128", "flac"));
    assert!(!models::format_covers("128", "320"));
    assert!(!models::format_covers("128", "ogg"));
}
