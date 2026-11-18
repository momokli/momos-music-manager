//! Loader test for #501: `desired_from_playlist` reads the hub DB read-only,
//! in playlist order. No network, in-memory SQLite.

use music_hub::tonies::sync::desired_from_playlist;

async fn insert_user(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_users (slug, display_name, created_at)
         VALUES ('alice', 'Alice', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_playlist(pool: &sqlx::SqlitePool, user_id: i64, remote: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, fetched_at)
         VALUES (?1, 'spotify', ?2, 'Mix', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(user_id)
    .bind(remote)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_track(
    pool: &sqlx::SqlitePool,
    sid: &str,
    isrc: Option<&str>,
    title: &str,
    duration_ms: Option<i64>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, isrc, title, duration_ms, first_seen_at)
         VALUES ('spotify', ?1, ?2, ?3, ?4, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(sid)
    .bind(isrc)
    .bind(title)
    .bind(duration_ms)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn link(pool: &sqlx::SqlitePool, playlist_id: i64, track_id: i64, position: i64) {
    sqlx::query(
        "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position)
         VALUES (?1, ?2, ?3)",
    )
    .bind(playlist_id)
    .bind(track_id)
    .bind(position)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn desired_from_playlist_follows_position_and_filters_playlist() {
    let pool = music_hub::db::connect("sqlite::memory:").await.unwrap();
    let user = insert_user(&pool).await;
    let target = insert_playlist(&pool, user, "target").await;
    let other = insert_playlist(&pool, user, "other").await;

    let first = insert_track(&pool, "s1", Some("ISRC1"), "First", Some(180_000)).await;
    let second = insert_track(&pool, "s2", None, "Second", None).await;
    let third = insert_track(&pool, "s3", Some("ISRC3"), "Third", Some(60_500)).await;

    // Insert memberships out of order: positions define the result order.
    link(&pool, target, second, 2).await;
    link(&pool, target, third, 1).await;
    link(&pool, target, first, 0).await;
    // Must not leak in from another playlist.
    link(&pool, other, first, 0).await;

    let desired = desired_from_playlist(&pool, target, "playlist").await.unwrap();

    assert_eq!(desired.len(), 3);
    assert_eq!(desired[0].title, "First");
    assert_eq!(desired[0].isrc.as_deref(), Some("ISRC1"));
    assert_eq!(desired[0].duration_ms, 180_000);
    assert_eq!(desired[1].title, "Third");
    assert_eq!(desired[1].isrc.as_deref(), Some("ISRC3"));
    assert_eq!(desired[1].duration_ms, 60_500);
    assert_eq!(desired[2].title, "Second");
    assert_eq!(desired[2].isrc, None);
    assert_eq!(desired[2].duration_ms, 0);
}

#[tokio::test]
async fn desired_from_missing_playlist_is_empty() {
    let pool = music_hub::db::connect("sqlite::memory:").await.unwrap();
    let desired = desired_from_playlist(&pool, 42, "playlist").await.unwrap();
    assert!(desired.is_empty());
}
