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

/// Insert basic seed data that most tests need:
/// - 2 tag categories (Setlist, Mood)
/// - 3 tags (Groovy, Deep, Dark)
/// - 3 files with various properties
/// - File location entries (local + backup variants)
/// - 1 folder
/// - Service tracks + playlists + playlist_tracks for tag resolution
pub async fn seed_basic_data(pool: &Pool<Sqlite>) {
    // ── Tag categories
    //    Migration 001 already seeds: Setlist(id=1), Phase(id=2), Mood(id=3),
    //    Vibe(id=4), Merkmal(id=5). We reference those IDs — no re-insertion.

    // ── Tags (all in Mood category, id=3 from migration 001)
    //    Migration 001 already inserts 6 phase tags (start, build, peak, release,
    //    sustain, end) which auto-assign IDs 1–6. Use IDs 7+ to avoid conflict.
    sqlx::query(
        r#"INSERT INTO tags (id, name, category_id, backpack)
           VALUES (7, 'Groovy', 3, 0),
                  (8, 'Deep', 3, 1),
                  (9, 'Dark', 3, 0)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Folder
    sqlx::query(
        r#"INSERT INTO folders (id, folder_path, scan_recursive, active)
           VALUES (1, '/test/stems', 1, 1)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Files
    //    File 1: FLAC, BPM=128.0, key=4m, ISRC=US001, backed up + local
    //    File 2: stem.m4a, BPM=128.5, key=4m, ISRC=US001 (same track, stem variant)
    //    File 3: FLAC, BPM=140.0, key=8m, ISRC=US002, backed up only (not local)
    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
             bpm, musical_key, isrc, rating, play_count, last_played,
             duration_ms, file_hash, spotify_id)
           VALUES
             (1, '/test/stems/Artist - Title.flac',    'flac',    5000000, 1700000000, 'Title One',   'Artist A', 128.0, '4m', 'US001', 4, 10, 1700000000, 300000, 'hash1', 'spotify:track:aaa'),
             (2, '/test/stems/Artist - Title.stem.m4a', 'stem.m4a', 8000000, 1700000000, 'Title One',  'Artist A', 128.5, '4m', 'US001', 4, 10, 1700000000, 300000, 'hash2', 'spotify:track:aaa'),
             (3, '/test/stems/Other - Track.flac',     'flac',    6000000, 1700000000, 'Track Two',   'Artist B', 140.0, '8m', 'US002', 2,  3, 1690000000, 240000, 'hash3', 'spotify:track:bbb'),
             (4, '/test/stems/Unlinked - Song.flac',   'flac',    4000000, 1700000000, 'Unlinked',    'Orphan',  NULL, NULL, 'US999', 0,  0, NULL,      180000, 'hash4', NULL)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── File locations
    sqlx::query(
        r#"INSERT INTO file_locations (file_id, location_type, path, file_size, last_verified)
           VALUES
             (1, 'local',  '/test/stems/Artist - Title.flac',       5000000, 1700000000),
             (1, 'backup', '/backup/stems/Artist - Title.flac',     5000000, 1700000000),
             (2, 'local',  '/test/stems/Artist - Title.stem.m4a',   8000000, 1700000000),
             (2, 'backup', '/backup/stems/Artist - Title.stem.m4a', 8000000, 1700000000),
             (3, 'backup', '/backup/stems/Other - Track.flac',      6000000, 1700000000),
             (4, 'backup', '/backup/stems/Unlinked - Song.flac',     4000000, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Update last_verified_local on files 1 and 2
    sqlx::query("UPDATE files SET last_verified_local = 1700000000 WHERE id IN (1, 2)")
        .execute(pool)
        .await
        .unwrap();

    // ── Service tracks
    sqlx::query(
        r#"INSERT INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
           VALUES
             (1, 'spotify', 'spotify:track:aaa', 'Title One',  'Artist A', 'US001', 1700000000),
             (2, 'spotify', 'spotify:track:bbb', 'Track Two',  'Artist B', 'US002', 1700000000),
             (3, 'spotify', 'spotify:track:ccc', 'Orphan Demo','Artist C', 'US003', 1690000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Service playlists
    sqlx::query(
        r#"INSERT INTO service_playlists (id, service, playlist_id, name, snapshot_id)
           VALUES
             (1, 'spotify', 'spotify:playlist:111', 'Groovy',   'snap1'),
             (2, 'spotify', 'spotify:playlist:222', 'Deep Mix', 'snap2')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Service playlist tracks (track→playlist linking for tag resolution)
    sqlx::query(
        r#"INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES
             (1, 1, 0, 1700000000),
             (2, 2, 0, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Seed data for the digging endpoint: tracks with BPM/key for suggestion testing.
pub async fn seed_digging_data(pool: &Pool<Sqlite>) {
    for (i, (isrc, title, artist, bpm, key)) in [
        ("US100", "Games People Play", "Paula van Klar", 140.0, "3m"),
        ("US101", "The Void", "Maite Dedecker", 141.0, "8m"),
        ("US102", "This Summer", "Anna Reusch", 140.0, "6m"),
        // Outlier: 160 BPM — should be excluded from suggestions
        ("US103", "Mean One", "Elon Bass", 160.0, "1m"),
    ]
    .into_iter()
    .enumerate()
    {
        let file_id = 10 + i as i64;
        sqlx::query(
            r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
                 bpm, musical_key, isrc, file_hash)
               VALUES (?, ?, 'flac', 5000000, 1700000000, ?, ?, ?, ?, ?, 'dig-hash')"#,
        )
        .bind(file_id)
        .bind(format!("/test/stems/{}.flac", title))
        .bind(title)
        .bind(artist)
        .bind(bpm)
        .bind(key)
        .bind(isrc)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            r#"INSERT INTO file_locations (file_id, location_type, path, file_size)
               VALUES (?, 'local', ?, 5000000)"#,
        )
        .bind(file_id)
        .bind(format!("/test/stems/{}.flac", title))
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Seed WAV source data: links 5 WAV children to stem file (id=2) via `source_of`.
pub async fn seed_wav_variant_data(pool: &Pool<Sqlite>) {
    for (i, stem_type) in ["vocals", "bass", "drums", "instrumental", "other"]
        .into_iter()
        .enumerate()
    {
        let wav_id = 20 + i as i64;
        sqlx::query(
            r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
                 isrc, source_of, stem_type, file_hash)
               VALUES (?, ?, 'wav', 2000000, 1700000000, 'Title One', 'Artist A', 'US001', 2, ?, 'wav-hash')"#,
        )
        .bind(wav_id)
        .bind(format!(
            "/test/stems/Artist_Title/Artist - Title_{}.wav",
            stem_type
        ))
        .bind(stem_type)
        .execute(pool)
        .await
        .unwrap();

        // WAVs are backed up but not local
        sqlx::query(
            r#"INSERT INTO file_locations (file_id, location_type, path, file_size)
               VALUES (?, 'backup', ?, 2000000)"#,
        )
        .bind(wav_id)
        .bind(format!(
            "/backup/stems/Artist_Title/Artist - Title_{}.wav",
            stem_type
        ))
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Seed tag hierarchy: a Setlist tag with Mood+Vibe parents, playlist, and file link.
///
/// Tag 10 "collapse-capital" (Setlist) -> parents: tag 11 "shadow" (Mood), tag 12 "techno" (Vibe)
/// Playlist 3 "collapse-capital" matches tag 10
/// File 1 (US001) linked via service_track 1 -> playlist 3 -> tag 10 -> resolves to parents 11+12
///
/// NOTE: parent name "shadow" avoids collision with tag 9 "Dark" from seed_basic_data
///       (tags.name has UNIQUE COLLATE NOCASE).
///
/// After calling this, call `refresh_file_resolved_tags()` to populate the materialised table.
pub async fn seed_tag_hierarchy(pool: &Pool<Sqlite>) {
    // -- Parent tags (Phase + Mood + Vibe) --
    // Use "shadow" to avoid UNIQUE COLLATE NOCASE collision with tag 9 "Dark" from seed_basic_data
    sqlx::query(
        r#"INSERT INTO tags (id, name, category_id, backpack)
           VALUES (11, 'shadow', 3, 0),
                  (12, 'techno', 4, 0),
                  (13, 'driving', 2, 0)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // -- Setlist tag (child) --
    sqlx::query(
        r#"INSERT INTO tags (id, name, category_id, backpack)
           VALUES (10, 'collapse-capital', 1, 0)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // -- Parent relationships --
    sqlx::query(
        r#"INSERT INTO tag_parents (tag_id, parent_tag_id)
           VALUES (10, 11),
                  (10, 12),
                  (10, 13)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // -- Playlist matching the Setlist tag name --
    sqlx::query(
        r#"INSERT INTO service_playlists (id, service, playlist_id, name)
           VALUES (3, 'spotify', 'spotify:playlist:333', 'collapse-capital')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // -- Link existing track 1 (US001) to playlist 3 --
    sqlx::query(
        r#"INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES (3, 1, 0, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();
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
