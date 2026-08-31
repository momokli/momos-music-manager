//! Integration tests for the tag roundtrip inbox — FULL feature set:
//! similar-tag suggestions, rename / merge / dismiss staging, and the
//! mapping-aware write path (`compute_target_comment`).
//!
//! Scenario (self-contained):
//!   Tag 10 "Groovy" (Mood, prefix M), tag 11 "Groove" (similar spelling).
//!   Playlist 70 "Groovy" with tracks 71 + 73 (ISRC-linked to files 71/73).
//!   File 70: stored comment has the typo tag "groovie", NO track link.
//!   File 71: stored comment has the typo tag "groovie", track in "Groovy".
//!   File 72: stored comment has "house", no track link (no suggestions).
//!   File 73: stored comment NULL, track in "Groovy" (tag added by target).
//!
//!   → Files 70-73 are all in the inbox. "groovie" is a NEW tag (not in the
//!     tags table); its suggestions include "groovy" (distance 2) and
//!     "groove" (distance 1).

mod common;

use serde_json::{Value, json};
use sqlx::SqlitePool;

// ═══════════════════════════════════════════════════════════════════════════
// Scenario seeding
// ═══════════════════════════════════════════════════════════════════════════

async fn seed_resolve_scenario(pool: &SqlitePool) {
    // Existing tags (canonical vocabulary).
    sqlx::query(
        r#"INSERT OR IGNORE INTO tags (id, name, category_id, backpack)
           VALUES (10, 'Groovy', 3, 0), (11, 'Groove', 3, 0)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Files 70-73. Comments as described in the module doc comment.
    sqlx::query(
        r#"INSERT OR IGNORE INTO files (id, file_path, file_type, file_size, last_modified,
             title, artist, isrc, comment, file_hash, spotify_id)
           VALUES
             (70, '/test/stems/Resolve - TypoNoLink.flac','flac', 5000000, 1700000000,
              'Resolve TypoNoLink', 'Resolve Artist', 'US070',
              '[___] groovie sp:spotify:track:t70', 'hash70', 'spotify:track:t70'),
             (71, '/test/stems/Resolve - TypoLinked.flac', 'flac', 5000000, 1700000000,
              'Resolve TypoLinked', 'Resolve Artist', 'US071',
              '[___] groovie sp:spotify:track:t71', 'hash71', 'spotify:track:t71'),
             (72, '/test/stems/Resolve - House.flac',      'flac', 5000000, 1700000000,
              'Resolve House', 'Resolve Artist', 'US072',
              '[___] house sp:spotify:track:t72', 'hash72', 'spotify:track:t72'),
             (73, '/test/stems/Resolve - NoComment.flac',  'flac', 5000000, 1700000000,
              'Resolve NoComment', 'Resolve Artist', 'US073',
              NULL, 'hash73', 'spotify:track:t73')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Service tracks (ISRC link: v_file_track_link matches st.isrc = f.isrc).
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
           VALUES
             (71, 'spotify', 'spotify:track:t71', 'Resolve TypoLinked', 'Resolve Artist', 'US071', 1700000000),
             (73, 'spotify', 'spotify:track:t73', 'Resolve NoComment', 'Resolve Artist', 'US073', 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // Playlist "Groovy" + track links → tag resolution.
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlists (id, service, playlist_id, name, snapshot_id)
           VALUES (70, 'spotify', 'spotify:playlist:resolve70', 'Groovy', 'snap-resolve')"#,
    )
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES (70, 71, 0, 1700000000), (70, 73, 0, 1700000000)"#,
    )
    .execute(pool)
    .await
    .unwrap();

    momos_music_manager::db::refresh_file_resolved_tags(pool)
        .await
        .unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════
// Similar-tag suggestions
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn db_inbox_new_tags_with_suggestions() {
    let pool = common::create_test_db().await;
    seed_resolve_scenario(&pool).await;

    let items = momos_music_manager::db::get_inbox_files(&pool, 100, 0)
        .await
        .unwrap();
    assert_eq!(items.len(), 4);

    // File 70: new tag "groovie" (from the stored comment side), suggestions
    // include "groove" (distance 1) and "groovy" (distance 2).
    let f70 = items.iter().find(|i| i.file_id == 70).unwrap();
    let groovie = f70.new_tags.iter().find(|n| n.tag == "groovie").unwrap();
    assert!(!groovie.added, "groovie comes from the stored comment (removed side)");
    assert!(groovie.mapping.is_none(), "no mapping yet");

    let suggestions: Vec<(String, usize)> = groovie
        .suggestions
        .iter()
        .map(|s| (s.tag.clone(), s.distance))
        .collect();
    // Suggestion names keep the canonical spelling from the tags table.
    assert!(
        suggestions.iter().any(|(t, d)| t.eq_ignore_ascii_case("groove") && *d == 1),
        "groove must be suggested with distance 1, got {:?}",
        suggestions
    );
    assert!(
        suggestions.iter().any(|(t, d)| t.eq_ignore_ascii_case("groovy") && *d == 2),
        "groovy must be suggested with distance 2, got {:?}",
        suggestions
    );

    // Suggestion counts come from the materialized resolution:
    // files 71 + 73 resolve tag "Groovy" → count 2.
    let groovy_sugg = groovie
        .suggestions
        .iter()
        .find(|s| s.tag.eq_ignore_ascii_case("groovy"))
        .unwrap();
    assert_eq!(groovy_sugg.count, 2);

    // File 71: diff has BOTH the typo (removed) and the canonical (added).
    let f71 = items.iter().find(|i| i.file_id == 71).unwrap();
    assert_eq!(f71.diff.tags_added, vec!["groovy"]);
    assert_eq!(f71.diff.tags_removed, vec!["groovie"]);
    let groovy_nt = f71.new_tags.iter().find(|n| n.tag == "groovy").unwrap();
    assert!(groovy_nt.added, "groovy comes from the target side");
    // The canonical tag itself gets no self-suggestion (distance 0 excluded).
    assert!(!groovy_nt
        .suggestions
        .iter()
        .any(|s| s.tag == "groovy"));

    // File 72: "house" has no similar existing tag.
    let f72 = items.iter().find(|i| i.file_id == 72).unwrap();
    let house = f72.new_tags.iter().find(|n| n.tag == "house").unwrap();
    assert!(house.suggestions.is_empty(), "house has no similar tags");

    // File 73: canonical tag from the target side only.
    let f73 = items.iter().find(|i| i.file_id == 73).unwrap();
    let groovy_nt = f73.new_tags.iter().find(|n| n.tag == "groovy").unwrap();
    assert!(groovy_nt.added);
    assert!(!groovy_nt
        .suggestions
        .iter()
        .any(|s| s.tag == "groovy"));
}

// ═══════════════════════════════════════════════════════════════════════════
// Rename / merge / dismiss staging
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn api_inbox_rename_persists_mapping() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_resolve_scenario(&pool).await;

    let resp = client
        .post(format!("{base}/api/inbox/resolve"))
        .json(&json!({
            "tag": "groovie",
            "action": "rename",
            "targetTag": "groovyy"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["rawTag"], "groovie");
    assert_eq!(body["data"]["targetTag"], "groovyy");
    assert_eq!(body["data"]["action"], "rename");
    assert_eq!(body["data"]["status"], "open");
    // 2 files currently carry "groovie" in their stored comment (70, 71).
    assert_eq!(body["data"]["fileCount"], 2);

    // Mapping is listed.
    let resp = client
        .get(format!("{base}/api/inbox/mappings"))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let mappings = body["data"]["mappings"].as_array().unwrap();
    assert_eq!(mappings.len(), 1);
    assert_eq!(mappings[0]["rawTag"], "groovie");
    assert_eq!(mappings[0]["targetTag"], "groovyy");
}

#[tokio::test]
async fn api_inbox_merge_into_existing_tag() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_resolve_scenario(&pool).await;

    let resp = client
        .post(format!("{base}/api/inbox/resolve"))
        .json(&json!({
            "tag": "groovie",
            "action": "merge",
            "targetTag": "Groovy"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "merge into existing tag must succeed");

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["rawTag"], "groovie");
    assert_eq!(body["data"]["targetTag"], "groovy");
    assert_eq!(body["data"]["action"], "merge");
}

#[tokio::test]
async fn api_inbox_merge_validation() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_resolve_scenario(&pool).await;

    // Merge into a NON-existing tag → 400.
    let resp = client
        .post(format!("{base}/api/inbox/resolve"))
        .json(&json!({"tag": "groovie", "action": "merge", "targetTag": "nonexistent"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // Merge into itself → 400.
    let resp = client
        .post(format!("{base}/api/inbox/resolve"))
        .json(&json!({"tag": "groovie", "action": "merge", "targetTag": "groovie"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // Rename without targetTag → 400.
    let resp = client
        .post(format!("{base}/api/inbox/resolve"))
        .json(&json!({"tag": "groovie", "action": "rename"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // Unknown action → 400.
    let resp = client
        .post(format!("{base}/api/inbox/resolve"))
        .json(&json!({"tag": "groovie", "action": "explode", "targetTag": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // No mappings were created by any of the failed calls.
    let resp = client
        .get(format!("{base}/api/inbox/mappings"))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["mappings"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn api_inbox_dismiss_has_no_write_effect() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_resolve_scenario(&pool).await;

    let resp = client
        .post(format!("{base}/api/inbox/resolve"))
        .json(&json!({"tag": "groovie", "action": "dismiss"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["action"], "dismiss");

    // Dismissed mappings must NOT affect the write path: the target for file
    // 70 stays tag-less (groovie is dropped, not kept).
    let target = momos_music_manager::db::compute_target_comment(&pool, 70)
        .await
        .unwrap();
    assert_eq!(target, "[___] sp:spotify:track:t70");
}

// ═══════════════════════════════════════════════════════════════════════════
// Staging semantics: mapping-aware write path
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn db_write_path_rename_keeps_tag_with_new_spelling() {
    let pool = common::create_test_db().await;
    seed_resolve_scenario(&pool).await;

    // Rename "groovie" → "groovyy" (typo fix): the write must produce the NEW
    // spelling for every file whose stored comment carries the typo.
    momos_music_manager::db::upsert_tag_inbox_mapping(&pool, "groovie", "rename", "groovyy")
        .await
        .unwrap();

    // File 70: target had no tags; the mapped stored tag is kept, corrected.
    let t70 = momos_music_manager::db::compute_target_comment(&pool, 70)
        .await
        .unwrap();
    assert_eq!(t70, "[___] groovyy sp:spotify:track:t70");

    // File 71: canonical from the chain + mapped stored tag.
    let t71 = momos_music_manager::db::compute_target_comment(&pool, 71)
        .await
        .unwrap();
    assert_eq!(t71, "[_M_] groovy groovyy sp:spotify:track:t71");

    // File 72: unaffected.
    let t72 = momos_music_manager::db::compute_target_comment(&pool, 72)
        .await
        .unwrap();
    assert_eq!(t72, "[___] sp:spotify:track:t72");

    // File 73: unaffected.
    let t73 = momos_music_manager::db::compute_target_comment(&pool, 73)
        .await
        .unwrap();
    assert_eq!(t73, "[_M_] groovy sp:spotify:track:t73");
}

#[tokio::test]
async fn db_write_path_merge_retags_all_affected_files() {
    let pool = common::create_test_db().await;
    seed_resolve_scenario(&pool).await;

    // Merge "groovie" → "groovy": ALL files with the typo get the canonical.
    momos_music_manager::db::upsert_tag_inbox_mapping(&pool, "groovie", "merge", "groovy")
        .await
        .unwrap();

    let t70 = momos_music_manager::db::compute_target_comment(&pool, 70)
        .await
        .unwrap();
    assert_eq!(t70, "[___] groovy sp:spotify:track:t70");
    assert!(!t70.contains("groovie"), "typo must be gone from the target");

    let t71 = momos_music_manager::db::compute_target_comment(&pool, 71)
        .await
        .unwrap();
    assert_eq!(t71, "[_M_] groovy sp:spotify:track:t71");
    assert!(!t71.contains("groovie"));

    // Playlist-typo case: a target that RESOLVES the raw tag is rewritten to
    // the canonical tag. Simulate by resolving "Groove" for file 73: rename
    // its resolved tag via a mapping (groove → groovy) and check the target.
    momos_music_manager::db::upsert_tag_inbox_mapping(&pool, "groove", "merge", "groovy")
        .await
        .unwrap();
    // File 73's resolved tag is "groovy" (playlist Groovy), so use a file
    // whose resolution would carry "groove": give file 72 a track in a
    // "Groove" playlist.
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
           VALUES (72, 'spotify', 'spotify:track:t72', 'Resolve House', 'Resolve Artist', 'US072', 1700000000)"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlists (id, service, playlist_id, name, snapshot_id)
           VALUES (71, 'spotify', 'spotify:playlist:resolve71', 'Groove', 'snap-resolve2')"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
           VALUES (71, 72, 0, 1700000000)"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    momos_music_manager::db::refresh_file_resolved_tags(&pool)
        .await
        .unwrap();

    let t72 = momos_music_manager::db::compute_target_comment(&pool, 72)
        .await
        .unwrap();
    assert!(
        !t72.contains("groove"),
        "resolved raw tag must be rewritten, got: {}",
        t72
    );
    assert!(t72.contains("groovy"), "canonical tag must be written: {}", t72);
}

#[tokio::test]
async fn api_inbox_shows_staged_target_and_mapping_after_merge() {
    let (client, base, pool) = common::spawn_test_app().await;
    seed_resolve_scenario(&pool).await;

    // Merge groovie → groovy via the API.
    client
        .post(format!("{base}/api/inbox/resolve"))
        .json(&json!({"tag": "groovie", "action": "merge", "targetTag": "groovy"}))
        .send()
        .await
        .unwrap();

    // The inbox now shows the STAGED target (canonical spelling) and the
    // open mapping on the new-tag entry.
    let resp = client
        .get(format!("{base}/api/inbox?limit=100"))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let files = body["data"]["files"].as_array().unwrap();

    let f70 = files.iter().find(|f| f["fileId"] == 70).unwrap();
    assert_eq!(
        f70["targetComment"], "[___] groovy sp:spotify:track:t70",
        "target must reflect the merge (staging)"
    );
    let groovie = f70["newTags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["tag"] == "groovie")
        .unwrap();
    assert_eq!(groovie["mapping"]["action"], "merge");
    assert_eq!(groovie["mapping"]["targetTag"], "groovy");
}

#[tokio::test]
async fn db_inbox_rename_to_self_drops_file_after_staging() {
    let pool = common::create_test_db().await;
    seed_resolve_scenario(&pool).await;

    // "Keep this typed tag": rename the tag to itself. The staged target
    // equals the stored comment → the file leaves the inbox WITHOUT a write.
    momos_music_manager::db::upsert_tag_inbox_mapping(&pool, "groovie", "rename", "groovie")
        .await
        .unwrap();

    let items = momos_music_manager::db::get_inbox_files(&pool, 100, 0)
        .await
        .unwrap();
    let ids: Vec<i64> = items.iter().map(|i| i.file_id).collect();
    assert!(
        !ids.contains(&70),
        "file 70 stored comment == staged target after rename-to-self, got {:?}",
        ids
    );
    // Files 71 (typo still differs from chain target), 72, 73 remain.
    assert_eq!(items.len(), 3);

    let count = momos_music_manager::db::get_inbox_count(&pool).await.unwrap();
    assert_eq!(count, 3);
}
