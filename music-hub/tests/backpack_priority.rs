//! Backpack membership drives download priority (#683, slice B2).
//!
//! Covers: `priorities()` counts **every** backpack (even inactive), a
//! backpack-only track is ordered by `order_pending`, `order_pending` and
//! `priorities()` agree on the value, a freshly added source orders its missing
//! tracks with `BACKPACK_PRIORITY`, and the `POST /backpacks/{id}/prioritize`
//! action orders exactly the not-yet-`ready` tracks (or flashes without a token).

mod common;

use std::sync::{Arc, Mutex};

use axum::routing::post;
use axum::Json;
use axum::Router;
use music_hub::config::Config;
use serde_json::{json, Value};

const TS: &str = "2026-01-01T00:00:00+00:00";
/// Mirrors `pages::BACKPACK_PRIORITY` (private); the issue mandates `>= 5`.
const EXPECTED_BACKPACK_PRIORITY: i32 = 10;
/// Mirrors `crawl::BACKPACK_BONUS` (public).
const BONUS: i32 = music_hub::crawl::BACKPACK_BONUS;

/// Boot a mock music-api `/orders` that records every request body.
async fn mock_orders(sink: Arc<Mutex<Vec<Value>>>) -> String {
    let handler = move |Json(body): Json<Value>| {
        let sink = sink.clone();
        async move {
            sink.lock().unwrap().push(body);
            Json(json!({ "orderId": "o1" }))
        }
    };
    let app = Router::new().route("/orders", post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Boot a hub with a mock music-api `/orders` (token set) and seeded fixtures.
async fn spawn_with_orders(sink: Arc<Mutex<Vec<Value>>>) -> common::TestApp {
    let base = mock_orders(sink).await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let mut cfg = Config::for_test(url);
    cfg.music_api_base = base;
    cfg.music_api_token = Some("tok".to_string());
    common::spawn_with(dir, cfg).await
}

/// A backpack row (owned by `user_id`); `active` sets `is_active`.
async fn insert_backpack(pool: &sqlx::SqlitePool, user_id: i64, active: bool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, 'BP', ?2, ?3, ?3) RETURNING id",
    )
    .bind(user_id)
    .bind(i64::from(active))
    .bind(TS)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A `track`-kind backpack item pointing at `track_id`.
async fn insert_item(pool: &sqlx::SqlitePool, backpack_id: i64, track_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_backpack_items
             (backpack_id, source_kind, source_track_id, position, added_at)
         VALUES (?1, 'track', ?2, 1, ?3) RETURNING id",
    )
    .bind(backpack_id)
    .bind(track_id)
    .bind(TS)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// One materialised `(item, track)` row.
async fn materialize(pool: &sqlx::SqlitePool, item_id: i64, track_id: i64) {
    sqlx::query(
        "INSERT INTO hub_backpack_item_tracks (item_id, track_id, backend, computed_at)
         VALUES (?1, ?2, 'track', ?3)",
    )
    .bind(item_id)
    .bind(track_id)
    .bind(TS)
    .execute(pool)
    .await
    .unwrap();
}

async fn set_isrc(pool: &sqlx::SqlitePool, track_id: i64, isrc: &str) {
    sqlx::query("UPDATE hub_tracks SET isrc = ?1 WHERE id = ?2")
        .bind(isrc)
        .bind(track_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn set_ready(pool: &sqlx::SqlitePool, isrc: &str) {
    sqlx::query("INSERT INTO hub_music_state (isrc, state, checked_at) VALUES (?1, 'ready', ?2)")
        .bind(isrc)
        .bind(TS)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn priorities_add_backpack_bonus_even_for_inactive_backpack() {
    let app = common::spawn().await;
    // `t_alice` is liked only (base priority 1). Put it in an **inactive**
    // backpack; it must gain exactly `BACKPACK_BONUS` (#683).
    let bp = insert_backpack(&app.pool, app.seed.alice, false).await;
    let item = insert_item(&app.pool, bp, app.seed.t_alice).await;
    materialize(&app.pool, item, app.seed.t_alice).await;

    let p = music_hub::crawl::priorities(&app.pool, &[app.seed.t_alice, app.seed.t_bob]).await;
    assert_eq!(
        p[&app.seed.t_alice],
        1 + BONUS,
        "backpack member gains the bonus (inactive backpack counts)"
    );
    assert_eq!(
        p[&app.seed.t_bob], 1,
        "non-member keeps its like-only priority"
    );
}

#[tokio::test]
async fn order_pending_orders_backpack_only_track() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = spawn_with_orders(sink.clone()).await;

    // A track with **no** like and **no** playlist — only a backpack reference.
    let track: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists, isrc, first_seen_at)
         VALUES ('spotify', 'bp_only', 'Backpack Only', 'Zzz', 'ISRC-BP', ?1) RETURNING id",
    )
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    let bp = insert_backpack(&app.pool, app.seed.alice, true).await;
    let item = insert_item(&app.pool, bp, track).await;
    materialize(&app.pool, item, track).await;

    let first = music_hub::crawl::order_pending(&app.pool, &app.cfg, 100)
        .await
        .unwrap();
    assert_eq!(first, 1, "the backpack-only track is ordered");

    {
        let calls = sink.lock().unwrap();
        assert_eq!(calls.len(), 1, "one order group");
        assert_eq!(calls[0]["items"][0]["isrc"], "ISRC-BP");
        assert_eq!(
            calls[0]["priority"], BONUS,
            "backpack-only priority = 0 + 0 + BACKPACK_BONUS"
        );
    }

    let second = music_hub::crawl::order_pending(&app.pool, &app.cfg, 100)
        .await
        .unwrap();
    assert_eq!(second, 0, "unchanged priority re-orders nothing");
}

#[tokio::test]
async fn order_pending_priority_matches_priorities() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = spawn_with_orders(sink.clone()).await;

    // Mixed fixture: like + playlist + backpack across several tracks.
    for (id, isrc) in [
        (app.seed.t_all, "ISRC-ALL"),
        (app.seed.t_two, "ISRC-TWO"),
        (app.seed.t_pl, "ISRC-PL"),
        (app.seed.t_alice, "ISRC-ALICE"),
    ] {
        set_isrc(&app.pool, id, isrc).await;
    }
    // t_alice is a backpack member (bonus), t_pl playlist-only, t_two liked-only.
    let bp = insert_backpack(&app.pool, app.seed.alice, true).await;
    let item = insert_item(&app.pool, bp, app.seed.t_alice).await;
    materialize(&app.pool, item, app.seed.t_alice).await;

    let ids = [
        app.seed.t_all,
        app.seed.t_two,
        app.seed.t_pl,
        app.seed.t_alice,
    ];
    let expected = music_hub::crawl::priorities(&app.pool, &ids).await;

    music_hub::crawl::order_pending(&app.pool, &app.cfg, 100)
        .await
        .unwrap();

    for id in ids {
        let recorded: i64 =
            sqlx::query_scalar("SELECT priority FROM hub_order_state WHERE track_id = ?1")
                .bind(id)
                .fetch_one(&app.pool)
                .await
                .unwrap();
        assert_eq!(
            recorded as i32, expected[&id],
            "order_pending and priorities() agree for track {id}"
        );
    }
    // Spot-check the formula: t_alice = liked(1) + backpack bonus(2).
    assert_eq!(expected[&app.seed.t_alice], 1 + BONUS);
}

#[tokio::test]
async fn add_source_orders_missing_with_backpack_priority() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = spawn_with_orders(sink.clone()).await;

    // An alice-owned tag over 5 tracks, each with an ISRC (none ready).
    let tag: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, 'bp-tag', 'BP Tag', ?2) RETURNING id",
    )
    .bind(app.seed.alice)
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    let tracks = [
        app.seed.t_all,
        app.seed.t_two,
        app.seed.t_pl,
        app.seed.t_alice,
        app.seed.t_bob,
    ];
    for (i, id) in tracks.iter().enumerate() {
        set_isrc(&app.pool, *id, &format!("ISRC-SRC-{i}")).await;
        sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
            .bind(id)
            .bind(tag)
            .execute(&app.pool)
            .await
            .unwrap();
    }

    let bp = insert_backpack(&app.pool, app.seed.alice, true).await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{bp}/sources")))
        .header("Cookie", &cookie)
        .form(&[("source_kind", "tag"), ("source_tag_id", &tag.to_string())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let calls = sink.lock().unwrap();
    assert_eq!(calls.len(), 1, "one order issued for the new source");
    assert_eq!(calls[0]["items"].as_array().unwrap().len(), 5);
    let prio = calls[0]["priority"].as_i64().unwrap();
    assert!(prio >= 5, "new-source order priority >= 5 (got {prio})");
    assert_eq!(prio as i32, EXPECTED_BACKPACK_PRIORITY);
}

#[tokio::test]
async fn prioritize_orders_exactly_the_missing() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = spawn_with_orders(sink.clone()).await;

    // Three materialised tracks: two already `ready`, one not.
    let bp = insert_backpack(&app.pool, app.seed.alice, true).await;
    for (id, isrc, ready) in [
        (app.seed.t_all, "ISRC-R1", true),
        (app.seed.t_two, "ISRC-R2", true),
        (app.seed.t_pl, "ISRC-MISS", false),
    ] {
        set_isrc(&app.pool, id, isrc).await;
        let item = insert_item(&app.pool, bp, id).await;
        materialize(&app.pool, item, id).await;
        if ready {
            set_ready(&app.pool, isrc).await;
        }
    }

    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{bp}/prioritize")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let calls = sink.lock().unwrap();
    assert_eq!(calls.len(), 1, "one order issued");
    let items = calls[0]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "only the missing ISRC is ordered");
    assert_eq!(items[0]["isrc"], "ISRC-MISS");
    assert_eq!(
        calls[0]["priority"].as_i64().unwrap() as i32,
        EXPECTED_BACKPACK_PRIORITY
    );
}

#[tokio::test]
async fn prioritize_without_token_flashes_and_orders_nothing() {
    // Default config has no music-api token.
    let app = common::spawn().await;
    let bp = insert_backpack(&app.pool, app.seed.alice, true).await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{bp}/prioritize")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/backpacks");
}

#[tokio::test]
async fn prioritize_requires_login_and_owner_scope() {
    let app = common::spawn().await;
    let bp = insert_backpack(&app.pool, app.seed.alice, true).await;

    // No session -> redirect to login.
    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{bp}/prioritize")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");

    // A foreign user's backpack is indistinguishable from a missing one (404).
    let bob = app.session_cookie(app.seed.bob).await;
    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{bp}/prioritize")))
        .header("Cookie", &bob)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn add_source_without_token_orders_nothing() {
    let app = common::spawn().await;
    let tag: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, 'no-token-tag', 'No Token', ?2) RETURNING id",
    )
    .bind(app.seed.alice)
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    set_isrc(&app.pool, app.seed.t_all, "ISRC-NT").await;
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(app.seed.t_all)
        .bind(tag)
        .execute(&app.pool)
        .await
        .unwrap();

    let bp = insert_backpack(&app.pool, app.seed.alice, true).await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .post(app.url(&format!("/backpacks/{bp}/sources")))
        .header("Cookie", &cookie)
        .form(&[("source_kind", "tag"), ("source_tag_id", &tag.to_string())])
        .send()
        .await
        .unwrap();
    // Clean flash (redirect), no music-api call (nothing to observe -> no panic).
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/backpacks");
}
