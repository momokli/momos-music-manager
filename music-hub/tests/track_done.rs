//! Integration tests for the track "tagged-complete" flag (#345, T5-1 / F1).
//!
//! A track counts as complete when every configured describing family has a
//! resolved tag (`auto`) or the user manually marked it done (`manual`).

mod common;

use music_hub::tagdone;
use music_hub::tagtypes::{TagType, set as set_type};

const FAMILIES: &[&str] = &["MOOD", "GENRE", "VIBE", "PHASE", "MERKMAL"];

/// Tag `track` with a single typed tag through the public HTTP endpoint, so the
/// whole pipeline (ensure_tag -> set type -> tag_track -> resolved tags) runs.
async fn tag_via_http(app: &common::TestApp, cookie: &str, track: i64, name: &str, ty: &str) {
    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/tag")))
        .header("Cookie", cookie)
        .form(&[("name", name), ("type", ty)])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn done_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url(&format!("/track/{}/done", app.seed.t_all)))
        .form(&[("scope", "all")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn manual_done_persists_and_shows() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/done")))
        .header("Cookie", &cookie)
        .form(&[("scope", "all")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()["location"],
        format!("/track/{track}?scope=all")
    );

    // DB row exists.
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_tag_done WHERE user_id = ?1 AND track_id = ?2",
    )
    .bind(alice)
    .bind(track)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "manual done row missing");

    // The track view shows the manual "fertig" badge.
    let html = app
        .client()
        .get(app.url(&format!("/track/{track}")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("fertig"), "manual 'fertig' badge missing");
}

#[tokio::test]
async fn undone_clears() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    app.client()
        .post(app.url(&format!("/track/{track}/done")))
        .header("Cookie", &cookie)
        .form(&[("scope", "all")])
        .send()
        .await
        .unwrap();

    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/undone")))
        .header("Cookie", &cookie)
        .form(&[("scope", "all")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()["location"],
        format!("/track/{track}?scope=all")
    );

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_tag_done WHERE user_id = ?1 AND track_id = ?2",
    )
    .bind(alice)
    .bind(track)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "manual done row not cleared");

    let html = app
        .client()
        .get(app.url(&format!("/track/{track}")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("offen"), "badge should be 'offen' after undone");
}

#[tokio::test]
async fn auto_complete_requires_all_families() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    // Four of five families -> still incomplete, PHASE missing.
    for (name, ty) in [
        ("Dunkel", "MOOD"),
        ("House", "GENRE"),
        ("Warm", "VIBE"),
        ("Instrumental", "MERKMAL"),
    ] {
        tag_via_http(&app, &cookie, track, name, ty).await;
    }
    let st = tagdone::status(&app.pool, alice, track, FAMILIES).await;
    assert!(!st.complete, "four families must not be complete");
    assert!(!st.auto, "auto must be false with a missing family");
    assert!(st.missing.contains(&"PHASE".to_string()), "PHASE missing");

    // Tag the fifth family -> auto complete, no manual mark.
    tag_via_http(&app, &cookie, track, "Peak", "PHASE_1").await;
    let st = tagdone::status(&app.pool, alice, track, FAMILIES).await;
    assert!(st.auto, "all five families must be auto-complete");
    assert!(st.complete, "complete follows auto");
    assert!(st.missing.is_empty(), "no family may remain missing");
    assert!(!st.manual, "no manual mark was set");
}

#[tokio::test]
async fn phase_variants_count_as_one_family() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;

    // A single PHASE_1 tag satisfies the whole PHASE family.
    let tag_id = music_hub::tags::ensure_tag(&app.pool, alice, "Peak")
        .await
        .unwrap();
    set_type(&app.pool, alice, tag_id, Some(TagType::Phase1))
        .await
        .unwrap();
    music_hub::tags::tag_track(&app.pool, alice, track, tag_id)
        .await
        .unwrap();

    let st = tagdone::status(&app.pool, alice, track, FAMILIES).await;
    assert!(
        !st.missing.contains(&"PHASE".to_string()),
        "PHASE_1 must satisfy the PHASE family: {:?}",
        st.missing
    );
    // The other families are still missing, so not complete.
    assert!(!st.auto, "other families are still missing");
}

#[tokio::test]
async fn manual_overrides_auto() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    // No families tagged at all.
    let before = tagdone::status(&app.pool, alice, track, FAMILIES).await;
    assert!(!before.complete && !before.auto, "initially incomplete");

    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/done")))
        .header("Cookie", &cookie)
        .form(&[("scope", "all")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let after = tagdone::status(&app.pool, alice, track, FAMILIES).await;
    assert!(after.complete, "manual mark overrides the auto rule");
    assert!(!after.auto, "auto stays false");
    assert!(after.manual, "manual flag set");
}
