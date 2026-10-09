//! Integration tests for the four hub overlap views (issue #123).
//!
//! The fixture doc-comment in `src/db/testing.rs` states the expected numbers;
//! each test below re-derives them from the view SQL and asserts exact results
//! against the seeded rows.

mod common;

use sqlx::Row;

/// Spawning the app runs every migration against a brand-new temp SQLite file,
/// so simply reaching this point proves the migration chain applies cleanly.
/// Assert the expected tables and views actually exist.
#[tokio::test]
async fn migrations_run_cleanly_on_fresh_db() {
    let app = common::spawn().await;

    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'
           AND name IN ('hub_users', 'hub_tracks', 'hub_playlists',
                        'hub_playlist_tracks', 'hub_liked_tracks')",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(tables, 5, "expected the five core hub tables");

    let views: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'view' AND name LIKE 'hub_v_%'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(views, 4, "expected the four overlap views");
}

/// `hub_v_shared_tracks` should return exactly the tracks present for >= 2
/// distinct users: `t_all` (liked by all 3), `t_two` (liked by alice+bob) and
/// `t_pl` (in alice's + bob's playlists, nobody's likes).
#[tokio::test]
async fn shared_tracks_view_lists_exactly_the_cross_user_tracks() {
    let app = common::spawn().await;

    let rows = sqlx::query(
        "SELECT track_id, user_count FROM hub_v_shared_tracks
          ORDER BY user_count DESC, track_id",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap();

    let got: Vec<(i64, i64)> = rows
        .iter()
        .map(|r| (r.get::<i64, _>("track_id"), r.get::<i64, _>("user_count")))
        .collect();

    assert_eq!(got.len(), 3, "exactly three shared tracks");
    assert_eq!(
        got,
        vec![
            (app.seed.t_all, 3),
            (app.seed.t_two, 2),
            (app.seed.t_pl, 2),
        ]
    );

    // The user-only tracks must not leak in.
    let ids: Vec<i64> = got.iter().map(|(id, _)| *id).collect();
    for excluded in [app.seed.t_alice, app.seed.t_bob, app.seed.t_carol] {
        assert!(!ids.contains(&excluded), "track {excluded} is not shared");
    }
}

/// `hub_v_user_overlap` should count each unordered pair once: alice↔bob share
/// `t_all`, `t_two`, `t_pl` (3); alice↔carol and bob↔carol share only `t_all`.
#[tokio::test]
async fn user_overlap_view_counts_pairwise_shared_tracks() {
    let app = common::spawn().await;

    let mut rows: Vec<(i64, i64, i64)> =
        sqlx::query_as("SELECT user_a_id, user_b_id, shared_tracks FROM hub_v_user_overlap")
            .fetch_all(&app.pool)
            .await
            .unwrap();
    rows.sort();

    assert_eq!(rows.len(), 3, "one row per unordered user pair");
    assert_eq!(
        rows,
        vec![
            (app.seed.alice, app.seed.bob, 3),
            (app.seed.alice, app.seed.carol, 1),
            (app.seed.bob, app.seed.carol, 1),
        ]
    );
}

/// `hub_v_track_playlists` is one row per playlist membership: alice has two
/// (`t_all`, `t_pl`), bob and carol one each → 4 rows.
#[tokio::test]
async fn track_playlists_view_has_one_row_per_membership() {
    let app = common::spawn().await;

    let mut rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT track_id, user_id, playlist_id FROM hub_v_track_playlists",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap();
    rows.sort();

    assert_eq!(rows.len(), 4, "exactly four memberships");
    assert_eq!(
        rows,
        vec![
            (app.seed.t_all, app.seed.alice, app.seed.pl_alice),
            (app.seed.t_pl, app.seed.alice, app.seed.pl_alice),
            (app.seed.t_pl, app.seed.bob, app.seed.pl_bob),
            (app.seed.t_carol, app.seed.carol, app.seed.pl_carol),
        ]
    );
}

/// `hub_v_track_presence` flattens every (track, user, why) fact: 8 likes plus
/// 4 playlist memberships = 12 rows.
#[tokio::test]
async fn track_presence_view_flattens_likes_and_memberships() {
    let app = common::spawn().await;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_v_track_presence")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let liked: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_v_track_presence WHERE source = 'liked'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    let playlisted: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_v_track_presence WHERE source = 'playlist'")
            .fetch_one(&app.pool)
            .await
            .unwrap();

    assert_eq!(liked, 8, "eight likes");
    assert_eq!(playlisted, 4, "four playlist memberships");
    assert_eq!(total, 12, "12 presence rows total");
}
