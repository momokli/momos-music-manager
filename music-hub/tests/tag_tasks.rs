//! Integration tests for the daily/weekly tagging task (#346).
//!
//! The task surfaces **uncomplete** tracks for a user, **most-played first**,
//! persisted per `(user, cadence, period_start)` so it is stable within a
//! period.

mod common;

use chrono::{TimeZone, Utc};
use music_hub::tagdone;
use music_hub::tagtasks::{self, Cadence};

/// Tag a track with one typed tag through the public `tags` API, so the whole
/// pipeline (ensure_tag -> set type -> tag_track -> resolved tags) runs.
async fn add_tag(app: &common::TestApp, user: i64, track: i64, name: &str, ty: &str) {
    let tag_id = music_hub::tags::ensure_tag(&app.pool, user, name)
        .await
        .unwrap();
    let t = music_hub::tagtypes::TagType::parse(ty).unwrap();
    music_hub::tagtypes::set(&app.pool, user, tag_id, Some(t))
        .await
        .unwrap();
    music_hub::tags::tag_track(&app.pool, &app.cfg, user, track, tag_id)
        .await
        .unwrap();
}

async fn set_plays(app: &common::TestApp, user: i64, track: i64, plays: i64) {
    sqlx::query(
        "INSERT INTO hub_traktor_tracks (user_id, track_id, play_count, imported_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id, track_id) DO UPDATE SET play_count = excluded.play_count",
    )
    .bind(user)
    .bind(track)
    .bind(plays)
    .bind("2026-01-01T00:00:00+00:00")
    .execute(&app.pool)
    .await
    .unwrap();
}

async fn add_history(app: &common::TestApp, user: i64, track: i64, n: usize) {
    for _ in 0..n {
        sqlx::query(
            "INSERT INTO hub_listening_history (user_id, track_id, played_at, source)
             VALUES (?1, ?2, ?3, 'spotify')",
        )
        .bind(user)
        .bind(track)
        .bind("2026-01-01T00:00:00+00:00")
        .execute(&app.pool)
        .await
        .unwrap();
    }
}

async fn get_task_json(app: &common::TestApp, cookie: &str, cadence: &str) -> serde_json::Value {
    let resp = app
        .client()
        .get(app.url(&format!("/api/hub/tag-task?cadence={cadence}")))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    resp.json().await.unwrap()
}

fn track_ids(json: &serde_json::Value) -> Vec<i64> {
    json["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["trackId"].as_i64().unwrap())
        .collect()
}

#[tokio::test]
async fn api_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/api/hub/tag-task?cadence=daily"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "login required");
}

#[tokio::test]
async fn api_orders_most_played_first() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    // Traktor plays + listening-history rows both count toward "plays".
    set_plays(&app, alice, app.seed.t_two, 10).await;
    set_plays(&app, alice, app.seed.t_all, 3).await;
    add_history(&app, alice, app.seed.t_alice, 2).await;

    let json = get_task_json(&app, &cookie, "daily").await;
    let ids = track_ids(&json);
    assert_eq!(ids[0], app.seed.t_two, "most-played track first");
    assert_eq!(ids[1], app.seed.t_all, "second by plays");
    assert_eq!(ids[2], app.seed.t_alice, "history rows count as plays");

    let items = json["data"]["items"].as_array().unwrap();
    assert_eq!(items[0]["plays"].as_i64(), Some(10));
    assert_eq!(items[1]["plays"].as_i64(), Some(3));
    assert_eq!(items[2]["plays"].as_i64(), Some(2));
    assert_eq!(items[0]["rank"].as_i64(), Some(1));
}

#[tokio::test]
async fn api_excludes_auto_complete_tracks() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    // Fully tag t_all (all five describing families) -> auto-complete.
    for (name, ty) in [
        ("Dunkel", "MOOD"),
        ("House", "GENRE"),
        ("Warm", "VIBE"),
        ("Peak", "PHASE_1"),
        ("Instrumental", "MERKMAL"),
    ] {
        add_tag(&app, alice, app.seed.t_all, name, ty).await;
    }
    let st = tagdone::status(
        &app.pool,
        alice,
        app.seed.t_all,
        &["MOOD", "GENRE", "VIBE", "PHASE", "MERKMAL"],
    )
    .await;
    assert!(st.auto, "fixture must be auto-complete");

    let json = get_task_json(&app, &cookie, "daily").await;
    assert!(
        !track_ids(&json).contains(&app.seed.t_all),
        "auto-complete track must not appear in the task"
    );
}

#[tokio::test]
async fn api_excludes_manually_done_tracks() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    tagdone::set_done(&app.pool, alice, app.seed.t_two)
        .await
        .unwrap();

    let json = get_task_json(&app, &cookie, "daily").await;
    assert!(
        !track_ids(&json).contains(&app.seed.t_two),
        "manually-done track must not appear in the task"
    );
}

#[tokio::test]
async fn api_reports_missing_families_and_metadata() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    add_tag(&app, alice, app.seed.t_all, "Dunkel", "MOOD").await;
    add_tag(&app, alice, app.seed.t_all, "House", "GENRE").await;

    let json = get_task_json(&app, &cookie, "daily").await;
    let item = json["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["trackId"].as_i64() == Some(app.seed.t_all))
        .expect("t_all must be listed");

    let missing: Vec<String> = item["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        missing,
        vec!["VIBE", "PHASE", "MERKMAL"],
        "missing families in canonical order"
    );
    assert_eq!(item["title"].as_str(), Some("Shared Anthem"));
    assert_eq!(item["artists"].as_str(), Some("Aaa"));
    assert_eq!(json["data"]["cadence"].as_str(), Some("daily"));
}

#[tokio::test]
async fn api_rejects_invalid_cadence() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url("/api/hub/tag-task?cadence=hourly"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_defaults_cadence_and_weekly_roundtrip() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    // No cadence param -> setting default (`daily`).
    let resp = app
        .client()
        .get(app.url("/api/hub/tag-task"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["data"]["cadence"].as_str(), Some("daily"));

    let weekly = get_task_json(&app, &cookie, "weekly").await;
    assert_eq!(weekly["data"]["cadence"].as_str(), Some("weekly"));
    // A weekly task with no play data still lists all uncomplete tracks.
    assert!(!track_ids(&weekly).is_empty());
}

#[tokio::test]
async fn task_for_is_stable_within_period_and_rolls_at_boundary() {
    let app = common::spawn().await;
    let alice = app.seed.alice;

    let now = Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0).unwrap();
    let first = tagtasks::task_for(&app.pool, alice, Cadence::Daily, now, 50)
        .await
        .unwrap();
    let again = tagtasks::task_for(&app.pool, alice, Cadence::Daily, now, 50)
        .await
        .unwrap();
    assert_eq!(first.id, again.id, "same period must reuse the task id");
    assert_eq!(first.items, again.items, "same period must keep the items");

    // A different moment later the same day is still the same period.
    let same_day = Utc.with_ymd_and_hms(2026, 5, 12, 23, 59, 0).unwrap();
    let still = tagtasks::task_for(&app.pool, alice, Cadence::Daily, same_day, 50)
        .await
        .unwrap();
    assert_eq!(first.id, still.id);

    // Crossing the day boundary generates a fresh task.
    let next_day = Utc.with_ymd_and_hms(2026, 5, 13, 0, 0, 0).unwrap();
    let rolled = tagtasks::task_for(&app.pool, alice, Cadence::Daily, next_day, 50)
        .await
        .unwrap();
    assert_ne!(first.id, rolled.id, "new period -> new task id");
    assert_eq!(rolled.period_start, "2026-05-13");
}

#[tokio::test]
async fn candidates_respect_limit_and_order() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let families: Vec<String> = ["MOOD", "GENRE", "VIBE", "PHASE", "MERKMAL"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    set_plays(&app, alice, app.seed.t_two, 5).await;
    set_plays(&app, alice, app.seed.t_all, 4).await;
    set_plays(&app, alice, app.seed.t_pl, 3).await;

    let all = tagtasks::candidates(&app.pool, alice, &families, 50)
        .await
        .unwrap();
    let ranks: Vec<i64> = all.iter().map(|i| i.rank).collect();
    assert_eq!(ranks, (1..=all.len() as i64).collect::<Vec<_>>());
    assert_eq!(all[0].track_id, app.seed.t_two);
    assert_eq!(all[0].plays, 5);

    let limited = tagtasks::candidates(&app.pool, alice, &families, 2)
        .await
        .unwrap();
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[0].track_id, app.seed.t_two);
    assert_eq!(limited[1].track_id, app.seed.t_all);
}
