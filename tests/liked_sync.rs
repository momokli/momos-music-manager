//! Integration tests for `liked_sync` — merge/retire logic against a real
//! in-memory SQLite database. No network is touched: the fetch layer is
//! bypassed by feeding hand-built [`LikedItem`] fixtures.

mod common;

use std::collections::HashSet;

use momos_music_manager::db;
use momos_music_manager::liked_sync::{LIKED_PLAYLIST_ID, LikedItem, merge_liked_items, retire_missing_likes};
use momos_music_manager::spotify::models::TrackInfo;
use sqlx::{Pool, Sqlite};

fn track(id: &str) -> TrackInfo {
    TrackInfo {
        id: id.to_string(),
        name: format!("Track {id}"),
        artists: "Some Artist".to_string(),
        album: Some("Some Album".to_string()),
        isrc: None,
        duration_ms: 180_000,
        track_number: Some(1),
        disc_number: Some(1),
        explicit: false,
        popularity: Some(50),
    }
}

fn item(id: &str, added_at: i64) -> LikedItem {
    LikedItem {
        track: track(id),
        added_at,
    }
}

/// `added_at` of a membership in the likes mirror for a given Spotify id.
async fn liked_added_at(pool: &Pool<Sqlite>, service_id: &str) -> Option<i64> {
    sqlx::query_scalar(
        r#"
        SELECT spt.added_at
        FROM service_playlist_tracks spt
        JOIN service_playlists sp ON sp.id = spt.playlist_id
        JOIN service_tracks st ON st.id = spt.track_id
        WHERE sp.service = 'spotify' AND sp.playlist_id = ?
          AND st.service = 'spotify' AND st.service_id = ?
        "#,
    )
    .bind(LIKED_PLAYLIST_ID)
    .bind(service_id)
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn liked_deleted_at(pool: &Pool<Sqlite>, service_id: &str) -> Option<Option<i64>> {
    sqlx::query_scalar(
        r#"
        SELECT spt.deleted_at
        FROM service_playlist_tracks spt
        JOIN service_playlists sp ON sp.id = spt.playlist_id
        JOIN service_tracks st ON st.id = spt.track_id
        WHERE sp.service = 'spotify' AND sp.playlist_id = ?
          AND st.service = 'spotify' AND st.service_id = ?
        "#,
    )
    .bind(LIKED_PLAYLIST_ID)
    .bind(service_id)
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn liked_membership_rows(pool: &Pool<Sqlite>) -> i64 {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM service_playlist_tracks spt
        JOIN service_playlists sp ON sp.id = spt.playlist_id
        WHERE sp.service = 'spotify' AND sp.playlist_id = ?
        "#,
    )
    .bind(LIKED_PLAYLIST_ID)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn merge_inserts_new_likes_with_spotify_added_at() {
    let pool = common::create_test_db().await;

    let linked = merge_liked_items(&pool, &[item("t1", 1_111_111)], LIKED_PLAYLIST_ID)
        .await
        .unwrap();

    assert_eq!(linked, 1);
    // Date comes from Spotify, not now().
    assert_eq!(liked_added_at(&pool, "t1").await, Some(1_111_111));
}

#[tokio::test]
async fn merge_updates_added_at_for_existing_like() {
    let pool = common::create_test_db().await;

    merge_liked_items(&pool, &[item("t1", 100)], LIKED_PLAYLIST_ID)
        .await
        .unwrap();
    assert_eq!(liked_added_at(&pool, "t1").await, Some(100));

    // Relike at Spotify moves the date; the merge must refresh it.
    merge_liked_items(&pool, &[item("t1", 200)], LIKED_PLAYLIST_ID)
        .await
        .unwrap();
    assert_eq!(liked_added_at(&pool, "t1").await, Some(200));
}

#[tokio::test]
async fn merge_is_idempotent() {
    let pool = common::create_test_db().await;

    let items = [item("t1", 42), item("t2", 43)];
    merge_liked_items(&pool, &items, LIKED_PLAYLIST_ID)
        .await
        .unwrap();
    let first = liked_membership_rows(&pool).await;

    merge_liked_items(&pool, &items, LIKED_PLAYLIST_ID)
        .await
        .unwrap();
    let second = liked_membership_rows(&pool).await;

    assert_eq!(first, 2);
    assert_eq!(first, second);
}

#[tokio::test]
async fn merge_creates_the_liked_playlist_as_kind_liked() {
    let pool = common::create_test_db().await;

    merge_liked_items(&pool, &[item("t1", 7)], LIKED_PLAYLIST_ID)
        .await
        .unwrap();

    let (service, name, kind): (String, String, String) = sqlx::query_as(
        "SELECT service, name, playlist_kind FROM service_playlists WHERE playlist_id = ?",
    )
    .bind(LIKED_PLAYLIST_ID)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(service, "spotify");
    assert_eq!(name, "liked");
    assert_eq!(kind, "liked");
}

#[tokio::test]
async fn retire_missing_likes_soft_deletes_only_absent_tracks() {
    let pool = common::create_test_db().await;

    merge_liked_items(&pool, &[item("t1", 1), item("t2", 2)], LIKED_PLAYLIST_ID)
        .await
        .unwrap();

    let current: HashSet<String> = ["t1".to_string()].into_iter().collect();
    let retired = retire_missing_likes(&pool, LIKED_PLAYLIST_ID, &current)
        .await
        .unwrap();

    assert_eq!(retired, 1);
    assert_eq!(liked_deleted_at(&pool, "t1").await, Some(None));
    assert!(liked_deleted_at(&pool, "t2").await.unwrap().is_some());
}

#[tokio::test]
async fn retire_missing_likes_keeps_track_in_other_playlists() {
    let pool = common::create_test_db().await;

    // A curated playlist ("Groovy") also contains t2.
    let mut conn = pool.acquire().await.unwrap();
    let groovy = db::upsert_service_playlist(
        &mut conn,
        "spotify",
        "groovy",
        "Groovy",
        None,
        None,
    )
    .await
    .unwrap();
    drop(conn);

    merge_liked_items(&pool, &[item("t1", 1), item("t2", 2)], LIKED_PLAYLIST_ID)
        .await
        .unwrap();

    // Pin t2 into Groovy.
    let mut tx = pool.begin().await.unwrap();
    let db_track = db::upsert_service_track(
        &mut tx,
        "spotify",
        "t2",
        "Track t2",
        "Some Artist",
        None,
        None,
        Some(180_000),
        None,
    )
    .await
    .unwrap();
    db::add_track_to_playlist(&mut tx, groovy.id, db_track.id, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // t2 is no longer liked…
    let current: HashSet<String> = ["t1".to_string()].into_iter().collect();
    retire_missing_likes(&pool, LIKED_PLAYLIST_ID, &current)
        .await
        .unwrap();

    // …but its Groovy membership survives.
    let groovy_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM service_playlist_tracks WHERE playlist_id = ? AND track_id = ?",
    )
    .bind(groovy.id)
    .bind(db_track.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(groovy_deleted, None);
    assert!(liked_deleted_at(&pool, "t2").await.unwrap().is_some());
}

#[tokio::test]
async fn reliked_track_is_reactivated() {
    let pool = common::create_test_db().await;

    merge_liked_items(&pool, &[item("t1", 1)], LIKED_PLAYLIST_ID)
        .await
        .unwrap();

    // Unlike everything → tombstone.
    let empty: HashSet<String> = HashSet::new();
    retire_missing_likes(&pool, LIKED_PLAYLIST_ID, &empty)
        .await
        .unwrap();
    assert!(liked_deleted_at(&pool, "t1").await.unwrap().is_some());

    // Relike → the same membership is reactivated with the new date.
    merge_liked_items(&pool, &[item("t1", 99)], LIKED_PLAYLIST_ID)
        .await
        .unwrap();

    assert_eq!(liked_deleted_at(&pool, "t1").await, Some(None));
    assert_eq!(liked_added_at(&pool, "t1").await, Some(99));
}
