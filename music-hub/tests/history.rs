//! Integration tests for the import-history module (plan H1–H3, issues
//! #224–#226): the run ledger, the change-event helpers and the `/history` UI.

mod common;

use music_hub::history::{diff_membership, diff_scalar, finish_run, record_event, start_run};
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
        record_event(&app.pool, run, alice, "traktor", "track", "x", "bogus", None, None)
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

// ── #351: import-history polish — filters, diffs, change events ──────────────

/// Fetch `/history` (or a sub-path) as `alice`.
async fn fetch(app: &common::TestApp, alice: i64, path: &str) -> String {
    let cookie = app.session_cookie(alice).await;
    app.client()
        .get(app.url(path))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

#[tokio::test]
async fn history_change_filter_scopes_events() {
    let app = common::spawn().await;
    let (alice, _) = seed_history(&app).await;

    // added → only ALPHA-ref.
    let html = fetch(&app, alice, "/history?change=added").await;
    assert!(html.contains("ALPHA-ref"), "added event missing");
    assert!(
        !html.contains("BETA-ref"),
        "changed leaked into change=added"
    );
    assert!(
        !html.contains("GAMMA-ref"),
        "removed leaked into change=added"
    );

    // removed → only GAMMA-ref.
    let html = fetch(&app, alice, "/history?change=removed").await;
    assert!(html.contains("GAMMA-ref"), "removed event missing");
    assert!(
        !html.contains("ALPHA-ref"),
        "added leaked into change=removed"
    );
    assert!(
        !html.contains("BETA-ref"),
        "changed leaked into change=removed"
    );

    // An unknown value is ignored → all events visible again.
    let html = fetch(&app, alice, "/history?change=bogus").await;
    assert!(
        html.contains("ALPHA-ref") && html.contains("BETA-ref") && html.contains("GAMMA-ref"),
        "unknown change= must be ignored (all events visible)"
    );
}

#[tokio::test]
async fn history_run_filter_scopes_events() {
    let app = common::spawn().await;
    let (a, _) = seed_history(&app).await;

    // run a → only its own event.
    let html = fetch(&app, alice_of(&app), &format!("/history?run={a}")).await;
    assert!(html.contains("ALPHA-ref"), "run a event missing");
    assert!(!html.contains("BETA-ref"), "run b event leaked into run=a");
    assert!(!html.contains("GAMMA-ref"), "run c event leaked into run=a");
    // Active filter chip is shown.
    assert!(
        html.contains(&format!("Run #{a}")),
        "run filter chip missing"
    );

    // Empty run= is tolerated (no error, all events visible).
    let html = fetch(&app, alice_of(&app), "/history?run=").await;
    assert!(html.contains("ALPHA-ref"), "empty run= must not error");

    // Unknown run id → filter-aware empty state.
    let html = fetch(&app, alice_of(&app), "/history?run=999999").await;
    assert!(
        html.contains("Keine Änderungen für diesen Filter"),
        "filter-aware empty state missing for unknown run"
    );
}

#[tokio::test]
async fn history_run_link_and_chip_present() {
    let app = common::spawn().await;
    let (a, _) = seed_history(&app).await;
    let alice = alice_of(&app);

    // The run id in the runs table links to ?run=<id>.
    let html = fetch(&app, alice, "/history").await;
    assert!(
        html.contains(&format!("run={a}")),
        "run id link missing in runs table"
    );

    // The chip link keeps other params (source) and drops run.
    let html = fetch(&app, alice, &format!("/history?run={a}&source=srcalpha")).await;
    assert!(html.contains(&format!("Run #{a}")), "chip label missing");
    assert!(html.contains("hub-chip-on"), "active run chip missing");
    assert!(
        html.contains("href=\"/history?source=srcalpha\""),
        "chip link must drop run and preserve source"
    );
}

#[tokio::test]
async fn history_change_counts_badges() {
    let app = common::spawn().await;
    let (alice, _) = seed_history(&app).await;
    let html = fetch(&app, alice, "/history").await;

    // Run a: added=1; run b: changed=1; run c: removed=1.
    assert!(html.contains("+1"), "added count badge missing (+1)");
    assert!(html.contains("~1"), "changed count badge missing (~1)");
    assert!(html.contains("−1"), "removed count badge missing (−1)");
    // The badge classes carry the change kind.
    assert!(html.contains("hub-badge-ok"), "added badge class missing");
    assert!(html.contains("hub-badge-no"), "removed badge class missing");
    assert!(
        html.contains("hub-badge-warn"),
        "changed badge class missing (hub-badge-warn)"
    );
}

#[tokio::test]
async fn history_stats_rendered_as_chips() {
    let app = common::spawn().await;
    let (alice, _) = seed_history(&app).await;
    let html = fetch(&app, alice, "/history").await;

    // Run a's stats {"marker":"STATALPHA"} → key and value rendered as a chip.
    assert!(html.contains("marker"), "stats key missing");
    assert!(html.contains("STATALPHA"), "stats value missing");
    assert!(
        html.contains("marker: STATALPHA"),
        "stats must render as key: value chip"
    );
}

#[tokio::test]
async fn history_diff_rendering() {
    let app = common::spawn().await;
    let (alice, _) = seed_history(&app).await;
    let html = fetch(&app, alice, "/history").await;

    assert!(html.contains("Änderung (Diff)"), "diff column missing");
    // added → only the after value, green.
    assert!(
        html.contains("hub-diff-add\">7<"),
        "added after value not marked hub-diff-add"
    );
    // removed → only the before value, red/struck.
    assert!(
        html.contains("hub-diff-rem\">5<"),
        "removed before value not marked hub-diff-rem"
    );
    // changed → before, arrow, after.
    assert!(
        html.contains("hub-diff-rem\">3<") && html.contains("hub-diff-add\">8<"),
        "changed values missing"
    );
    assert!(html.contains("hub-diff-arrow"), "changed arrow missing");
}

#[tokio::test]
async fn history_filtered_empty_state() {
    let app = common::spawn().await;
    let (_, c) = seed_history(&app).await;
    let alice = alice_of(&app);

    // Run c only has a removed event; change=added yields a filter-aware message.
    let html = fetch(&app, alice, &format!("/history?change=added&run={c}")).await;
    assert!(
        html.contains("Keine Änderungen für diesen Filter"),
        "filter-aware empty state missing"
    );
    assert!(
        !html.contains("Keine Änderungen."),
        "generic empty state must not appear with an active filter"
    );
}

#[tokio::test]
async fn history_empty_state_for_freetext_and_source_filters() {
    let app = common::spawn().await;
    let (_, _) = seed_history(&app).await;
    let alice = alice_of(&app);

    // A free-text filter matching nothing → filter-aware empty state, not the
    // generic one, and the reset link clears every filter back to /history.
    let html = fetch(&app, alice, "/history?q=NOMATCHZZZ").await;
    assert!(
        html.contains("Keine Änderungen für diesen Filter"),
        "filter-aware empty state missing for q filter"
    );
    assert!(
        !html.contains("Keine Änderungen."),
        "generic empty state must not appear with an active q filter"
    );
    assert!(
        html.contains("href=\"/history\""),
        "reset link for q filter must clear back to /history"
    );

    // A source filter matching nothing → filter-aware empty state as well.
    let html = fetch(&app, alice, "/history?source=nosuchsource").await;
    assert!(
        html.contains("Keine Änderungen für diesen Filter"),
        "filter-aware empty state missing for source filter"
    );
    assert!(
        html.contains("href=\"/history\""),
        "reset link for source filter must clear back to /history"
    );

    // Reset from a combined filter drops q/source/user/run/change together.
    let html = fetch(&app, alice, "/history?source=srcalpha&q=NOMATCHZZZ").await;
    assert!(
        html.contains("href=\"/history\""),
        "reset link must drop all filter params"
    );
}

/// The seeded alice user id.
fn alice_of(app: &common::TestApp) -> i64 {
    app.seed.alice
}
