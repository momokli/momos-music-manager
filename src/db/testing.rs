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
        "rediscovery_pushes",
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

/// Seed data for dynamic bundle testing. Extends seed_basic_scenario with:
/// - Files at different BPMs (120, 140, 155, 180)
/// - Tags "hammahalle" (Mood), "spät" (Vibe), "bouncy" (Vibe)
/// - Playlists matching tag names for file_resolved_tags resolution
/// - Links: file 61 → hammahalle, file 62 → spät, file 63 → bouncy
pub async fn seed_dynamic_bundles_scenario(pool: &Pool<Sqlite>) -> HashMap<String, usize> {
    let mut counts = seed_basic_scenario(pool).await;

    // Tags: hammahalle (Mood=3), spät (Vibe=4), bouncy (Vibe=4)
    sqlx::query(
        r#"INSERT OR IGNORE INTO tags (id, name, category_id, backpack)
           VALUES
             (50, 'hammahalle', 3, 0),
             (51, 'spät', 4, 0),
             (52, 'bouncy', 4, 0)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Files with different BPMs
    // id=60: 120 BPM flac, id=61: 140 BPM stem.m4a, id=62: 155 BPM stem.m4a, id=63: 180 BPM flac
    sqlx::query(
        r#"INSERT OR IGNORE INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
             bpm, musical_key, isrc, file_hash, spotify_id)
           VALUES
             (60, '/test/stems/BPM120 - Track.flac',    'flac',    5000000, 1700000000, 'BPM120',  'Artist120', 120.0, '4m', 'US060', 'hash60', 'spotify:track:bpm120'),
             (61, '/test/stems/BPM140 - Track.stem.m4a', 'stem.m4a', 8000000, 1700000000, 'BPM140',  'Artist140', 140.0, '6m', 'US061', 'hash61', 'spotify:track:bpm140'),
             (62, '/test/stems/BPM155 - Track.stem.m4a', 'stem.m4a', 8000000, 1700000000, 'BPM155',  'Artist155', 155.0, '7m', 'US062', 'hash62', 'spotify:track:bpm155'),
             (63, '/test/stems/BPM180 - Track.flac',    'flac',    5000000, 1700000000, 'BPM180',  'Artist180', 180.0, '9m', 'US063', 'hash63', 'spotify:track:bpm180')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // File locations (local + backup) for each
    sqlx::query(
        r#"INSERT OR IGNORE INTO file_locations (file_id, location_type, path, file_size, last_verified)
           VALUES
             (60, 'local',  '/test/stems/BPM120 - Track.flac',        5000000, 1700000000),
             (60, 'backup', '/backup/stems/BPM120 - Track.flac',      5000000, 1700000000),
             (61, 'local',  '/test/stems/BPM140 - Track.stem.m4a',    8000000, 1700000000),
             (61, 'backup', '/backup/stems/BPM140 - Track.stem.m4a',  8000000, 1700000000),
             (62, 'local',  '/test/stems/BPM155 - Track.stem.m4a',    8000000, 1700000000),
             (62, 'backup', '/backup/stems/BPM155 - Track.stem.m4a',  8000000, 1700000000),
             (63, 'local',  '/test/stems/BPM180 - Track.flac',        5000000, 1700000000),
             (63, 'backup', '/backup/stems/BPM180 - Track.flac',      5000000, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query("UPDATE files SET last_verified_local = 1700000000 WHERE id IN (60, 61, 62, 63)")
        .execute(pool)
        .await
        .unwrap();

    // Service tracks for linking
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
           VALUES
             (60, 'spotify', 'spotify:track:bpm120', 'BPM120', 'Artist120', 'US060', 1700000000),
             (61, 'spotify', 'spotify:track:bpm140', 'BPM140', 'Artist140', 'US061', 1700000000),
             (62, 'spotify', 'spotify:track:bpm155', 'BPM155', 'Artist155', 'US062', 1700000000),
             (63, 'spotify', 'spotify:track:bpm180', 'BPM180', 'Artist180', 'US063', 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Playlists matching tag names for tag resolution
    // id=3: hammahalle, id=4: spät, id=5: bouncy
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlists (id, service, playlist_id, name)
           VALUES
             (3, 'spotify', 'spotify:playlist:hammahalle', 'hammahalle'),
             (4, 'spotify', 'spotify:playlist:spat',        'spät'),
             (5, 'spotify', 'spotify:playlist:bouncy',      'bouncy')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Link tracks to playlists: file 61 → hammahalle, file 62 → spät, file 63 → bouncy
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES
             (3, 61, 0, 1700000000),
             (4, 62, 0, 1700000000),
             (5, 63, 0, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Populate file_resolved_tags so tag resolution works
    crate::db::refresh_file_resolved_tags(pool).await.unwrap();

    *counts.get_mut("tags").unwrap() += 3;
    *counts.get_mut("files").unwrap() += 4;
    *counts.get_mut("file_locations").unwrap() += 8;
    *counts.get_mut("service_tracks").unwrap() += 4;
    *counts.get_mut("service_playlists").unwrap() += 3;
    *counts.get_mut("service_playlist_tracks").unwrap() += 3;
    counts
}

/// Seed data for the `liked_songs` testing scenario (Issue #58).
///
/// Extends `seed_basic_scenario` with the playlists and track links needed to
/// exercise the liked/generated semantics of migration 032:
/// - Playlist 5 `liked` (`playlist_kind='liked'`, `playlist_id='spotify:liked'`)
/// - Playlist 6 `Today's Selection` (`playlist_kind='generated'`) — never counts/tags
/// - Playlist 7 `Likes` (`playlist_kind='liked'`) — name-variant mirror
///
/// Track links are reconciled against the two rows `seed_basic_scenario` creates:
/// Track 1 in Playlist 5 (`added_at=1500000000`) and Playlist 1 (`added_at=1600000000`),
/// Track 2 only in Playlist 5 (`added_at=1400000000`), Track 3 in Playlists 1+2
/// (`added_at=1700000000`). So against `v_track_forgotten_facts`: Track 1 →
/// `playlist_count=1`/`last_touched_at=1600000000`/liked, Track 2 → `0`/`1400000000`/liked,
/// Track 3 → `2`/`1700000000`/not liked. All INSERTs are `OR IGNORE` (idempotent).
pub async fn seed_liked_songs_scenario(pool: &Pool<Sqlite>) -> HashMap<String, usize> {
    let mut counts = seed_basic_scenario(pool).await;

    // ── Playlists 5-7: likes mirror, generated daily, name-variant mirror
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlists (id, service, playlist_id, name, playlist_kind)
           VALUES
             (5, 'spotify', 'spotify:liked',            'liked',              'liked'),
             (6, 'spotify', 'spotify:playlist:today',   'Today''s Selection', 'generated'),
             (7, 'spotify', 'spotify:playlist:likes',   'Likes',              'liked')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Reconcile the two rows seed_basic_scenario created:
    //    (1,1) becomes an old like (added_at=1600000000) and (2,2) must go away so
    //    Track 2 ends up in no curated playlist (playlist_count=0).
    sqlx::query(
        "UPDATE service_playlist_tracks SET added_at = 1600000000 WHERE playlist_id = 1 AND track_id = 1",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM service_playlist_tracks WHERE playlist_id = 2 AND track_id = 2")
        .execute(pool)
        .await
        .unwrap();

    // ── Contract track links
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES
             (5, 1, 0, 1500000000),
             (5, 2, 0, 1400000000),
             (1, 3, 0, 1700000000),
             (2, 3, 0, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Populate file_resolved_tags like seed_lab_scenario does.
    crate::db::refresh_file_resolved_tags(pool).await.unwrap();

    // service_playlists: 2 → 5 (+3); service_playlist_tracks: 2 → 5
    // (2 − 1 deleted + 4 inserted = 5, i.e. +3).
    *counts.get_mut("service_playlists").unwrap() += 3;
    *counts.get_mut("service_playlist_tracks").unwrap() += 3;
    counts
}

// ── Rediscovery seed contract (Issue #80) ────────────────────────────────
//
// Fixed anchor, NO `unixepoch()`/`now()` anywhere — every timestamp below is a
// literal so tests can assert absolute values across runs.
//
//   REDISCOVERY_SEED_EPOCH = 1_790_000_000  (fixed "seed now", ≈ 2026-09-21 UTC)
//   DAY                    = 86_400
//   RECENT   = EPOCH -   90*DAY = 1_782_224_000  (≈ 3 months)
//   AGED_2Y  = EPOCH -  730*DAY = 1_726_928_000  (≈ 2 years)
//   AGED_5Y  = EPOCH - 1825*DAY = 1_632_320_000  (≈ 5 years)
//   PUSH_FRESH = EPOCH -  30*DAY = 1_787_408_000 (≈ 30 days, inside the 180d window)
//   PUSH_OLD   = AGED_5Y         = 1_632_320_000 (far outside the 180d window)

/// Fixed "seed now" anchor for the rediscovery scenario (≈ 2026-09-21 UTC).
pub const REDISCOVERY_SEED_EPOCH: i64 = 1_790_000_000;
const DAY: i64 = 86_400;
const RECENT: i64 = REDISCOVERY_SEED_EPOCH - 90 * DAY; // 1_782_224_000
const AGED_2Y: i64 = REDISCOVERY_SEED_EPOCH - 730 * DAY; // 1_726_928_000
const AGED_5Y: i64 = REDISCOVERY_SEED_EPOCH - 1825 * DAY; // 1_632_320_000
const PUSH_FRESH: i64 = REDISCOVERY_SEED_EPOCH - 30 * DAY; // 1_787_408_000
const PUSH_OLD: i64 = AGED_5Y; // 1_632_320_000

/// Seed the rediscovery scenario (Issue #80): extends [`seed_liked_songs_scenario`]
/// with tracks/files 10–18, Backpack tag+playlist 60 and 2 `rediscovery_pushes` rows.
///
/// # Contract (exact values — do not drift)
///
/// Anchor `REDISCOVERY_SEED_EPOCH = 1_790_000_000`, `DAY = 86_400`. Derived:
/// `RECENT = 1_782_224_000`, `AGED_2Y = 1_726_928_000`, `AGED_5Y = 1_632_320_000`,
/// `PUSH_FRESH = 1_787_408_000`, `PUSH_OLD = 1_632_320_000`.
///
/// Each new track N (10–18) is linked 1:1 to file N (same `isrc`) and lives in
/// playlist 5 `liked` with `added_at = last_touched_at`:
///
/// | Trk | File | bpm   | key  | genre  | last_touched_at | liked | pcount | backpack | pushed_at         |
/// |-----|------|-------|------|--------|-----------------|-------|--------|----------|-------------------|
/// | 10  | 10   | 120.0 | 1a   | House  | 1782224000      | ja    | 0      | nein     | —                 |
/// | 11  | 11   | 124.0 | 4m   | Techno | 1726928000      | ja    | 0      | nein     | —                 |
/// | 12  | 12   | 128.0 | 8m   | House  | 1632320000      | ja    | 0      | nein     | —                 |
/// | 13  | 13   | 140.0 | 12a  | Techno | 1632320000      | ja    | 0      | nein     | —                 |
/// | 14  | 14   | 155.0 | 1a   | House  | 1632320000      | ja    | 0      | nein     | —                 |
/// | 15  | 15   | NULL  | NULL | Techno | 1782224000      | ja    | 0      | nein     | —                 |
/// | 16  | 16   | 120.0 | 4m   | House  | 1632320000      | ja    | 1      | JA       | —                 |
/// | 17  | 17   | 124.0 | 8m   | Techno | 1632320000      | ja    | 0      | nein     | 1787408000 frisch |
/// | 18  | 18   | 128.0 | 12a  | House  | 1632320000      | ja    | 0      | nein     | 1632320000 alt    |
///
/// Track 16's backpack: tag 60 `Backpack` (category 5 Merkmal, `backpack=1`) +
/// curated playlist 60 `Backpack` with `service_playlist_tracks (60,16,0,1632320000)`;
/// `v_track_tags` then yields `backpack=1` for track 16. `rediscovery_pushes`: 2 rows —
/// track 17 `pushed_at=PUSH_FRESH` (slot 0), track 18 `pushed_at=PUSH_OLD` (slot 1).
///
/// # Expected return counts (base + this scenario)
///
/// `tags=4`, `files=13`, `file_locations=15`, `service_tracks=12`,
/// `service_playlists=6`, `service_playlist_tracks=15`, `folders=1`,
/// `rediscovery_pushes=2`. All INSERTs are idempotent (`OR IGNORE`, explicit ids).
pub async fn seed_rediscovery_scenario(pool: &Pool<Sqlite>) -> HashMap<String, usize> {
    let mut counts = seed_liked_songs_scenario(pool).await;
    // base counts: tags 3 · files 4 · file_locations 6 · service_tracks 3 ·
    // service_playlists 5 · service_playlist_tracks 5 · folders 1

    // ── Backpack tag (id 60, category 5 Merkmal) so playlist 60's name matches.
    sqlx::query(
        r#"INSERT OR IGNORE INTO tags (id, name, category_id, backpack)
           VALUES (60, 'Backpack', 5, 1)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Service tracks 10-18 (spotify; linked to files by the matching isrc).
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
           VALUES
             (10, 'spotify', 'spotify:track:rdis10', 'Rediscovery Ten',    'Artist R10', 'RDIS010', 1790000000),
             (11, 'spotify', 'spotify:track:rdis11', 'Rediscovery Eleven', 'Artist R11', 'RDIS011', 1790000000),
             (12, 'spotify', 'spotify:track:rdis12', 'Rediscovery Twelve', 'Artist R12', 'RDIS012', 1790000000),
             (13, 'spotify', 'spotify:track:rdis13', 'Rediscovery Thirteen','Artist R13', 'RDIS013', 1790000000),
             (14, 'spotify', 'spotify:track:rdis14', 'Rediscovery Fourteen','Artist R14', 'RDIS014', 1790000000),
             (15, 'spotify', 'spotify:track:rdis15', 'Rediscovery Fifteen','Artist R15', 'RDIS015', 1790000000),
             (16, 'spotify', 'spotify:track:rdis16', 'Rediscovery Sixteen','Artist R16', 'RDIS016', 1790000000),
             (17, 'spotify', 'spotify:track:rdis17', 'Rediscovery Seventeen','Artist R17', 'RDIS017', 1790000000),
             (18, 'spotify', 'spotify:track:rdis18', 'Rediscovery Eighteen','Artist R18', 'RDIS018', 1790000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Files 10-18 (bpm/key/genre per contract; track 15 unanalysed: NULL bpm/key).
    sqlx::query(
        r#"INSERT OR IGNORE INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
             bpm, musical_key, genre, isrc, play_count, last_played, duration_ms, file_hash)
           VALUES
             (10, '/test/stems/R10.flac', 'flac', 5000000, 1790000000, 'Rediscovery Ten',     'Artist R10', 120.0, '1a',  'House',  'RDIS010', 0, NULL, 300000, 'rdish10'),
             (11, '/test/stems/R11.flac', 'flac', 5000000, 1790000000, 'Rediscovery Eleven',  'Artist R11', 124.0, '4m',  'Techno', 'RDIS011', 0, NULL, 300000, 'rdish11'),
             (12, '/test/stems/R12.flac', 'flac', 5000000, 1790000000, 'Rediscovery Twelve',  'Artist R12', 128.0, '8m',  'House',  'RDIS012', 0, NULL, 300000, 'rdish12'),
             (13, '/test/stems/R13.flac', 'flac', 5000000, 1790000000, 'Rediscovery Thirteen','Artist R13', 140.0, '12a', 'Techno', 'RDIS013', 0, NULL, 300000, 'rdish13'),
             (14, '/test/stems/R14.flac', 'flac', 5000000, 1790000000, 'Rediscovery Fourteen','Artist R14', 155.0, '1a',  'House',  'RDIS014', 0, NULL, 300000, 'rdish14'),
             (15, '/test/stems/R15.flac', 'flac', 5000000, 1790000000, 'Rediscovery Fifteen', 'Artist R15', NULL,  NULL,  'Techno', 'RDIS015', 0, NULL, 300000, 'rdish15'),
             (16, '/test/stems/R16.flac', 'flac', 5000000, 1790000000, 'Rediscovery Sixteen', 'Artist R16', 120.0, '4m',  'House',  'RDIS016', 1, NULL, 300000, 'rdish16'),
             (17, '/test/stems/R17.flac', 'flac', 5000000, 1790000000, 'Rediscovery Seventeen','Artist R17',124.0, '8m',  'Techno', 'RDIS017', 0, NULL, 300000, 'rdish17'),
             (18, '/test/stems/R18.flac', 'flac', 5000000, 1790000000, 'Rediscovery Eighteen','Artist R18', 128.0, '12a', 'House',  'RDIS018', 0, NULL, 300000, 'rdish18')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── One local file_location per new file (owned=True; +9 → 15).
    sqlx::query(
        r#"INSERT OR IGNORE INTO file_locations (file_id, location_type, path, file_size, last_verified)
           VALUES
             (10, 'local', '/test/stems/R10.flac', 5000000, 1790000000),
             (11, 'local', '/test/stems/R11.flac', 5000000, 1790000000),
             (12, 'local', '/test/stems/R12.flac', 5000000, 1790000000),
             (13, 'local', '/test/stems/R13.flac', 5000000, 1790000000),
             (14, 'local', '/test/stems/R14.flac', 5000000, 1790000000),
             (15, 'local', '/test/stems/R15.flac', 5000000, 1790000000),
             (16, 'local', '/test/stems/R16.flac', 5000000, 1790000000),
             (17, 'local', '/test/stems/R17.flac', 5000000, 1790000000),
             (18, 'local', '/test/stems/R18.flac', 5000000, 1790000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Curated playlist 60 `Backpack` (+1 → 6) matching tag 60.
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlists (id, service, playlist_id, name, playlist_kind)
           VALUES (60, 'spotify', 'spotify:playlist:backpack', 'Backpack', 'curated')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── Track links: all 9 new tracks liked in playlist 5 (added_at=last_touched_at);
    //    track 16 additionally in curated playlist 60 (playlist_count=1).
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES
             (5, 10, 0, 1782224000),
             (5, 11, 0, 1726928000),
             (5, 12, 0, 1632320000),
             (5, 13, 0, 1632320000),
             (5, 14, 0, 1632320000),
             (5, 15, 0, 1782224000),
             (5, 16, 0, 1632320000),
             (5, 17, 0, 1632320000),
             (5, 18, 0, 1632320000),
             (60, 16, 0, 1632320000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // ── rediscovery_pushes: 2 rows (explicit ids for idempotency).
    sqlx::query(
        r#"INSERT OR IGNORE INTO rediscovery_pushes (id, track_id, playlist_id, pushed_at, facet_json, slot)
           VALUES
             (1, 17, NULL, 1787408000, NULL, 0),
             (2, 18, NULL, 1632320000, NULL, 1)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Keep the resolved-tag caches consistent with the new playlist/tag (as
    // seed_liked_songs_scenario does); the backpack check uses the live view.
    crate::db::refresh_file_resolved_tags(pool).await.unwrap();

    *counts.get_mut("tags").unwrap() += 1; // 3 → 4
    *counts.get_mut("files").unwrap() += 9; // 4 → 13
    *counts.get_mut("file_locations").unwrap() += 9; // 6 → 15
    *counts.get_mut("service_tracks").unwrap() += 9; // 3 → 12
    *counts.get_mut("service_playlists").unwrap() += 1; // 5 → 6
    *counts.get_mut("service_playlist_tracks").unwrap() += 10; // 5 → 15
    counts.insert("rediscovery_pushes".into(), 2);
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
