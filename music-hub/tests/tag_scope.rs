//! Integration tests for the per-user tag scope (#267).
//!
//! Seeded world: alice + bob share one collective, carol is outside. alice owns
//! `T_alice`, bob owns `T_bob`, carol owns `T_carol`. Track `X` carries
//! `T_alice`, track `Y` carries `T_alice + T_bob`.

mod common;

use music_hub::scope::{self, TagScope};
use music_hub::settings;
use music_hub::similarity;
use music_hub::user_settings;

/// A seeded universe: users, tags and the two tagged tracks.
struct World {
    app: common::TestApp,
    t_x: i64,
    t_y: i64,
    tag_alice: i64,
    tag_bob: i64,
    tag_carol: i64,
}

async fn insert_track(pool: &sqlx::SqlitePool, sid: &str, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists, first_seen_at)
         VALUES ('spotify', ?1, ?2, 'Aaa', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(sid)
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_tag(pool: &sqlx::SqlitePool, owner: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, type, created_at)
         VALUES (?1, ?2, ?2, 'MOOD', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn link(pool: &sqlx::SqlitePool, track_id: i64, tag_id: i64) {
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track_id)
        .bind(tag_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn seed_world() -> World {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let bob = app.seed.bob;
    let carol = app.seed.carol;

    // alice + bob share one collective; carol is outside.
    let cid = music_hub::tags::create_collective(&app.pool, alice, "Alpha Collective", "")
        .await
        .unwrap();
    music_hub::tags::join_collective(&app.pool, bob, cid)
        .await
        .unwrap();

    let tag_alice = insert_tag(&app.pool, alice, "T_alice").await;
    let tag_bob = insert_tag(&app.pool, bob, "T_bob").await;
    let tag_carol = insert_tag(&app.pool, carol, "T_carol").await;

    let t_x = insert_track(&app.pool, "tsync-x", "Track X").await;
    let t_y = insert_track(&app.pool, "tsync-y", "Track Y").await;
    link(&app.pool, t_x, tag_alice).await;
    link(&app.pool, t_x, tag_bob).await;
    link(&app.pool, t_y, tag_alice).await;
    link(&app.pool, t_y, tag_bob).await;

    // Track Z shares alice's tag with X but carries a bob-only tag (`T_bob2`)
    // that X does not have — so it is only recommendable under `collective`.
    let tag_bob2 = insert_tag(&app.pool, bob, "T_bob2").await;
    let t_z = insert_track(&app.pool, "tsync-z", "Track Z").await;
    link(&app.pool, t_z, tag_alice).await;
    link(&app.pool, t_z, tag_bob2).await;

    // alice + bob both like X and Y, so the overlap page considers them shared.
    for uid in [alice, bob] {
        for tid in [t_x, t_y] {
            sqlx::query(
                "INSERT INTO hub_liked_tracks (user_id, track_id, liked_at)
                 VALUES (?1, ?2, '2026-01-01T00:00:00+00:00')",
            )
            .bind(uid)
            .bind(tid)
            .execute(&app.pool)
            .await
            .unwrap();
        }
    }

    World {
        app,
        t_x,
        t_y,
        tag_alice,
        tag_bob,
        tag_carol,
    }
}

// ── US2: scope resolution ───────────────────────────────────────────────────

#[tokio::test]
async fn own_yields_only_the_viewer() {
    let w = seed_world().await;
    let owners = scope::visible_owner_ids(&w.app.pool, w.app.seed.alice, TagScope::Own).await;
    assert_eq!(owners, vec![w.app.seed.alice]);
}

#[tokio::test]
async fn collective_yields_viewer_and_shared_members() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let bob = w.app.seed.bob;
    let carol = w.app.seed.carol;

    let mut owners: Vec<i64> =
        scope::visible_owner_ids(&w.app.pool, alice, TagScope::Collective).await;
    owners.sort_unstable();
    assert_eq!(owners, vec![alice.min(bob), alice.max(bob)]);
    assert!(!owners.contains(&carol), "carol shares no collective");

    // carol is outside the collective: collective == own for her.
    let carol_owners =
        scope::visible_owner_ids(&w.app.pool, carol, TagScope::Collective).await;
    assert_eq!(carol_owners, vec![carol]);
}

#[tokio::test]
async fn default_is_own_and_query_param_wins_over_setting() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;

    // No setting, no param -> own.
    let eff = scope::effective(&w.app.pool, alice, None).await;
    assert_eq!(eff, TagScope::Own);

    // Setting collective, no param -> collective.
    user_settings::set(&w.app.pool, alice, user_settings::TAG_SCOPE, "collective")
        .await
        .unwrap();
    assert_eq!(
        scope::effective(&w.app.pool, alice, None).await,
        TagScope::Collective
    );

    // Query param wins over the setting (both directions).
    assert_eq!(
        scope::effective(&w.app.pool, alice, Some("own")).await,
        TagScope::Own
    );
    user_settings::set(&w.app.pool, alice, user_settings::TAG_SCOPE, "own")
        .await
        .unwrap();
    assert_eq!(
        scope::effective(&w.app.pool, alice, Some("collective")).await,
        TagScope::Collective
    );

    // A bogus param falls back to the setting.
    assert_eq!(
        scope::effective(&w.app.pool, alice, Some("bogus")).await,
        TagScope::Own
    );
}

#[tokio::test]
async fn user_settings_get_set_all_roundtrip() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    assert_eq!(
        user_settings::get(&w.app.pool, alice, user_settings::TAG_SCOPE).await,
        None
    );
    user_settings::set(&w.app.pool, alice, user_settings::TAG_SCOPE, "collective")
        .await
        .unwrap();
    assert_eq!(
        user_settings::get(&w.app.pool, alice, user_settings::TAG_SCOPE)
            .await
            .as_deref(),
        Some("collective")
    );
    let all = user_settings::all(&w.app.pool, alice).await;
    assert_eq!(all.get(user_settings::TAG_SCOPE).map(String::as_str), Some("collective"));

    // set_validated rejects an out-of-enum value and writes nothing.
    assert!(
        user_settings::set_validated(&w.app.pool, alice, user_settings::TAG_SCOPE, "bogus")
            .await
            .is_err()
    );
    assert_eq!(
        user_settings::get(&w.app.pool, alice, user_settings::TAG_SCOPE)
            .await
            .as_deref(),
        Some("collective")
    );
}

// ── US3: engine core honours owners ─────────────────────────────────────────

#[tokio::test]
async fn tag_similarity_differs_own_vs_collective() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let bob = w.app.seed.bob;
    let e = settings::engine(&w.app.pool).await;

    let own = similarity::tag_similarity(&w.app.pool, w.t_x, w.t_y, &e, &[alice]).await;
    let coll = similarity::tag_similarity(&w.app.pool, w.t_x, w.t_y, &e, &[alice, bob]).await;
    assert_eq!(own, 1.0, "own counts only alice's shared tag");
    assert_eq!(coll, 2.0, "collective counts alice + bob shared tags");
    assert!(coll > own, "collective must score higher");

    // carol is outside the collective: collective == own for her.
    let carol = w.app.seed.carol;
    let own_c = similarity::tag_similarity(&w.app.pool, w.t_x, w.t_y, &e, &[carol]).await;
    let coll_c = similarity::tag_similarity(&w.app.pool, w.t_x, w.t_y, &e, &[carol]).await;
    assert_eq!(own_c, coll_c);
}

#[tokio::test]
async fn similarity_to_seed_differs_own_vs_collective() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let bob = w.app.seed.bob;
    let e = settings::engine(&w.app.pool).await;

    let own = similarity::similarity_to_seed(&w.app.pool, w.t_x, &[w.t_y], &e, &[alice]).await;
    let coll =
        similarity::similarity_to_seed(&w.app.pool, w.t_x, &[w.t_y], &e, &[alice, bob]).await;
    assert_eq!(own.get(&w.t_y).copied(), Some(1.0));
    assert_eq!(coll.get(&w.t_y).copied(), Some(2.0));
}

#[tokio::test]
async fn ripeness_differs_own_vs_collective() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let bob = w.app.seed.bob;
    let e = settings::engine(&w.app.pool).await;

    let own = music_hub::scoring::ripeness(&w.app.pool, w.t_y, &e, &[alice]).await;
    let coll = music_hub::scoring::ripeness(&w.app.pool, w.t_y, &e, &[alice, bob]).await;
    assert_eq!(own.tag_score, 100.0, "one alice tag in the MOOD bucket");
    assert_eq!(coll.tag_score, 150.0, "alice + bob tag -> second tier");
    assert!(coll.total > own.total);

    let own_many =
        music_hub::scoring::ripeness_many(&w.app.pool, &[w.t_y], &e, &[alice]).await;
    let coll_many =
        music_hub::scoring::ripeness_many(&w.app.pool, &[w.t_y], &e, &[alice, bob]).await;
    assert!(coll_many.get(&w.t_y).unwrap() > own_many.get(&w.t_y).unwrap());
}

// ── US4: overlap + digging wiring ───────────────────────────────────────────

async fn get(app: &common::TestApp, path: &str, cookie: &str) -> String {
    app.client()
        .get(app.url(path))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

#[tokio::test]
async fn overlap_tag_filter_honours_scope() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let bob = w.app.seed.bob;
    let cookie = w.app.session_cookie(alice).await;
    let users = format!("{alice},{bob}");

    // `T_bob` is owned by bob. Under `own` (viewer=alice) it is filtered out, so
    // no shared track matches; under `collective` bob's tag counts.
    let own = get(
        &w.app,
        &format!("/overlap?users={users}&tag=T_bob&tag_scope=own"),
        &cookie,
    )
    .await;
    assert!(!own.contains("Track X"), "own must exclude bob's tag: {own}");

    let coll = get(
        &w.app,
        &format!("/overlap?users={users}&tag=T_bob&tag_scope=collective"),
        &cookie,
    )
    .await;
    assert!(
        coll.contains("Track X"),
        "collective must include bob's tag: {coll}"
    );
}

#[tokio::test]
async fn digging_exposes_scope_toggle() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;
    let html = get(&w.app, &format!("/digging?seed={}", w.t_x), &cookie).await;
    assert!(
        html.contains("tag_scope=collective"),
        "digging must offer the collective toggle"
    );
    assert!(html.contains("tag_scope=own"), "digging must offer the own toggle");
}

// ── US5: recommend / tag-queue / track detail ───────────────────────────────

#[tokio::test]
async fn recommend_scoped_differs_own_vs_collective() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let bob = w.app.seed.bob;

    let own = music_hub::recommend::recommend_tags_scoped(&w.app.pool, w.t_x, 20, &[alice]).await;
    let coll =
        music_hub::recommend::recommend_tags_scoped(&w.app.pool, w.t_x, 20, &[alice, bob]).await;
    let names = |v: &[music_hub::recommend::Recommendation]| {
        v.iter().map(|r| r.name.clone()).collect::<Vec<_>>()
    };
    assert!(
        !names(&own).iter().any(|n| n == "T_bob2"),
        "own must not recommend bob's tag: {:?}",
        names(&own)
    );
    assert!(
        names(&coll).iter().any(|n| n == "T_bob2"),
        "collective must recommend bob's tag: {:?}",
        names(&coll)
    );
}

#[tokio::test]
async fn track_detail_recommend_honours_tag_scope_param() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;

    let own = get(
        &w.app,
        &format!("/track/{}?tag_scope=own", w.t_x),
        &cookie,
    )
    .await;
    assert!(
        !own.contains("T_bob2"),
        "own track detail must hide bob's recommendation"
    );

    let coll = get(
        &w.app,
        &format!("/track/{}?tag_scope=collective", w.t_x),
        &cookie,
    )
    .await;
    assert!(
        coll.contains("T_bob2"),
        "collective track detail must show bob's recommendation"
    );
}

#[tokio::test]
async fn track_detail_scope_and_rec_scope_coexist() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;
    let html = get(
        &w.app,
        &format!("/track/{}?tag_scope=collective&scope=mine", w.t_x),
        &cookie,
    )
    .await;
    // The legacy rec-scope toggle links and the engine toggle links both exist,
    // and the mutation redirect keeps both axes.
    assert!(html.contains("scope=mine"));
    assert!(html.contains("tag_scope=collective"));
}

// ── US6: /settings GET/POST ─────────────────────────────────────────────────

#[tokio::test]
async fn settings_get_defaults_to_own_and_post_persists() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;

    // No setting yet -> `own` preselected.
    let before = get(&w.app, "/settings", &cookie).await;
    assert!(before.contains("<strong>own</strong>"), "default must be own");

    let resp = w
        .app
        .client()
        .post(w.app.url("/settings"))
        .header("Cookie", &cookie)
        .form(&[("tag_scope", "collective")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(resp.headers()["location"].to_str().unwrap().starts_with("/settings"));

    let after = get(&w.app, "/settings", &cookie).await;
    assert!(
        after.contains("<strong>collective</strong>"),
        "stored value must be preselected"
    );
    assert_eq!(
        user_settings::get(&w.app.pool, alice, user_settings::TAG_SCOPE)
            .await
            .as_deref(),
        Some("collective")
    );
}

#[tokio::test]
async fn settings_post_validates_and_rejects_bogus() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;

    let resp = w
        .app
        .client()
        .post(w.app.url("/settings"))
        .header("Cookie", &cookie)
        .form(&[("tag_scope", "bogus")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        user_settings::get(&w.app.pool, alice, user_settings::TAG_SCOPE).await,
        None,
        "invalid value must not be stored"
    );
}

#[tokio::test]
async fn setting_takes_effect_without_reload() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let bob = w.app.seed.bob;
    let cookie = w.app.session_cookie(alice).await;

    // Before saving, the default (no setting) keeps bob's tag out of the tag
    // filter under the `own` engine scope for overlap.
    let before = get(
        &w.app,
        &format!("/overlap?users={alice},{bob}&tag=T_bob&tag_scope=own"),
        &cookie,
    )
    .await;
    assert!(!before.contains("Track X"));

    // Save `collective`, then the *same* request without any query param picks it
    // up immediately (fresh read per request).
    w.app
        .client()
        .post(w.app.url("/settings"))
        .header("Cookie", &cookie)
        .form(&[("tag_scope", "collective")])
        .send()
        .await
        .unwrap();

    let after = get(
        &w.app,
        &format!("/overlap?users={alice},{bob}&tag=T_bob"),
        &cookie,
    )
    .await;
    assert!(
        after.contains("Track X"),
        "stored collective scope must apply without a reload"
    );
}

// ── #267 rework: default scope is `own` on detail surfaces ──────────────────

/// Extract the numeric `Ripeness` total rendered on the track page.
fn page_ripeness_total(html: &str) -> i64 {
    let idx = html.find("Ripeness").expect("ripeness block present");
    let rest = &html[idx..];
    let s = rest.find("<strong>").expect("ripeness strong") + "<strong>".len();
    let e = rest[s..].find("</strong>").expect("ripeness close") + s;
    rest[s..e].trim().parse().expect("ripeness is numeric")
}

/// Default ripeness total from `GET /api/hub/tracks/{id}/ripeness`.
async fn api_ripeness_total(app: &common::TestApp, track: i64, cookie: &str, q: &str) -> f64 {
    let body: serde_json::Value = app
        .client()
        .get(app.url(&format!("/api/hub/tracks/{track}/ripeness{q}")))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["data"]["total"].as_f64().expect("ripeness total")
}

#[tokio::test]
async fn track_detail_default_scope_is_own() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;

    // No params, no stored setting -> `own`: bob's tag must not be recommended.
    let plain = get(&w.app, &format!("/track/{}", w.t_x), &cookie).await;
    assert!(
        !plain.contains("T_bob2"),
        "default track detail must be `own` and hide bob's recommendation"
    );

    // The explicit legacy `?scope=all` keeps the old "all owners" behaviour.
    let all = get(&w.app, &format!("/track/{}?scope=all", w.t_x), &cookie).await;
    assert!(
        all.contains("T_bob2"),
        "explicit ?scope=all must still recommend across owners"
    );
}

#[tokio::test]
async fn track_detail_default_ripeness_matches_api() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;

    // Page default and API default must both resolve to `own` and agree.
    let page = get(&w.app, &format!("/track/{}", w.t_x), &cookie).await;
    let page_total = page_ripeness_total(&page);
    let api_total = api_ripeness_total(&w.app, w.t_x, &cookie, "").await;
    assert_eq!(
        page_total,
        api_total.round() as i64,
        "default page and API ripeness must agree (both `own`)"
    );

    // Both surfaces follow the collective scope when explicitly requested.
    let page_coll = get(
        &w.app,
        &format!("/track/{}?tag_scope=collective", w.t_x),
        &cookie,
    )
    .await;
    let api_coll = api_ripeness_total(&w.app, w.t_x, &cookie, "?tag_scope=collective").await;
    assert_eq!(page_ripeness_total(&page_coll), api_coll.round() as i64);
    assert!(api_coll > api_total, "collective ripeness must exceed own");
}

// ── #267 tester: HTTP-surface gaps (tag-queue, visibility, param>setting) ──

/// Form-POST helper (returns the raw response so callers can assert the status).
async fn post_form(
    app: &common::TestApp,
    path: &str,
    cookie: &str,
    form: &[(&str, &str)],
) -> reqwest::Response {
    app.client()
        .post(app.url(path))
        .header("Cookie", cookie)
        .form(form)
        .send()
        .await
        .unwrap()
}

/// The tag-queue detail is a scope-aware engine surface: under `own` bob's
/// `T_bob2` must not be recommended, under `collective` it must be.
#[tokio::test]
async fn tag_queue_detail_honours_tag_scope_param() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;

    let own = get(&w.app, &format!("/tag-queue/{}?tag_scope=own", w.t_x), &cookie).await;
    assert!(
        !own.contains("T_bob2"),
        "own tag-queue must not recommend bob's tag"
    );

    let coll = get(
        &w.app,
        &format!("/tag-queue/{}?tag_scope=collective", w.t_x),
        &cookie,
    )
    .await;
    assert!(
        coll.contains("T_bob2"),
        "collective tag-queue must recommend bob's tag: {coll}"
    );

    // Default (no param, no setting) must be `own`, matching the track page.
    let plain = get(&w.app, &format!("/tag-queue/{}", w.t_x), &cookie).await;
    assert!(
        !plain.contains("T_bob2"),
        "default tag-queue detail must be `own`"
    );
}

/// `?tag_scope` must win over a stored `collective` setting on the track page,
/// and (without a param) the stored setting must apply on the next request.
#[tokio::test]
async fn tag_scope_param_beats_stored_setting_on_track_page() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;

    let resp = post_form(&w.app, "/settings", &cookie, &[("tag_scope", "collective")]).await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    // No param -> stored setting (collective) applies without a reload.
    let stored = get(&w.app, &format!("/track/{}", w.t_x), &cookie).await;
    assert!(
        stored.contains("T_bob2"),
        "stored collective must apply on the next request"
    );

    // Explicit param overrides the stored setting.
    let param = get(
        &w.app,
        &format!("/track/{}?tag_scope=own", w.t_x),
        &cookie,
    )
    .await;
    assert!(
        !param.contains("T_bob2"),
        "?tag_scope=own must beat the stored collective setting"
    );

    // Legacy `?scope=mine` still resolves to `own` for the engine.
    let mine = get(&w.app, &format!("/track/{}?scope=mine", w.t_x), &cookie).await;
    assert!(!mine.contains("T_bob2"), "?scope=mine keeps own");
}

/// Tag *visibility* must be unaffected by the scope: `/tags` and `/tag/{id}`
/// show every owner's tags regardless of `own`/`collective`.
#[tokio::test]
async fn tag_visibility_unaffected_by_scope() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;

    // /tags (default own) lists bob's tag — visibility is global.
    let list_own = get(&w.app, "/tags", &cookie).await;
    assert!(list_own.contains("T_bob"), "/tags must list every owner's tags");

    // Store collective; visibility must not change.
    post_form(&w.app, "/settings", &cookie, &[("tag_scope", "collective")]).await;
    let list_coll = get(&w.app, "/tags", &cookie).await;
    assert!(list_coll.contains("T_bob"), "/tags visibility must be scope-independent");

    // /tag/{bob} detail shows the tagged track under both scopes.
    let detail = get(&w.app, &format!("/tag/{}", w.tag_bob), &cookie).await;
    assert!(
        detail.contains("Track X"),
        "/tag/{{id}} must show the track regardless of scope"
    );
    assert_eq!(
        user_settings::get(&w.app.pool, alice, user_settings::TAG_SCOPE)
            .await
            .as_deref(),
        Some("collective"),
        "setting persisted (sanity)"
    );
}

/// Overlap: the explicit `?tag_scope` param must beat a stored `own` setting.
#[tokio::test]
async fn overlap_param_beats_stored_setting() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let bob = w.app.seed.bob;
    let cookie = w.app.session_cookie(alice).await;

    // Store `own`: the tag filter must exclude bob's `T_bob` by default.
    post_form(&w.app, "/settings", &cookie, &[("tag_scope", "own")]).await;
    let plain = get(
        &w.app,
        &format!("/overlap?users={alice},{bob}&tag=T_bob"),
        &cookie,
    )
    .await;
    assert!(!plain.contains("Track X"), "stored own must exclude bob's tag");

    // Param overrides the stored setting.
    let coll = get(
        &w.app,
        &format!("/overlap?users={alice},{bob}&tag=T_bob&tag_scope=collective"),
        &cookie,
    )
    .await;
    assert!(
        coll.contains("Track X"),
        "?tag_scope=collective must beat the stored own setting"
    );
}

/// Extract the `href` of the "Eigene Tags" (own) toggle on `/overlap`,
/// unescaping the HTML entity the template emits for `&`.
fn own_toggle_href(html: &str) -> String {
    let idx = html.find("Eigene Tags").expect("own toggle present");
    let before = &html[..idx];
    let start = before.rfind("href=\"").expect("href before own label") + "href=\"".len();
    let end = before[start..].find('"').expect("href end") + start;
    before[start..end]
        .replace("&#38;", "&")
        .replace("&amp;", "&")
}

/// Regression (#267): with the stored setting `collective`, the `/overlap`
/// "Eigene Tags" toggle must carry an explicit `tag_scope=own`, otherwise the
/// click sends no param and `scope::effective` falls back to `collective` — the
/// toggle would be dead.
#[tokio::test]
async fn overlap_own_toggle_carries_explicit_param_under_collective_setting() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let bob = w.app.seed.bob;
    let cookie = w.app.session_cookie(alice).await;

    // Store `collective` so an own-URL without an explicit param would resolve
    // back to `collective` on click.
    post_form(&w.app, "/settings", &cookie, &[("tag_scope", "collective")]).await;

    let html = get(
        &w.app,
        &format!("/overlap?users={alice},{bob}&tag=T_bob"),
        &cookie,
    )
    .await;

    let own_url = own_toggle_href(&html);
    assert!(
        own_url.contains("tag_scope=own"),
        "own toggle must carry an explicit tag_scope=own: {own_url}"
    );

    // And clicking it must actually switch to `own` (bob's `T_bob` filtered out).
    let own = get(&w.app, &own_url, &cookie).await;
    assert!(
        !own.contains("Track X"),
        "own toggle URL must exclude bob's tag: {own}"
    );
}

/// `/digging?seed=` accepts `tag_scope` and reflects the active scope in its
/// toggle, under both an explicit param and a stored setting.
#[tokio::test]
async fn digging_accepts_tag_scope_param_and_setting() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;

    for s in ["own", "collective", "bogus"] {
        let html = get(
            &w.app,
            &format!("/digging?seed={}&tag_scope={s}", w.t_x),
            &cookie,
        )
        .await;
        assert!(!html.is_empty(), "digging?tag_scope={s} must render");
        assert!(html.contains("tag_scope=collective") && html.contains("tag_scope=own"));
    }

    post_form(&w.app, "/settings", &cookie, &[("tag_scope", "collective")]).await;
    let stored = get(&w.app, &format!("/digging?seed={}", w.t_x), &cookie).await;
    assert!(
        stored.contains("tag_scope=collective"),
        "digging must surface the stored scope"
    );
}
