//! Integration tests for the tag roundtrip inbox: /api/inbox + /api/inbox/count.
//!
//! The inbox lists files whose STORED comment differs from the GENERATED
//! TARGET comment. Comparison is roundtrip-based (`parse → generate →
//! compare`), so formatting differences (tag order, quoting, case) never
//! create false positives — only real content changes are flagged.
//!
//! Scenario (self-contained, no dependency on seed_basic_scenario):
//!   Tag 10 "Groovy" (Mood, prefix M), playlist 50 "Groovy", files 50-54
//!   linked via ISRC → track → playlist → tag, so every file resolves to
//!   target `[_M_] groovy sp:spotify:track:t<N>`.
//!
//!   - File 50: comment exactly matches the target       → NOT in inbox
//!   - File 51: comment differs only in formatting       → NOT in inbox
//!   - File 52: comment missing the source ID            → in inbox
//!   - File 53: comment is NULL                          → in inbox
//!   - File 54: comment has an extra tag + wrong PMV     → in inbox

mod common;

use serde_json::Value;
use sqlx::SqlitePool;

// ═══════════════════════════════════════════════════════════════════════════
// Scenario seeding
// ═══════════════════════════════════════════════════════════════════════════

async fn seed_inbox_scenario(pool: &SqlitePool) {
    // Tag "Groovy" in Mood category (id 3, prefix M). IDs > 6 avoid migration
    // 001's seeded phase tags.
    sqlx::query(
        r#"INSERT OR IGNORE INTO tags (id, name, category_id, backpack)
           VALUES (10, 'Groovy', 3, 0)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Files 50-54. Comments as described in the module doc comment.
    sqlx::query(
        r#"INSERT OR IGNORE INTO files (id, file_path, file_type, file_size, last_modified,
             title, artist, isrc, comment, file_hash, spotify_id)
           VALUES
             (50, '/test/stems/Inbox - Exact.flac',    'flac', 5000000, 1700000000,
              'Inbox Exact', 'Inbox Artist', 'US050',
              '[_M_] groovy sp:spotify:track:t50', 'hash50', 'spotify:track:t50'),
             (51, '/test/stems/Inbox - Formatting.flac','flac', 5000000, 1700000000,
              'Inbox Fmt', 'Inbox Artist', 'US051',
              '[_M_] GROOVY  sp:spotify:track:t51', 'hash51', 'spotify:track:t51'),
             (52, '/test/stems/Inbox - MissingSource.flac','flac', 5000000, 1700000000,
              'Inbox NoSrc', 'Inbox Artist', 'US052',
              '[_M_] groovy', 'hash52', 'spotify:track:t52'),
             (53, '/test/stems/Inbox - NoComment.flac', 'flac', 5000000, 1700000000,
              'Inbox None', 'Inbox Artist', 'US053',
              NULL, 'hash53', 'spotify:track:t53'),
             (54, '/test/stems/Inbox - ExtraTag.flac',  'flac', 5000000, 1700000000,
              'Inbox Extra', 'Inbox Artist', 'US054',
              '[___] house', 'hash54', 'spotify:track:t54')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Service tracks (ISRC link: v_file_track_link matches st.isrc = f.isrc)
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
           VALUES
             (50, 'spotify', 'spotify:track:t50', 'Inbox Exact', 'Inbox Artist', 'US050', 1700000000),
             (51, 'spotify', 'spotify:track:t51', 'Inbox Fmt',   'Inbox Artist', 'US051', 1700000000),
             (52, 'spotify', 'spotify:track:t52', 'Inbox NoSrc', 'Inbox Artist', 'US052', 1700000000),
             (53, 'spotify', 'spotify:track:t53', 'Inbox None',  'Inbox Artist', 'US053', 1700000000),
             (54, 'spotify', 'spotify:track:t54', 'Inbox Extra', 'Inbox Artist', 'US054', 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Playlist "Groovy" + track links → tag resolution
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlists (id, service, playlist_id, name, snapshot_id)
           VALUES (50, 'spotify', 'spotify:playlist:inbox50', 'Groovy', 'snap-inbox')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES (50, 50, 0, 1700000000), (50, 51, 0, 1700000000), (50, 52, 0, 1700000000),
                  (50, 53, 0, 1700000000), (50, 54, 0, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Materialize the tag resolution chain (file → track → playlist → tag)
    momos_music_manager::db::refresh_file_resolved_tags(pool)
        .await
        .unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════
// DB layer tests (roundtrip semantics)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn db_inbox_roundtrip_ignores_formatting() {
    let pool = common::create_test_db().await;
    seed_inbox_scenario(&pool).await;

    let items = momos_music_manager::db::get_inbox_files(&pool, 100, 0)
        .await
        .unwrap();

    let ids: Vec<i64> = items.iter().map(|i| i.file_id).collect();
    assert!(
        !ids.contains(&50),
        "exact match must NOT be in inbox, got: {:?}",
        ids
    );
    assert!(
        !ids.contains(&51),
        "formatting-only difference must NOT be in inbox (roundtrip), got: {:?}",
        ids
    );
    assert!(ids.contains(&52), "missing source ID must be in inbox: {:?}", ids);
    assert!(ids.contains(&53), "NULL comment must be in inbox: {:?}", ids);
    assert!(ids.contains(&54), "extra tag must be in inbox: {:?}", ids);
}

#[tokio::test]
async fn db_inbox_diff_details() {
    let pool = common::create_test_db().await;
    seed_inbox_scenario(&pool).await;

    let items = momos_music_manager::db::get_inbox_files(&pool, 100, 0)
        .await
        .unwrap();
    assert_eq!(items.len(), 3);

    // File 52: source ID missing → diff reports source_ids_added.
    let f52 = items.iter().find(|i| i.file_id == 52).unwrap();
    assert_eq!(f52.target_comment, "[_M_] groovy sp:spotify:track:t52");
    assert_eq!(f52.diff.source_ids_added, vec!["sp:spotify:track:t52"]);
    assert!(f52.diff.tags_added.is_empty());
    assert!(f52.diff.tags_removed.is_empty());
    assert!(f52.diff.phase_changed.is_none());

    // File 53: NULL comment → everything is added.
    let f53 = items.iter().find(|i| i.file_id == 53).unwrap();
    assert_eq!(f53.comment, None);
    assert_eq!(f53.diff.tags_added, vec!["groovy"]);
    assert_eq!(f53.diff.source_ids_added, vec!["sp:spotify:track:t53"]);
    assert_eq!(f53.diff.mood_changed, Some(('_', 'M')));

    // File 54: extra tag + wrong PMV.
    let f54 = items.iter().find(|i| i.file_id == 54).unwrap();
    assert_eq!(f54.diff.tags_added, vec!["groovy"]);
    assert_eq!(f54.diff.tags_removed, vec!["house"]);
    assert_eq!(f54.diff.mood_changed, Some(('_', 'M')));
}

#[tokio::test]
async fn db_inbox_pagination() {
    let pool = common::create_test_db().await;
    seed_inbox_scenario(&pool).await;

    let page1 = momos_music_manager::db::get_inbox_files(&pool, 2, 0)
        .await
        .unwrap();
    assert_eq!(page1.len(), 2);

    let page2 = momos_music_manager::db::get_inbox_files(&pool, 2, 2)
        .await
        .unwrap();
    assert_eq!(page2.len(), 1);

    // No overlap between pages
    let ids1: Vec<i64> = page1.iter().map(|i| i.file_id).collect();
    let ids2: Vec<i64> = page2.iter().map(|i| i.file_id).collect();
    assert!(ids1.iter().all(|id| !ids2.contains(id)));
}

#[tokio::test]
async fn db_inbox_count() {
    let pool = common::create_test_db().await;
    seed_inbox_scenario(&pool).await;

    let count = momos_music_manager::db::get_inbox_count(&pool).await.unwrap();
    assert_eq!(count, 3, "exactly files 52, 53, 54 need updates");

    // Fix one file → count drops.
    sqlx::query("UPDATE files SET comment = '[_M_] groovy sp:spotify:track:t52' WHERE id = 52")
        .execute(&pool)
        .await
        .unwrap();
    let count = momos_music_manager::db::get_inbox_count(&pool).await.unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn db_inbox_empty_when_nothing_to_do() {
    let pool = common::create_test_db().await;
    seed_inbox_scenario(&pool).await;

    // Bring every comment to its target (roundtrip-equal is enough).
    sqlx::query("UPDATE files SET comment = '[_M_] groovy sp:spotify:track:t52' WHERE id = 52")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE files SET comment = '[_M_] groovy sp:spotify:track:t53' WHERE id = 53")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE files SET comment = '[_M_] groovy sp:spotify:track:t54' WHERE id = 54")
        .execute(&pool)
        .await
        .unwrap();

    let items = momos_music_manager::db::get_inbox_files(&pool, 100, 0)
        .await
        .unwrap();
    assert!(items.is_empty(), "inbox must be empty, got: {:?}", items);

    let count = momos_music_manager::db::get_inbox_count(&pool).await.unwrap();
    assert_eq!(count, 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// API tests
// ═══════════════════════════════════════════════════════════════════════════

async fn api_data(body: Value) -> Value {
    body["data"].clone()
}

#[tokio::test]
async fn api_inbox_lists_files_with_diffs() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_inbox_scenario(&pool).await;

    let resp = client.get(format!("{base}/api/inbox")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let data = api_data(body).await;

    let files = data["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);

    let ids: Vec<i64> = files
        .iter()
        .map(|f| f["fileId"].as_i64().unwrap())
        .collect();
    assert!(ids.contains(&52) && ids.contains(&53) && ids.contains(&54));
    assert!(!ids.contains(&50) && !ids.contains(&51));

    // Total = 3 (not just the page size)
    assert_eq!(data["total"].as_i64().unwrap(), 3);

    // Diff payload is structured (camelCase)
    let f54 = files.iter().find(|f| f["fileId"].as_i64().unwrap() == 54).unwrap();
    assert_eq!(f54["diff"]["tagsAdded"][0], "groovy");
    assert_eq!(f54["diff"]["tagsRemoved"][0], "house");
    assert_eq!(f54["targetComment"], "[_M_] groovy sp:spotify:track:t54");
    assert_eq!(f54["comment"], "[___] house");
}

#[tokio::test]
async fn api_inbox_count() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_inbox_scenario(&pool).await;

    let resp = client
        .get(format!("{base}/api/inbox/count"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["count"].as_i64().unwrap(), 3);
}

#[tokio::test]
async fn api_inbox_pagination_params() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_inbox_scenario(&pool).await;

    let resp = client
        .get(format!("{base}/api/inbox?limit=2&offset=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let files = body["data"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(body["data"]["total"].as_i64().unwrap(), 3);
}

#[tokio::test]
async fn api_inbox_empty_db() {
    let (client, base, _pool) = common::spawn_test_app().await;

    let resp = client.get(format!("{base}/api/inbox")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["files"].as_array().unwrap().len(), 0);
    assert_eq!(body["data"]["total"].as_i64().unwrap(), 0);

    let resp = client
        .get(format!("{base}/api/inbox/count"))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["count"].as_i64().unwrap(), 0);
}
