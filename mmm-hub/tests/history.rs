//! Integration tests for the import-history module (plan H1–H3, issues
//! #224–#226): the run ledger, the change-event helpers and the `/history` UI.

mod common;

use mmm_hub::history::{diff_membership, diff_scalar, finish_run, record_event, start_run};
use reqwest::StatusCode;

#[tokio::test]
async fn history_requires_login() {
    let app = common::spawn().await;
    let resp = app.client().get(app.url("/history")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

// ── H1: run ledger ───────────────────────────────────────────────────────────

#[tokio::test]
async fn start_and_finish_run_records_ledger() {
    let app = common::spawn().await;
    let alice = app.seed.alice;

    let run = start_run(&app.pool, alice, "traktor").await.unwrap();

    let (status, finished, stats): (String, Option<String>, String) =
        sqlx::query_as("SELECT status, finished_at, stats FROM hub_import_runs WHERE id = ?1")
            .bind(run)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(status, "running");
    assert!(finished.is_none(), "a running run has no finished_at");
    assert_eq!(stats, "{}");

    let payload = serde_json::json!({ "tracks": 12, "playlists": 3 });
    finish_run(&app.pool, run, "ok", &payload).await.unwrap();

    let (status, finished, stats): (String, Option<String>, String) =
        sqlx::query_as("SELECT status, finished_at, stats FROM hub_import_runs WHERE id = ?1")
            .bind(run)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(status, "ok");
    assert!(finished.is_some(), "finish_run must stamp finished_at");
    let parsed: serde_json::Value = serde_json::from_str(&stats).unwrap();
    assert_eq!(parsed["tracks"], 12);
    assert_eq!(parsed["playlists"], 3);

    // The ledger is scoped to the user + source.
    let (uid, src): (i64, String) =
        sqlx::query_as("SELECT user_id, source FROM hub_import_runs WHERE id = ?1")
            .bind(run)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(uid, alice);
    assert_eq!(src, "traktor");
}

// ── H2: diff helpers ─────────────────────────────────────────────────────────

#[tokio::test]
async fn diff_membership_records_exact_added_and_removed() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let run = start_run(&app.pool, alice, "spotify").await.unwrap();

    // old {1,2,3}, new {3,4} → added {4}, removed {1,2}.
    diff_membership(
        &app.pool,
        run,
        alice,
        "spotify",
        "playlist_track",
        &[3, 1, 2],
        &[4, 3],
    )
    .await
    .unwrap();

    let added: Vec<String> = sqlx::query_scalar(
        "SELECT entity_ref FROM hub_import_events
          WHERE run_id = ?1 AND change = 'added' ORDER BY entity_ref",
    )
    .bind(run)
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(added, vec!["4"]);

    let removed: Vec<String> = sqlx::query_scalar(
        "SELECT entity_ref FROM hub_import_events
          WHERE run_id = ?1 AND change = 'removed' ORDER BY entity_ref",
    )
    .bind(run)
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(removed, vec!["1", "2"]);

    // Before/after carry the id for the value change.
    let (before, after): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT before, after FROM hub_import_events
          WHERE run_id = ?1 AND change = 'added'",
    )
    .bind(run)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(before, None);
    assert_eq!(after.as_deref(), Some("4"));

    // A no-op diff writes nothing.
    diff_membership(
        &app.pool,
        run,
        alice,
        "spotify",
        "playlist_track",
        &[1, 2, 3],
        &[1, 2, 3],
    )
    .await
    .unwrap();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_import_events WHERE run_id = ?1")
        .bind(run)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(total, 3, "idempotent diff must not add events");
}

#[tokio::test]
async fn diff_scalar_records_change_only_when_different() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let run = start_run(&app.pool, alice, "traktor").await.unwrap();

    // Changed playcount 5 → 9 records a 'changed' event.
    diff_scalar(
        &app.pool,
        run,
        alice,
        "traktor",
        "traktor_playcount",
        "track-42",
        Some("5"),
        Some("9"),
    )
    .await
    .unwrap();

    // Unchanged value writes nothing.
    diff_scalar(
        &app.pool,
        run,
        alice,
        "traktor",
        "traktor_playcount",
        "track-42",
        Some("9"),
        Some("9"),
    )
    .await
    .unwrap();
    // New rating from nothing → value records a change too.
    diff_scalar(
        &app.pool,
        run,
        alice,
        "traktor",
        "traktor_rating",
        "track-42",
        None,
        Some("4"),
    )
    .await
    .unwrap();

    let (change, before, after): (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT change, before, after FROM hub_import_events
          WHERE run_id = ?1 AND entity_type = 'traktor_playcount'",
    )
    .bind(run)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(change, "changed");
    assert_eq!(before.as_deref(), Some("5"));
    assert_eq!(after.as_deref(), Some("9"));

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_import_events WHERE run_id = ?1")
        .bind(run)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(total, 2, "only real changes are recorded");

    // record_event rejects an invalid change kind (DB CHECK).
    assert!(
        record_event(
            &app.pool, run, alice, "traktor", "track", "x", "bogus", None, None
        )
        .await
        .is_err()
    );
}

// ── H3: history page ─────────────────────────────────────────────────────────

async fn seed_history(app: &common::TestApp) -> (i64, i64) {
    let alice = app.seed.alice;
    let bob = app.seed.bob;

    let a = start_run(&app.pool, alice, "srcalpha").await.unwrap();
    record_event(
        &app.pool,
        a,
        alice,
        "srcalpha",
        "playlist_track",
        "ALPHA-ref",
        "added",
        None,
        Some("7"),
    )
    .await
    .unwrap();
    finish_run(
        &app.pool,
        a,
        "ok",
        &serde_json::json!({ "marker": "STATALPHA" }),
    )
    .await
    .unwrap();

    let b = start_run(&app.pool, alice, "srcbeta").await.unwrap();
    record_event(
        &app.pool,
        b,
        alice,
        "srcbeta",
        "traktor_playcount",
        "BETA-ref",
        "changed",
        Some("3"),
        Some("8"),
    )
    .await
    .unwrap();
    finish_run(
        &app.pool,
        b,
        "error",
        &serde_json::json!({ "marker": "STATBETA" }),
    )
    .await
    .unwrap();

    let c = start_run(&app.pool, bob, "srcalpha").await.unwrap();
    record_event(
        &app.pool,
        c,
        bob,
        "srcalpha",
        "playlist_track",
        "GAMMA-ref",
        "removed",
        Some("5"),
        None,
    )
    .await
    .unwrap();

    (a, c)
}

#[tokio::test]
async fn history_page_shows_runs_and_events() {
    let app = common::spawn().await;
    let (alice, _) = seed_history(&app).await;
    let cookie = app.session_cookie(alice).await;

    let resp = app
        .client()
        .get(app.url("/history"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = resp.text().await.unwrap();

    // Both runs and the change timeline.
    assert!(html.contains("Import-Historie"), "heading missing");
    assert!(
        html.contains("srcalpha") && html.contains("srcbeta"),
        "sources missing"
    );
    assert!(
        html.contains("ok") && html.contains("error"),
        "run status missing"
    );
    assert!(html.contains("STATALPHA"), "run stats missing");
    assert!(
        html.contains("ALPHA-ref") && html.contains("BETA-ref") && html.contains("GAMMA-ref"),
        "change timeline missing rows"
    );
    assert!(html.contains("playlist_track"), "entity type missing");
    // Bounded scroll regions (neat & funky, no full-page vertical scroll).
    assert!(
        html.matches("hub-scroll-y").count() >= 2,
        "both panels must be bounded scroll regions"
    );
}

#[tokio::test]
async fn history_page_filters_by_source_user_and_q() {
    let app = common::spawn().await;
    let (alice, _) = seed_history(&app).await;
    let cookie = app.session_cookie(alice).await;

    // source filter
    let html = app
        .client()
        .get(app.url("/history?source=srcalpha"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("ALPHA-ref") && html.contains("GAMMA-ref"));
    assert!(
        !html.contains("BETA-ref"),
        "srcbeta event leaked into source=srcalpha"
    );
    assert!(
        !html.contains("STATBETA"),
        "srcbeta run leaked into source=srcalpha"
    );

    // user filter (alice only): bob's gamma disappears.
    let html = app
        .client()
        .get(app.url("/history?user=alice"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("ALPHA-ref") && html.contains("BETA-ref"));
    assert!(
        !html.contains("GAMMA-ref"),
        "bob's event leaked into user=alice"
    );

    // free-text q.
    let html = app
        .client()
        .get(app.url("/history?q=GAMMA"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("GAMMA-ref"));
    assert!(
        !html.contains("ALPHA-ref"),
        "non-matching event leaked into q=GAMMA"
    );
}
