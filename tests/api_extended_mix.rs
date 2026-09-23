//! Integration tests for the Extended-Mix auto-upgrade library scan
//! (`GET /api/extended-mix/candidates`), issue #29.

mod common;

use sqlx::Pool;
use sqlx::Sqlite;

/// Seed a release whose shorter version is owned but whose Extended Mix is
/// available and not owned.
async fn seed_upgrade_candidate(pool: &Pool<Sqlite>) {
    sqlx::query(
        r#"INSERT INTO service_tracks (id, service, service_id, title, artist, isrc, duration_ms)
           VALUES
             (100, 'spotify', 'spotify:track:radio',    'Rider (Radio Edit)',   'Pavel Khvaleev', 'US100', 200000),
             (101, 'spotify', 'spotify:track:extended', 'Rider (Extended Mix)', 'Pavel Khvaleev', 'US101', 400000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Owned shorter version: file links to track 100 via ISRC US100.
    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_hash, file_type, file_size, last_modified, isrc, title, artist)
           VALUES (1, '/test/rider-radio.flac', 'h1', 'flac', 1000, 1700000000, 'US100', 'Rider (Radio Edit)', 'Pavel Khvaleev')"#,
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn reports_candidate_when_shorter_owned_and_extended_available() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_upgrade_candidate(&pool).await;

    let resp = client
        .get(format!("{base}/api/extended-mix/candidates"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["data"]["enabled"], false); // default opt-in OFF

    let candidates = json["data"]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["releaseKey"], "rider");
    assert_eq!(candidates[0]["ownedTitle"], "Rider (Radio Edit)");
    assert_eq!(candidates[0]["extendedTitle"], "Rider (Extended Mix)");
    assert_eq!(candidates[0]["extendedTrackId"], 101);
    assert_eq!(candidates[0]["extendedService"], "spotify");
    assert_eq!(candidates[0]["extendedServiceId"], "spotify:track:extended");
    assert_eq!(candidates[0]["extendedDurationMs"], 400000);
}

#[tokio::test]
async fn no_candidate_when_extended_mix_already_owned() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_upgrade_candidate(&pool).await;

    // Also own the Extended Mix (file with ISRC US101).
    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_hash, file_type, file_size, last_modified, isrc, title, artist)
           VALUES (2, '/test/rider-extended.flac', 'h2', 'flac', 2000, 1700000000, 'US101', 'Rider (Extended Mix)', 'Pavel Khvaleev')"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let resp = client
        .get(format!("{base}/api/extended-mix/candidates"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["data"]["candidates"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn no_candidate_for_different_artist() {
    let (client, base, pool) = common::spawn_test_app().await;

    // Same base title, but the Extended Mix belongs to a different artist.
    sqlx::query(
        r#"INSERT INTO service_tracks (id, service, service_id, title, artist, isrc)
           VALUES
             (100, 'spotify', 'spotify:track:radio',    'Rider (Radio Edit)',   'Pavel Khvaleev', 'US100'),
             (101, 'spotify', 'spotify:track:extended', 'Rider (Extended Mix)', 'Another Artist',  'US101')"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_hash, file_type, file_size, last_modified, isrc, title, artist)
           VALUES (1, '/test/rider-radio.flac', 'h1', 'flac', 1000, 1700000000, 'US100', 'Rider (Radio Edit)', 'Pavel Khvaleev')"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let resp = client
        .get(format!("{base}/api/extended-mix/candidates"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["data"]["candidates"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn empty_library_returns_empty_candidates() {
    let (client, base, _pool) = common::spawn_test_app().await;

    let resp = client
        .get(format!("{base}/api/extended-mix/candidates"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["data"]["enabled"], false);
    assert_eq!(json["data"]["candidates"].as_array().unwrap().len(), 0);
}
