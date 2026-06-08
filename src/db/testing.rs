//! Seed data functions for testing — used by both Rust integration tests
//! (via `tests/common/mod.rs`) and the Playwright E2E test seed endpoint
//! (`POST /api/testing/seed`).
//!
//! Every function takes a `&Pool<Sqlite>` and uses `unwrap()` pervasively —
//! these are test utilities and panics are acceptable.

use std::collections::HashMap;

use sqlx::{Pool, Sqlite};

/// Clear all user data from every table, preserving migration 001 defaults
/// (tag_categories id 1-5, tags id 1-6).
pub async fn clear_all_tables(pool: &Pool<Sqlite>) {
    // Delete in reverse FK order (children before parents)
    let tables = [
        "file_resolved_tags",
        "track_resolved_tags",
        "file_locations",
        "service_playlist_tracks",
        "playlist_subscriptions",
        "tag_parents",
        "tag_similarities",
        "tag_energy_levels",
        "tag_embeddings",
        "tag_bundles",
        "deemix_downloads",
        "service_tracks",
        "service_playlists",
        "files",
        "folders",
        "service_config",
    ];
    for table in &tables {
        sqlx::query(&format!("DELETE FROM {}", table))
            .execute(pool)
            .await
            .unwrap();
    }
    // Clear user-created tags (keep migration 001's phase tags: 1-6)
    sqlx::query("DELETE FROM tags WHERE id > 6")
        .execute(pool)
        .await
        .unwrap();
    // Clear user-created tag categories (keep migration 001 defaults: 1-5)
    sqlx::query("DELETE FROM tag_categories WHERE id > 5")
        .execute(pool)
        .await
        .unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════
// Seed Scenarios
// ═══════════════════════════════════════════════════════════════════════════

/// Basic seed data: tags, files, locations, service tracks/playlists,
/// and tag resolution chain. All INSERTs use OR IGNORE so the function
/// is idempotent — safe to call after clear_all_tables or after other seed functions.
pub async fn seed_basic_scenario(pool: &Pool<Sqlite>) -> HashMap<String, usize> {
    // ── Tags (in Mood category id=3, avoid IDs 1-6 which are Phase tags)
    sqlx::query(
        r#"INSERT OR IGNORE INTO tags (id, name, category_id, backpack)
           VALUES (7, 'Groovy', 3, 0),
                  (8, 'Deep', 3, 1),
                  (9, 'Dark', 3, 0)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Folder
    sqlx::query(
        r#"INSERT OR IGNORE INTO folders (id, folder_path, scan_recursive, active)
           VALUES (1, '/test/stems', 1, 1)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Files (4 rows: 2 with ISRC US001, 1 with US002, 1 unlinked)
    sqlx::query(
        r#"INSERT OR IGNORE INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
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

    // ── File locations (local + backup)
    sqlx::query(
        r#"INSERT OR IGNORE INTO file_locations (file_id, location_type, path, file_size, last_verified)
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

    // ── last_verified_local on local files
    sqlx::query("UPDATE files SET last_verified_local = 1700000000 WHERE id IN (1, 2)")
        .execute(pool)
        .await
        .unwrap();

    // ── Service tracks (3 rows)
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
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
        r#"INSERT OR IGNORE INTO service_playlists (id, service, playlist_id, name, snapshot_id)
           VALUES
             (1, 'spotify', 'spotify:playlist:111', 'Groovy',   'snap1'),
             (2, 'spotify', 'spotify:playlist:222', 'Deep Mix', 'snap2')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Service playlist tracks (track→playlist linking for tag resolution)
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES
             (1, 1, 0, 1700000000),
             (2, 2, 0, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    let mut counts = HashMap::new();
    counts.insert("tags".into(), 3);
    counts.insert("files".into(), 4);
    counts.insert("file_locations".into(), 6);
    counts.insert("service_tracks".into(), 3);
    counts.insert("service_playlists".into(), 2);
    counts.insert("service_playlist_tracks".into(), 2);
    counts.insert("folders".into(), 1);
    counts
}

/// Extended seed data for testing file filters: adds files with comments,
/// PMV tag resolution, and more file type variety.
pub async fn seed_files_filter_scenario(pool: &Pool<Sqlite>) -> HashMap<String, usize> {
    let mut counts = seed_basic_scenario(pool).await;

    // Add comment-status test files (id 30-32)
    sqlx::query(
        r#"INSERT OR IGNORE INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
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

    // Link file 30 to a service_track → Groovy playlist for tag resolution
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
           VALUES (4, 'spotify', 'spotify:track:zzz', 'CommentTest1', 'ArtistX', 'US030', 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
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
            r#"INSERT OR IGNORE INTO file_locations (file_id, location_type, path, file_size)
               VALUES (?, 'backup', ?, 5000000)"#,
        )
        .bind(id)
        .bind(path)
        .execute(pool)
        .await
        .unwrap();
    }

    // Add PMV tag hierarchy: Setlist tag → parent Mood+Vibe tags
    seed_tag_hierarchy(pool).await;

    // Refresh materialized tag table
    crate::db::refresh_file_resolved_tags(pool).await.unwrap();

    *counts.get_mut("files").unwrap() += 3;
    *counts.get_mut("service_tracks").unwrap() += 1;
    *counts.get_mut("service_playlist_tracks").unwrap() += 1;
    *counts.get_mut("file_locations").unwrap() += 3;
    counts.insert("tags".into(), 6); // 3 basic + 3 from tag_hierarchy
    counts
}

/// Seed tag hierarchy: Setlist tag with Mood+Vibe+Phase parents.
/// Creates tag 10 "collapse-capital" (Setlist) with parents 11 (shadow/Mood),
/// 12 (techno/Vibe), 13 (driving/Phase). Creates playlist matching tag name,
/// links existing track 1 to it.
pub async fn seed_tag_hierarchy(pool: &Pool<Sqlite>) {
    sqlx::query(
        r#"INSERT OR IGNORE INTO tags (id, name, category_id, backpack)
           VALUES (11, 'shadow', 3, 0),
                  (12, 'techno', 4, 0),
                  (13, 'driving', 2, 0)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT OR IGNORE INTO tags (id, name, category_id, backpack)
           VALUES (10, 'collapse-capital', 1, 0)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT OR IGNORE INTO tag_parents (tag_id, parent_tag_id)
           VALUES (10, 11), (10, 12), (10, 13)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlists (id, service, playlist_id, name)
           VALUES (3, 'spotify', 'spotify:playlist:333', 'collapse-capital')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES (3, 1, 0, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Seed data for digging/suggestion testing: tracks with BPM/key.
pub async fn seed_digging_scenario(pool: &Pool<Sqlite>) -> HashMap<String, usize> {
    let mut counts = seed_basic_scenario(pool).await;

    for (i, (isrc, title, artist, bpm, key)) in [
        ("US100", "Games People Play", "Paula van Klar", 140.0, "3m"),
        ("US101", "The Void", "Maite Dedecker", 141.0, "8m"),
        ("US102", "This Summer", "Anna Reusch", 140.0, "6m"),
        ("US103", "Mean One", "Elon Bass", 160.0, "1m"),
    ]
    .into_iter()
    .enumerate()
    {
        let file_id = 10 + i as i64;
        sqlx::query(
            r#"INSERT OR IGNORE INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
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
            r#"INSERT OR IGNORE INTO file_locations (file_id, location_type, path, file_size)
               VALUES (?, 'local', ?, 5000000)"#,
        )
        .bind(file_id)
        .bind(format!("/test/stems/{}.flac", title))
        .execute(pool)
        .await
        .unwrap();
    }

    // Create a tag for the seed files so seed_tag works in digging
    sqlx::query(
        r#"INSERT OR IGNORE INTO tags (id, name, category_id, backpack)
           VALUES (14, 'Collapse-capital', 1, 0)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Create playlist matching the tag, link digging files to it
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlists (id, service, playlist_id, name)
           VALUES (4, 'spotify', 'spotify:playlist:444', 'Collapse-capital')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Link files 10-12 (the non-outlier ones) to the playlist via service_tracks
    for (i, isrc) in ["US100", "US101", "US102"].into_iter().enumerate() {
        let track_id = 10 + i as i64;
        let file_id = 10 + i as i64;
        sqlx::query(
            r#"INSERT OR IGNORE INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
               VALUES (?, 'spotify', ?, ?, ?, ?, 1700000000)"#,
        )
        .bind(track_id)
        .bind(format!("spotify:track:dig{:03}", i))
        .bind(match i { 0 => "Games People Play", 1 => "The Void", _ => "This Summer" })
        .bind(match i { 0 => "Paula van Klar", 1 => "Maite Dedecker", _ => "Anna Reusch" })
        .bind(isrc)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
               VALUES (4, ?, 0, 1700000000)"#,
        )
        .bind(track_id)
        .execute(pool)
        .await
        .unwrap();

        // Link file to track via v_file_track_link (ISRC match)
        sqlx::query("UPDATE files SET spotify_id = ? WHERE id = ?")
            .bind(format!("spotify:track:dig{:03}", i))
            .bind(file_id)
            .execute(pool)
            .await
            .unwrap();
    }

    crate::db::refresh_file_resolved_tags(pool).await.unwrap();

    *counts.get_mut("files").unwrap() += 4;
    *counts.get_mut("file_locations").unwrap() += 4;
    *counts.get_mut("service_tracks").unwrap() += 3;
    *counts.get_mut("service_playlists").unwrap() += 1;
    *counts.get_mut("service_playlist_tracks").unwrap() += 3;
    *counts.get_mut("tags").unwrap() += 1;
    counts
}

/// Seed WAV source data: 5 WAV children linked to stem file id=2 via source_of.
pub async fn seed_wav_variant_scenario(pool: &Pool<Sqlite>) -> HashMap<String, usize> {
    let mut counts = seed_basic_scenario(pool).await;

    for (i, stem_type) in ["vocals", "bass", "drums", "instrumental", "other"]
        .into_iter()
        .enumerate()
    {
        let wav_id = 20 + i as i64;
        sqlx::query(
            r#"INSERT OR IGNORE INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
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

        sqlx::query(
            r#"INSERT OR IGNORE INTO file_locations (file_id, location_type, path, file_size)
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

    *counts.get_mut("files").unwrap() += 5;
    *counts.get_mut("file_locations").unwrap() += 5;
    counts
}

/// Seed data for comment diff testing. Creates two files:
/// - File 40: comment differs from target → needsUpdate=true (local, backed up)
/// - File 41: comment matches target → needsUpdate=false (local, backed up)
/// Both are linked to playlist "Groovy" → tag "Groovy" (Mood/M).
pub async fn seed_comment_diff_scenario(pool: &Pool<Sqlite>) -> HashMap<String, usize> {
    let mut counts = seed_basic_scenario(pool).await;

    // Files 40, 41 with explicit comments
    sqlx::query(
        r#"INSERT OR IGNORE INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
             isrc, comment, file_hash, spotify_id)
           VALUES
             (40, '/test/stems/Diff - NeedsUpdate.flac', 'flac', 5000000, 1700000000,
              'DiffTest Needs', 'Artist Diff', 'US040', 'old wrong comment', 'hash40', 'spotify:track:diff1'),
             (41, '/test/stems/Diff - UpToDate.flac',   'flac', 5000000, 1700000000,
              'DiffTest OK',    'Artist Diff', 'US041', 'groovy',           'hash41', 'spotify:track:diff2')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Both are local + backed up
    sqlx::query(
        r#"INSERT OR IGNORE INTO file_locations (file_id, location_type, path, file_size, last_verified)
           VALUES
             (40, 'local',  '/test/stems/Diff - NeedsUpdate.flac', 5000000, 1700000000),
             (40, 'backup', '/backup/stems/Diff - NeedsUpdate.flac', 5000000, 1700000000),
             (41, 'local',  '/test/stems/Diff - UpToDate.flac',   5000000, 1700000000),
             (41, 'backup', '/backup/stems/Diff - UpToDate.flac', 5000000, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Service tracks for linking
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
           VALUES
             (20, 'spotify', 'spotify:track:diff1', 'DiffTest Needs', 'Artist Diff', 'US040', 1700000000),
             (21, 'spotify', 'spotify:track:diff2', 'DiffTest OK',    'Artist Diff', 'US041', 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Link to playlist "Groovy" (id=1) → tag "Groovy" (Mood, prefix M)
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES (1, 20, 0, 1700000000), (1, 21, 0, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Populate file_resolved_tags so target comment can be computed
    crate::db::refresh_file_resolved_tags(pool).await.unwrap();

    *counts.get_mut("files").unwrap() += 2;
    *counts.get_mut("file_locations").unwrap() += 4;
    *counts.get_mut("service_tracks").unwrap() += 2;
    counts
}

/// Seed a subscribed playlist for archive/subscription testing.
pub async fn seed_subscribed_playlist(pool: &Pool<Sqlite>) {
    sqlx::query("UPDATE service_playlists SET archive_deleted = 1 WHERE id = 1")
        .execute(pool)
        .await
        .unwrap();

    sqlx::query(
        r#"INSERT OR IGNORE INTO playlist_subscriptions (service, playlist_id) VALUES ('spotify', 'spotify:playlist:111')"#,
    )
    .execute(pool)
    .await
    .unwrap();
}
