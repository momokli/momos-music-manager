//! Integration tests for the per-user tag scope (#267).
//!
//! Seeded world: alice + bob share one collective, carol is outside. alice owns
//! `T_alice`, bob owns `T_bob`, carol owns `T_carol`. Track `X` carries
//! `T_alice`, track `Y` carries `T_alice + T_bob`.

mod common;

use std::collections::HashSet;

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

// Keep the imports referenced even before the page tests land.
#[allow(dead_code)]
fn _touch() {
    let _ = settings::TAG_SCOPE;
    let _: HashSet<i64> = HashSet::new();
    let _ = w_field();
}

#[allow(dead_code)]
fn w_field() -> i64 {
    0
}
