//! Integration tests for the comment-template preview (issue #670, slice C4).
//!
//! Covers the `/kellner/{id}/template/preview` and
//! `/comment-templates/{id}/preview` htmx fragments against the `common`
//! harness: session gating, `404` scoping, the union/dedup/export-order track
//! source, the `10 von N` + `n/512` counters, the `...` cut and the "no active
//! backpack" hint. No music-api is configured in the harness, so a `200` also
//! proves the preview never reaches out (acceptance: no network call / no
//! `MUSIC_API_TOKEN`).

mod common;

use music_hub::backpacks::{self, SourceRef};
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

async fn audio(app: &common::TestApp, track_id: i64, bpm: Option<f64>, key: Option<&str>) {
    sqlx::query("INSERT INTO hub_track_audio (track_id, bpm, musical_key) VALUES (?1, ?2, ?3)")
        .bind(track_id)
        .bind(bpm)
        .bind(key)
        .execute(&app.pool)
        .await
        .expect("audio");
}

/// Create a backpack holding `track_ids` in order (literal track sources).
async fn backpack_with(app: &common::TestApp, user_id: i64, name: &str, track_ids: &[i64]) -> i64 {
    let bp = backpacks::create_backpack(&app.pool, user_id, name)
        .await
        .expect("create backpack");
    for t in track_ids {
        backpacks::add_source(&app.pool, bp, SourceRef::Track(*t))
            .await
            .expect("add source");
    }
    bp
}

async fn set_device_backpacks(app: &common::TestApp, device_id: i64, ids: &[i64]) {
    for (pos, bid) in ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO hub_device_backpacks (device_id, backpack_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(device_id)
        .bind(bid)
        .bind(pos as i64)
        .bind(TS)
        .execute(&app.pool)
        .await
        .expect("link device backpack");
    }
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
    let alice_dev = bound_device(&app, alice, "alice-k", &"a".repeat(64)).await;    let bob_dev = bound_device(&app, bob, "bob-k", &"c".repeat(64)).await;

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

    let (status, html) = post_form(
        &app,
        Some(&alice_cookie),
        &format!("/comment-templates/{shared}/preview"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "shared read-only → 200");

    let (status, _) = post_form(
        &app,
        Some(&alice_cookie),
        &format!("/comment-templates/{system}/preview"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "system → 200");
    assert!(html.contains("Keine aktiven Backpacks"), "no active bp hint");
}

// ── counters, order, dedup, cut ──────────────────────────────────────────────

#[tokio::test]
async fn device_preview_shows_ten_of_n_and_rows() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    // 15 + 10 distinct tracks in two active backpacks.
    let mut t1: Vec<i64> = Vec::new();
    for i in 0..15 {
        t1.push(track(&app, &format!("s1{i}"), &format!("T1-{i}")).await);
    }
    let mut t2: Vec<i64> = Vec::new();
    for i in 0..10 {
        t2.push(track(&app, &format!("s2{i}"), &format!("T2-{i}")).await);
    }
    let bpa = backpack_with(&app, uid, "real-a", &t1).await;
    let bpb = backpack_with(&app, uid, "real-b", &t2).await;
    set_device_backpacks(&app, did, &[bpa, bpb]).await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/kellner/{did}/template/preview"),
        "comment_template_id=0&comment_template={tags}",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("10 von 25"), "counter: {html}");
    assert!(html.contains("T1-0"), "first track listed");
    assert!(html.contains("/512"), "char counter shown");
    assert!(!html.contains("T1-11"), "only the first ten rendered");
}

#[tokio::test]
async fn union_dedups_and_keeps_export_order() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let t1 = track(&app, "u1", "Erste").await;
    let t2 = track(&app, "u2", "Zweite").await;
    let t3 = track(&app, "u3", "Dritte").await;
    // bp1 = [t1, t2]; bp2 = [t2, t3]; t2 must appear exactly once.
    let bpa = backpack_with(&app, uid, "u-a", &[t1, t2]).await;
    let bpb = backpack_with(&app, uid, "u-b", &[t2, t3]).await;
    set_device_backpacks(&app, did, &[bpa, bpb]).await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/kellner/{did}/template/preview"),
        "comment_template_id=0&comment_template={tags}",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("3 von 3"), "deduped total: {html}");
    let first = html.find("Erste").expect("Erste present");
    let second = html.find("Zweite").expect("Zweite present");
    let third = html.find("Dritte").expect("Dritte present");
    assert!(first < second && second < third, "export order preserved");
    assert_eq!(html.matches("Zweite").count(), 1, "t2 not duplicated");
}

#[tokio::test]
async fn over_512_chars_is_cut_with_marker() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let t1 = track(&app, "c1", "Long").await;
    let bp = backpack_with(&app, uid, "c-bp", &[t1]).await;
    set_device_backpacks(&app, did, &[bp]).await;

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

// ── tag/audio join + library variant union ──────────────────────────────────

#[tokio::test]
async fn preview_renders_tags_and_audio_from_hub() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let t1 = track(&app, "r1", "Rendered").await;
    let tg = tag(&app, uid, "techno", "Techno").await;
    resolve_tag(&app, t1, tg).await;
    audio(&app, t1, Some(128.0), Some("8A")).await;
    let bp = backpack_with(&app, uid, "r-bp", &[t1]).await;
    set_device_backpacks(&app, did, &[bp]).await;

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
async fn library_preview_uses_active_backpacks_of_user() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;

    let t1 = track(&app, "l1", "Lib-Track").await;
    let bp_active = backpack_with(&app, uid, "lib-active", &[t1]).await;
    // A second, non-active backpack must not contribute.
    let t2 = track(&app, "l2", "Ignored").await;
    let _bp_idle = backpack_with(&app, uid, "lib-idle", &[t2]).await;
    backpacks::set_active(&app.pool, uid, bp_active)
        .await
        .expect("set active");

    let tid = template(&app, Some(uid), "Lib", "{tags}", false).await;
    // Give t1 a tag so the row is non-empty.
    let tg = tag(&app, uid, "house", "House").await;
    resolve_tag(&app, t1, tg).await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/comment-templates/{tid}/preview"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Lib-Track"), "active backpack track: {html}");
    assert!(html.contains("House"), "rendered tag");
    assert!(!html.contains("Ignored"), "idle backpack excluded");
    assert!(html.contains("1 von 1"), "counter: {html}");
}

#[tokio::test]
async fn device_without_active_backpack_shows_hint() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("cp-alice", "1234").await;
    let did = bound_device(&app, uid, "kellner-1", &"a".repeat(64)).await;

    let (status, html) = post_form(
        &app,
        Some(&cookie),
        &format!("/kellner/{did}/template/preview"),
        "comment_template_id=0",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("hub-empty"), "hint rendered: {html}");
    assert!(html.contains("Keine aktiven Backpacks"), "hint text: {html}");
    assert!(!html.contains("<li"), "no rows");
}
