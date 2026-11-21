//! Integration tests for the comment-template preview (issue #670, slice C4;
//! source switched in #678, slice C6).
//!
//! Covers the `/kellner/{id}/template/preview` and
//! `/comment-templates/{id}/preview` htmx fragments against the `common`
//! harness: session gating, `404` scoping, the **top tagged tracks** source
//! (rank `tag_count DESC, last_tagged_at DESC NULLS last, id ASC`,
//! `PREVIEW_LIMIT = 3`), the `3 von N` + `n/512` counters, the scope of the
//! caller's own tags, backpack-independence and the empty hint. No music-api is
//! configured in the harness, so a `200` also proves the preview never reaches
//! out (acceptance: no network call / no `MUSIC_API_TOKEN`).

mod common;

use music_hub::devices::hash_token;
use reqwest::StatusCode;

const TS: &str = "2026-01-01T00:00:00+00:00";

async fn bound_device(app: &common::TestApp, user_id: i64, name: &str, raw_token: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_devices (user_id, name, token_hash, created_at, paired_at)
         VALUES (?1, ?2, ?3, ?4, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(hash_token(raw_token))
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .expect("insert bound device")
}

async fn track(app: &common::TestApp, sid: &str, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, isrc, title, artists, first_seen_at)
         VALUES ('spotify', ?1, ?2, ?3, 'X', ?4) RETURNING id",
    )
    .bind(sid)
    .bind(format!("DEAAA{sid}"))
    .bind(title)
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .expect("insert track")
}

/// Set a track's `last_tagged_at` (rank tiebreak input; `None` = NULL).
async fn set_last_tagged(app: &common::TestApp, track_id: i64, at: Option<&str>) {
    sqlx::query("UPDATE hub_tracks SET last_tagged_at = ?2 WHERE id = ?1")
        .bind(track_id)
        .bind(at)
        .execute(&app.pool)
        .await
        .expect("set last_tagged_at");
}

async fn tag(app: &common::TestApp, user_id: i64, slug: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, ?2, ?3, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(slug)
    .bind(name)
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .expect("insert tag")
}

async fn resolve_tag(app: &common::TestApp, track_id: i64, tag_id: i64) {
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track_id)
        .bind(tag_id)
        .execute(&app.pool)
        .await
        .expect("resolved tag");
}

/// Tag `track_id` with `count` fresh, distinct tags owned by `user_id`.
async fn tag_with(app: &common::TestApp, user_id: i64, track_id: i64, prefix: &str, count: usize) {
    for i in 0..count {
        let tg = tag(app, user_id, &format!("{prefix}-{i}"), &format!("Tag {prefix} {i}")).await;
        resolve_tag(app, track_id, tg).await;
    }
}

async fn audio(app: &common::TestApp, track_id: i64, bpm: Option<f64>, key: Option<&str>) {
    sqlx::query("INSERT INTO hub_track_audio (track_id, bpm, musical_key) VALUES (?1, ?2, ?3)")
        .bind(track_id)
        .bind(bpm)
        .bind(key)
        .execute(&app.pool)
        .await
        .expect("audio");
}

async fn template(
    app: &common::TestApp,
    owner: Option<i64>,
    name: &str,
    body: &str,
    shared: bool,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_comment_templates
            (owner_user_id, name, body, is_shared, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(body)
    .bind(i64::from(shared))
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .expect("insert template")
}

async fn post_form(
    app: &common::TestApp,
    cookie: Option<&str>,
    path: &str,
    body: &str,
) -> (StatusCode, String) {
    let mut req = app
        .client()
        .post(app.url(path))
        .header("Content-Type", "application/x-www-form-urlencoded");
    if let Some(c) = cookie {
        req = req.header("Cookie", c);
    }
    let resp = req.body(body.to_string()).send().await.expect("post");
    let status = resp.status();
    (status, resp.text().await.expect("body"))
}

// ── session + scoping ────────────────────────────────────────────────────────

#[tokio::test]
async fn preview_requires_session() {
    let app = common::spawn().await;
    let (uid, _c) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let (status, _) = post_form(&app, None, &format!("/kellner/{did}/template/preview"), "").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "no session → redirect");

    let (status, _) = post_form(&app, None, "/comment-templates/1/preview", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn foreign_and_unknown_device_are_404() {
    let app = common::spawn().await;
    let (alice, alice_cookie) = app.user_with_password("cp-alice", "1234").await;
    let (bob, _bob) = app.user_with_password("cp-bob", "1234").await;
    let alice_dev = bound_device(&app, alice, "alice-k", &"a".repeat(64)).await;
    let bob_dev = bound_device(&app, bob, "bob-k", &"c".repeat(64)).await;

    // Bob's device is invisible to Alice.
    let (status, _) = post_form(
        &app,
        Some(&alice_cookie),
        &format!("/kellner/{bob_dev}/template/preview"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Unknown id → same 404.
    let (status, _) = post_form(
        &app,
        Some(&alice_cookie),
        "/kellner/999999/template/preview",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Own device → 200 (control).
    let (status, _) = post_form(
        &app,
        Some(&alice_cookie),
        &format!("/kellner/{alice_dev}/template/preview"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn foreign_private_template_is_404_shared_is_200() {
    let app = common::spawn().await;
    let (_alice, alice_cookie) = app.user_with_password("cp-alice", "1234").await;
    let (bob, _bob) = app.user_with_password("cp-bob", "1234").await;

    let private = template(&app, Some(bob), "bob-priv", "{tags}", false).await;
    let shared = template(&app, Some(bob), "bob-shared", "{tags}", true).await;
    let system = template(&app, None, "sys", "{tags}", false).await;

    let (status, _) = post_form(
        &app,
        Some(&alice_cookie),
        &format!("/comment-templates/{private}/preview"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "foreign private → 404");

    let (status, _) = post_form(
        &app,
        Some(&alice_cookie),
        &format!("/comment-templates/{shared}/preview"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "shared read-only → 200");

    let (status, html) = post_form(
        &app,
        Some(&alice_cookie),
        &format!("/comment-templates/{system}/preview"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "system → 200");
    assert!(html.contains("Keine getaggten Tracks"), "no tagged tracks hint");
}

// ── source: top tagged tracks, rank, limit, counter ──────────────────────────

#[tokio::test]
async fn device_preview_shows_three_of_n_in_rank_order() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    // Five tagged tracks with strictly decreasing tag counts (ids ascending):
    // rank must be Alpha (5), Bravo (4), Charlie (3), Delta (2), Echo (1).
    let alpha = track(&app, "r1", "Alpha").await;
    let bravo = track(&app, "r2", "Bravo").await;
    let charlie = track(&app, "r3", "Charlie").await;
    let delta = track(&app, "r4", "Delta").await;
    let echo = track(&app, "r5", "Echo").await;
    tag_with(&app, uid, alpha, "a", 5).await;
    tag_with(&app, uid, bravo, "b", 4).await;
    tag_with(&app, uid, charlie, "c", 3).await;
    tag_with(&app, uid, delta, "d", 2).await;
    tag_with(&app, uid, echo, "e", 1).await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/kellner/{did}/template/preview"),
        "comment_template_id=0&comment_template={tags} {bpm}",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("3 von 5"), "counter: {html}");
    assert!(html.contains("/512"), "char counter shown");

    let a = html.find("Alpha").expect("Alpha present");
    let b = html.find("Bravo").expect("Bravo present");
    let c = html.find("Charlie").expect("Charlie present");
    assert!(a < b && b < c, "rank order tag_count DESC: {html}");
    assert!(!html.contains("Delta"), "only the top three rendered");
    assert!(!html.contains("Echo"), "only the top three rendered");
}

#[tokio::test]
async fn rank_tiebreak_last_tagged_at_then_id() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    // Four tracks, one tag each (equal tag_count). Order by last_tagged_at DESC
    // (NULLs last), then id ASC: Bravo (newest), Alpha, then the NULL pair by id.
    let alpha = track(&app, "t1", "Alpha").await;
    let bravo = track(&app, "t2", "Bravo").await;
    let charlie = track(&app, "t3", "Charlie").await;
    let delta = track(&app, "t4", "Delta").await;
    tag_with(&app, uid, alpha, "a", 1).await;
    tag_with(&app, uid, bravo, "b", 1).await;
    tag_with(&app, uid, charlie, "c", 1).await;
    tag_with(&app, uid, delta, "d", 1).await;
    set_last_tagged(&app, alpha, Some("2026-02-01T00:00:00+00:00")).await;
    set_last_tagged(&app, bravo, Some("2026-03-01T00:00:00+00:00")).await;
    // charlie, delta both NULL → id ascending puts charlie (lower id) first.

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/kellner/{did}/template/preview"),
        "comment_template_id=0&comment_template={tags}",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("3 von 4"), "counter: {html}");

    let b = html.find("Bravo").expect("Bravo present");
    let a = html.find("Alpha").expect("Alpha present");
    let c = html.find("Charlie").expect("Charlie present");
    assert!(b < a && a < c, "tiebreak order newest-then-id: {html}");
    assert!(!html.contains("Delta"), "id tiebreak kept Delta out");
}

#[tokio::test]
async fn scope_ignores_foreign_tag() {
    let app = common::spawn().await;
    let (alice, cookie) = app.user_with_password("cp-alice", "1234").await;
    let (bob, _bob) = app.user_with_password("cp-bob", "1234").await;
    let did = bound_device(&app, alice, "alice-k", &"a".repeat(64)).await;

    // A track carrying only Bob's tag must stay invisible to Alice.
    let foreign = track(&app, "f1", "ForeignOnly").await;
    let bob_tag = tag(&app, bob, "bobtag", "BobTag").await;
    resolve_tag(&app, foreign, bob_tag).await;

    // A track carrying Alice's own tag is visible.
    let own = track(&app, "o1", "OwnTrack").await;
    let alice_tag = tag(&app, alice, "alicetag", "AliceTag").await;
    resolve_tag(&app, own, alice_tag).await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/kellner/{did}/template/preview"),
        "comment_template_id=0&comment_template={tags}",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("OwnTrack"), "own tag visible: {html}");
    assert!(html.contains("AliceTag"), "own tag rendered");
    assert!(!html.contains("ForeignOnly"), "foreign tag hidden: {html}");
    assert!(html.contains("1 von 1"), "foreign track not counted: {html}");
}

#[tokio::test]
async fn tagged_tracks_shown_without_backpack() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    // No backpack at all — the source is the user's tagged tracks.
    let t1 = track(&app, "n1", "NoBackpack").await;
    tag_with(&app, uid, t1, "n", 2).await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/kellner/{did}/template/preview"),
        "comment_template_id=0&comment_template={tags}",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("NoBackpack"), "tagged track without bp: {html}");
    assert!(html.contains("1 von 1"), "counter: {html}");
}

#[tokio::test]
async fn library_preview_uses_users_tagged_tracks() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;

    let t1 = track(&app, "l1", "Lib-Track").await;
    let tg = tag(&app, uid, "house", "House").await;
    resolve_tag(&app, t1, tg).await;

    let tid = template(&app, Some(uid), "Lib", "{tags}", false).await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/comment-templates/{tid}/preview"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Lib-Track"), "tagged track: {html}");
    assert!(html.contains("House"), "rendered tag");
    assert!(html.contains("1 von 1"), "counter: {html}");
}

// ── render: facts join, cut, empty hint ──────────────────────────────────────

#[tokio::test]
async fn preview_renders_tags_and_audio_from_hub() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let t1 = track(&app, "r1", "Rendered").await;
    let tg = tag(&app, uid, "techno", "Techno").await;
    resolve_tag(&app, t1, tg).await;
    audio(&app, t1, Some(128.0), Some("8A")).await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/kellner/{did}/template/preview"),
        "comment_template_id=0&comment_template={tags} {bpm} {key}",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Techno BPM-128 KEY-8A"), "rendered facts: {html}");
}

#[tokio::test]
async fn over_512_chars_is_cut_with_marker() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let t1 = track(&app, "c1", "Long").await;
    let tg = tag(&app, uid, "long", "LongTag").await;
    resolve_tag(&app, t1, tg).await;

    // A 600-char literal body pushes the rendered Comment past 512.
    let body = format!("{} {{tags}}", "x".repeat(600));
    let tid = template(&app, Some(uid), "Long", &body, false).await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/kellner/{did}/template/preview"),
        &format!("comment_template_id={tid}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("512/512"), "char counter at the ceiling: {html}");
    assert!(html.contains("..."), "cut marker visible");
    assert!(!html.contains(&"x".repeat(600)), "the filler was cut");
}

#[tokio::test]
async fn device_without_tagged_tracks_shows_hint() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    // A track exists but carries no tag → nothing to preview.
    let _t = track(&app, "e1", "Untagged").await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/kellner/{did}/template/preview"),
        "comment_template_id=0",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("hub-empty"), "hint rendered: {html}");
    assert!(
        html.contains("Keine getaggten Tracks — Vorschau nicht möglich."),
        "hint text: {html}"
    );
    assert!(!html.contains("<li"), "no rows");
}

// ── live preview on /comment-templates (#681, slice C8) ──────────────────────

/// The body field posts its current value to the preview route, so the fragment
/// must render the **typed** body, not the stored one.
#[tokio::test]
async fn library_preview_uses_submitted_body() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;

    let t1 = track(&app, "lp1", "LiveTrack").await;
    let tg = tag(&app, uid, "house", "House").await;
    resolve_tag(&app, t1, tg).await;
    audio(&app, t1, Some(128.0), Some("8A")).await;

    // Stored body renders tags; the submitted body renders BPM instead.
    let tid = template(&app, Some(uid), "Live", "{tags}", false).await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/comment-templates/{tid}/preview"),
        "body=%7Bbpm%7D",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("BPM-128"), "typed body rendered: {html}");
    assert!(!html.contains("House"), "stored body ignored: {html}");
}

/// After a save the page renders the preview server-side (no click needed) and
/// the preview button is a real POST form, so it works without JavaScript.
#[tokio::test]
async fn library_page_renders_initial_preview_and_noscript_form() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;

    let t1 = track(&app, "pg1", "PageTrack").await;
    let tg = tag(&app, uid, "house", "House").await;
    resolve_tag(&app, t1, tg).await;

    let tid = template(&app, Some(uid), "Paged", "{tags}", false).await;

    let resp = app
        .client()
        .get(app.url("/comment-templates"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("get page");
    assert_eq!(resp.status(), StatusCode::OK);
    let html = resp.text().await.expect("page body");

    assert!(html.contains("PageTrack"), "preview rendered on load: {html}");
    assert!(html.contains("1 von 1"), "counter on load: {html}");
    assert!(
        html.contains(&format!("action=\"/comment-templates/{tid}/preview\"")),
        "no-JS button is a real form: {html}"
    );
    assert!(
        html.contains(&format!("hx-post=\"/comment-templates/{tid}/preview\"")),
        "live input is wired to htmx: {html}"
    );
    assert!(
        html.contains("hx-trigger=\"keyup changed delay:400ms\""),
        "live typing debounce: {html}"
    );
}

/// Same seed, same source: the kellner route with the stored template and the
/// library route must produce the same fragment (#681 acceptance).
#[tokio::test]
async fn kellner_and_library_preview_match_same_seed() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let t1 = track(&app, "par1", "Parity-A").await;
    let t2 = track(&app, "par2", "Parity-B").await;
    tag_with(&app, uid, t1, "pa", 2).await;
    tag_with(&app, uid, t2, "pb", 1).await;
    audio(&app, t1, Some(128.0), Some("8A")).await;

    let tid = template(&app, Some(uid), "Parity", "{tags} {bpm} {key}", false).await;

    let (status, kellner) = post_form(
        &app,
        Some(&cookie),
        &format!("/kellner/{did}/template/preview"),
        &format!("comment_template_id={tid}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, library) = post_form(
        &app,
        Some(&cookie),
        &format!("/comment-templates/{tid}/preview"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(kellner, library, "both routes render the same fragment");
}
