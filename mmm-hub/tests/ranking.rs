//! Integration tests for wiring similarity v2 + ripeness into Digging/Overlap
//! (#223), plus the Rumpelkiste exception in playlist co-occurrence (#222).

mod common;

use mmm_hub::tags;

/// Adding a `(track, tag)` edge straight into the materialized resolved table.
async fn resolved(pool: &sqlx::SqlitePool, track: i64, tag: i64) {
    sqlx::query("INSERT OR IGNORE INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track)
        .bind(tag)
        .execute(pool)
        .await
        .unwrap();
}

/// A playlist that feeds a Rumpelkiste tag must not create co-occurrence
/// suggestions: `t_all` only co-occurs with `t_pl` inside alice's playlist, so
/// once that playlist is a Rumpelkiste source the suggestion disappears (#222).
#[tokio::test]
async fn rumpelkiste_playlist_is_ignored_in_digging_cooccurrence() {
    let app = common::spawn().await;
    let alice = app.seed.alice;

    let before = mmm_hub::digging::internal_suggestions(&app.pool, app.seed.t_pl, 100)
        .await
        .unwrap();
    assert!(
        before.iter().any(|s| s.id == app.seed.t_all),
        "baseline: t_all should co-occur with t_pl"
    );

    // Turn alice's "Deep House" playlist into a Rumpelkiste tag source.
    let tag = tags::create_from_playlist(&app.pool, alice, app.seed.pl_alice)
        .await
        .unwrap();
    let rumpel = tags::create_group(&app.pool, alice, "Rumpelkiste", "")
        .await
        .unwrap();
    tags::add_tag_to_group(&app.pool, alice, tag, rumpel)
        .await
        .unwrap();

    let after = mmm_hub::digging::internal_suggestions(&app.pool, app.seed.t_pl, 100)
        .await
        .unwrap();
    assert!(
        !after.iter().any(|s| s.id == app.seed.t_all),
        "Rumpelkiste playlist edge must be ignored"
    );
}

/// `similarity_to_seed` scores candidates that share tags with the seed (batched),
/// and gives cross-group matches the configured bonus.
#[tokio::test]
async fn similarity_to_seed_scores_shared_cross_group_tags() {
    let app = common::spawn().await;
    let me = app.seed.alice;
    let mood = tags::create_group(&app.pool, me, "Mood", "").await.unwrap();
    let vibe = tags::create_group(&app.pool, me, "Vibe", "").await.unwrap();
    let dark = tags::ensure_tag(&app.pool, me, "Dark").await.unwrap();
    let ware = tags::ensure_tag(&app.pool, me, "Warehouse").await.unwrap();
    tags::add_tag_to_group(&app.pool, me, dark, mood)
        .await
        .unwrap();
    tags::add_tag_to_group(&app.pool, me, ware, vibe)
        .await
        .unwrap();

    // Seed + candidate both tagged {Dark(Mood), Warehouse(Vibe)} -> cross-group pair.
    for tr in [app.seed.t_pl, app.seed.t_all] {
        resolved(&app.pool, tr, dark).await;
        resolved(&app.pool, tr, ware).await;
    }

    let e = mmm_hub::settings::engine(&app.pool).await;
    let m = mmm_hub::similarity::similarity_to_seed(
        &app.pool,
        app.seed.t_pl,
        &[app.seed.t_all, app.seed.t_carol],
        &e,
    )
    .await;

    let s_all = m.get(&app.seed.t_all).copied().unwrap_or(0.0);
    // base(1.0)*2 shared tags + cross(1.5)*1 pair = 3.5 by default.
    assert!(
        s_all >= 3.0,
        "shared cross-group tags must score, got {s_all}"
    );
    assert!(
        m.get(&app.seed.t_carol).copied().unwrap_or(0.0) == 0.0,
        "a track sharing no tags must not score"
    );
}

/// Batched ripeness agrees with the single-track computation.
#[tokio::test]
async fn ripeness_many_matches_single_track_scoring() {
    let app = common::spawn().await;
    let me = app.seed.alice;
    let g = tags::create_group(&app.pool, me, "Mood", "").await.unwrap();
    let a = tags::ensure_tag(&app.pool, me, "Dark").await.unwrap();
    let b = tags::ensure_tag(&app.pool, me, "Deep").await.unwrap();
    tags::add_tag_to_group(&app.pool, me, a, g).await.unwrap();
    tags::add_tag_to_group(&app.pool, me, b, g).await.unwrap();
    resolved(&app.pool, app.seed.t_all, a).await;
    resolved(&app.pool, app.seed.t_all, b).await;

    let e = mmm_hub::settings::engine(&app.pool).await;
    let single = mmm_hub::scoring::ripeness(&app.pool, app.seed.t_all, &e)
        .await
        .total;
    let many = mmm_hub::scoring::ripeness_many(&app.pool, &[app.seed.t_all], &e).await;
    let got = many.get(&app.seed.t_all).copied().unwrap_or(-1.0);
    assert!(
        (single - got).abs() < 1e-6,
        "batched {got} must equal single {single}"
    );
}

/// The digging view exposes the new similarity + ripeness signals as columns.
#[tokio::test]
async fn digging_renders_similarity_and_ripeness_columns() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url(&format!("/digging?seed={}", app.seed.t_pl)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains(">Sim<"), "similarity column missing");
    assert!(html.contains(">Ripe<"), "ripeness column missing");
}

/// `?sort=ripeness` ranks the overlap view by data completeness.
#[tokio::test]
async fn overlap_sorts_by_ripeness() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url("/overlap?sort=ripeness"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Ripe"), "ripeness sort header missing");
    assert!(html.contains("Shared Anthem"), "shared track missing");
}
