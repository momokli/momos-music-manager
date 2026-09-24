//! Shared test helpers for momos-music-manager integration tests.
//!
//! Every test creates a fresh in-memory SQLite DB, runs all migrations,
//! seeds hand-crafted data, builds the full Axum router (no background tasks),
//! and spawns it on a random port.
//!
//! # Usage
//!
//! ```ignore
//! mod common;
//!
//! #[tokio::test]
//! async fn files_filter_is_local_true() {
//!     let (client, base, pool) = common::spawn_test_app().await;
//!     common::seed_basic_data(&pool).await;
//!
//!     let resp = client.get(format!("{}/api/files?limit=5&isLocal=true", base))
//!         .send().await.unwrap();
//!     assert_eq!(resp.status(), 200);
//!
//!     let json: serde_json::Value = resp.json().await.unwrap();
//!     let files = json["data"].as_array().unwrap();
//!     for f in files {
//!         assert_eq!(f["isLocal"], true);
//!     }
//! }
//! ```

use std::sync::Arc;

use sqlx::{Pool, Sqlite, SqlitePool};

use momos_music_manager::AppState;
use momos_music_manager::config::ServiceCredentials;
use momos_music_manager::tasks::TaskManager;

// ═══════════════════════════════════════════════════════════════════════════
// Test App Factory
// ═══════════════════════════════════════════════════════════════════════════

/// Create an in-memory SQLite DB, run all migrations, return pool.
pub async fn create_test_db() -> Pool<Sqlite> {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&pool)
        .await
        .unwrap();
    run_migrations(&pool).await;
    // Migration 016 is a no-op (rename was applied manually on production DB).
    // Fresh DBs still have tags.followed and need the rename to tags.backpack.
    momos_music_manager::db::ensure_backpack_column(&pool)
        .await
        .unwrap();
    pool
}

/// Run all .sql files from `migrations/` in numeric order.
async fn run_migrations(pool: &Pool<Sqlite>) {
    let mut dir = tokio::fs::read_dir("migrations").await.unwrap();
    let mut files = Vec::new();
    while let Some(entry) = dir.next_entry().await.unwrap() {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "sql") {
            files.push(path);
        }
    }
    files.sort();

    for path in &files {
        let sql = tokio::fs::read_to_string(path).await.unwrap();
        let sql = sql.trim();
        if !sql.is_empty() {
            sqlx::query(sql).execute(pool).await.unwrap_or_else(|e| {
                panic!(
                    "Migration {} failed:\nError: {}\n\nFirst 500 chars:\n{}",
                    path.display(),
                    e,
                    &sql[..sql.len().min(500)]
                )
            });
        }
    }
}

/// Build a test AppState with no real credentials, no embeddings.
pub fn test_app_state(pool: Pool<Sqlite>) -> Arc<AppState> {
    Arc::new(AppState {
        db: pool,
        config: ServiceCredentials::defaults_for_test(),
        task_manager: TaskManager::new(),
        embeddings: tokio::sync::Mutex::new(None),
        category_means: tokio::sync::Mutex::new(None),
        public_url: None,
    })
}

/// Create a full test app (DB + migrations + router + running server).
/// Returns (reqwest Client, base URL, DB pool) for hitting endpoints and seeding.
pub async fn spawn_test_app() -> (reqwest::Client, String, Pool<Sqlite>) {
    use std::time::Duration;

    let pool = create_test_db().await;
    let state = test_app_state(pool.clone());
    let app = momos_music_manager::build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind test server");
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Poll until the server is ready (no fragile sleep)
    let client = reqwest::Client::new();
    for attempt in 0..50 {
        if client
            .get(format!("{base}/api/version"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return (client, base, pool);
        }
        if attempt == 0 {
            continue; // first attempt is instant
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    panic!("Test server at {base} did not start within 1 second");
}

// ═══════════════════════════════════════════════════════════════════════════
// Seed Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Insert basic seed data that most tests need.
/// Delegates to the shared `db::testing::seed_basic_scenario` function.
pub async fn seed_basic_data(pool: &Pool<Sqlite>) {
    momos_music_manager::db::testing::seed_basic_scenario(pool).await;
}

/// Seed data for the digging endpoint: tracks with BPM/key for suggestion testing.
/// Delegates to the shared `db::testing::seed_digging_scenario` function.
pub async fn seed_digging_data(pool: &Pool<Sqlite>) {
    momos_music_manager::db::testing::seed_digging_scenario(pool).await;
}

/// Seed WAV source data: links 5 WAV children to stem file (id=2) via `source_of`.
/// Delegates to the shared `db::testing::seed_wav_variant_scenario` function.
pub async fn seed_wav_variant_data(pool: &Pool<Sqlite>) {
    momos_music_manager::db::testing::seed_wav_variant_scenario(pool).await;
}

/// Seed tag hierarchy: a Setlist tag with Mood+Vibe parents, playlist, and file link.
///
/// Tag 10 "collapse-capital" (Setlist) -> parents: tag 11 "shadow" (Mood), tag 12 "techno" (Vibe)
/// Playlist 3 "collapse-capital" matches tag 10
/// File 1 (US001) linked via service_track 1 -> playlist 3 -> tag 10 -> resolves to parents 11+12
///
/// After calling this, call `refresh_file_resolved_tags()` to populate the materialised table.
/// Delegates to the shared `db::testing::seed_tag_hierarchy` function.
pub async fn seed_tag_hierarchy(pool: &Pool<Sqlite>) {
    momos_music_manager::db::testing::seed_tag_hierarchy(pool).await;
}

/// Seed files with explicit `comment` values for comment-status filter testing.
///
/// File 30: comment="[M] dark deep" -- target computed from resolved tags differs -> needs_update
/// File 31: comment="" (stored empty) -- no tags resolve -> target is empty -> up_to_date
/// File 32: comment=NULL -- no tags resolve -> target is empty -> NULL != "" -> needs_update
///
/// Needs `refresh_file_resolved_tags()` after seeding to populate the materialised table.
pub async fn seed_files_with_comments(pool: &Pool<Sqlite>) {
    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
             isrc, comment, file_hash, spotify_id)
           VALUES
             (30, '/test/stems/Comment - NeedsUpdate.flac', 'flac', 5000000, 1700000000,
              'CommentTest1', 'ArtistX', 'US030', '[M] dark deep', 'hash30', 'spotify:track:zzz'),
             (31, '/test/stems/Comment - UpToDate.flac',   'flac', 5000000, 1700000000,
              'CommentTest2', 'ArtistY', 'US031', '',              'hash31', NULL),
             (32, '/test/stems/Comment - NullComment.flac', 'flac', 5000000, 1700000000,
              'CommentTest3', 'ArtistZ', 'US032', NULL,            'hash32', NULL)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // File 30 gets linked to a service_track and playlist with a tag so it resolves
    sqlx::query(
        r#"INSERT INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
           VALUES (4, 'spotify', 'spotify:track:zzz', 'CommentTest1', 'ArtistX', 'US030', 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Reuse Groovy playlist (id=1) -- link track 4 to it so file 30 gets tag "Groovy"
    sqlx::query(
        r#"INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES (1, 4, 0, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Back up all 3 comment test files
    for (id, path) in [
        (30, "/backup/stems/Comment - NeedsUpdate.flac"),
        (31, "/backup/stems/Comment - UpToDate.flac"),
        (32, "/backup/stems/Comment - NullComment.flac"),
    ] {
        sqlx::query(
            r#"INSERT INTO file_locations (file_id, location_type, path, file_size)
               VALUES (?, 'backup', ?, 5000000)"#,
        )
        .bind(id)
        .bind(path)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Seed a subscribed playlist with `archive_deleted=true`.
///
/// Playlist 3 (collapse-capital) gets a subscription row and archive_deleted=true.
/// Use AFTER `seed_tag_hierarchy()` (which creates playlist 3) or standalone.
pub async fn seed_subscribed_playlist(pool: &Pool<Sqlite>) {
    // Mark playlist 3 as archived
    sqlx::query("UPDATE service_playlists SET archive_deleted = 1 WHERE id = 3")
        .execute(pool)
        .await
        .unwrap();

    // Add subscription
    sqlx::query(
        r#"INSERT INTO playlist_subscriptions (service, playlist_id, service_playlist_id)
           VALUES ('spotify', 'spotify:playlist:333', 3)"#,
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Seed a service_config row for the given service.
///
/// Use this to pre-configure a service before testing config read/update endpoints.
/// Default stored values: user_id="test_user", playlist_id="test_playlist".
pub async fn seed_service_config(pool: &Pool<Sqlite>, service: &str) {
    let now = 1700000000;
    sqlx::query(
        r#"INSERT OR REPLACE INTO service_config (service, user_id, playlist_id, is_connected, created_at, updated_at)
           VALUES (?, 'test_user', 'test_playlist', 0, ?, ?)"#,
    )
    .bind(service)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
}

/// Seed data for dynamic bundle testing.
///
/// Extends seed_basic_data with:
/// - Tags: hammahalle (id=50, Mood), spät (id=51, Vibe), bouncy (id=52, Vibe)
/// - Files with varying BPM: id=60 (120 flac), id=61 (140 stem.m4a), id=62 (155 stem.m4a), id=63 (180 flac)
/// - Playlists matching tag names so file_resolved_tags gets populated
/// - Service tracks + playlist links: 61→hammahalle, 62→spät, 63→bouncy
///
/// Delegates to the shared `db::testing::seed_dynamic_bundles_scenario` function.
pub async fn seed_dynamic_bundles_data(pool: &Pool<Sqlite>) {
    momos_music_manager::db::testing::seed_dynamic_bundles_scenario(pool).await;
}

/// Seed data for laboratory analysis testing.
///
/// Extends seed_basic_data with:
/// - File 5: needs analysis (no BPM, no key), IS local, IS backed up, linked to tag "Laboratory"
/// - Tag 20: "Laboratory" (Setlist category)
/// - Playlist 4: "Laboratory" matching tag
pub async fn seed_lab_scenario(pool: &Pool<Sqlite>) {
    seed_basic_data(pool).await;

    // File 5: needs analysis (no BPM, no key), IS local, IS backed up
    sqlx::query(
        r#"INSERT OR IGNORE INTO files (id, file_path, file_type, file_size, last_modified, title, artist, isrc, file_hash)
           VALUES (5, '/test/stems/Needs - Analysis.flac', 'flac', 5000000, 1700000000, 'Needy Track', 'Test Artist', 'US005', 'hash5')"#
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT OR IGNORE INTO file_locations (file_id, location_type, path, file_size, last_verified)
           VALUES (5, 'local', '/test/stems/Needs - Analysis.flac', 5000000, 1700000000),
                  (5, 'backup', '/backup/stems/Needs - Analysis.flac', 5000000, 1700000000)"#
    )
    .execute(pool)
    .await
    .unwrap();

    // Tag "Laboratory" (id=20, Setlist category=1)
    sqlx::query(
        r#"INSERT OR IGNORE INTO tags (id, name, category_id) VALUES (20, 'Laboratory', 1)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Playlist matching tag name
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlists (id, service, playlist_id, name, snapshot_id)
           VALUES (4, 'spotify', 'spotify:playlist:444', 'Laboratory', 'snap4')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Link file 5 to the Laboratory playlist via spotify_id
    sqlx::query(r#"UPDATE files SET spotify_id = 'spotify:track:eee' WHERE id = 5"#)
        .execute(pool)
        .await
        .unwrap();

    sqlx::query(
        r#"INSERT OR IGNORE INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
           VALUES (5, 'spotify', 'spotify:track:eee', 'Needy Track', 'Test Artist', 'US005', 1700000000)"#
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES (4, 5, 0, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Also add local presence for file 5
    sqlx::query("UPDATE files SET last_verified_local = 1700000000 WHERE id = 5")
        .execute(pool)
        .await
        .unwrap();

    momos_music_manager::db::refresh_file_resolved_tags(pool)
        .await
        .unwrap();
}
