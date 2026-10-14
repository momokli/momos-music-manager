//! Integration tests for the tag "skip" flag (#345).
//!
//! A track counts as complete when every configured describing family has a
//! resolved tag (`auto`) or the user skipped it (`manual`). The skip action now
//! lives on the tag-task (`/tag-task/{id}/done`), not the track page.

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
async fn manual_skip_overrides_auto() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;

    // No families tagged at all.
    let before = tagdone::status(&app.pool, alice, track, FAMILIES).await;
    assert!(!before.complete && !before.auto, "initially incomplete");

    tagdone::set_done(&app.pool, alice, track).await.unwrap();

    let after = tagdone::status(&app.pool, alice, track, FAMILIES).await;
    assert!(after.complete, "manual skip overrides the auto rule");
    assert!(!after.auto, "auto stays false");
    assert!(after.manual, "manual flag set");
}
