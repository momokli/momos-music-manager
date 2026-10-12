//! Integration tests for `/api/tracks*` endpoints.
//!
//! Tests cover:
//!   - Pagination (list, count parity)
//!   - Filtering (services, playlists, tags, search)
//!   - Sorting
//!   - Track detail (inBackpack, linked files, WAV source variants)
//!
//! All tests create a fresh in-memory DB, run all migrations, seed
//! hand-crafted data, and hit the running Axum server.

mod common;

use momos_music_manager::db::refresh_track_resolved_tags;

// ═════════════════════════════════════════════════════════════════════════
// List / Pagination
// ═════════════════════════════════════════════════════════════════════════

/// `GET /api/tracks?limit=2` returns exactly 2 items.
#[tokio::test]
async fn tracks_list_paginated() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?limit=2", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    assert_eq!(
        tracks.len(),
        2,
        "expected 2 tracks with limit=2, got {}",
        tracks.len()
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: services
// ═════════════════════════════════════════════════════════════════════════

/// `?services=spotify` returns all 3 tracks (all are service='spotify').
#[tokio::test]
async fn tracks_filter_services() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?services=spotify&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    assert_eq!(
        tracks.len(),
        3,
        "expected 3 tracks for services=spotify, got {}",
        tracks.len()
    );

    // All returned tracks should have service='spotify'
    for t in tracks {
        assert_eq!(
            t["service"], "spotify",
            "track {} has unexpected service",
            t["id"]
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: playlists
// ═════════════════════════════════════════════════════════════════════════

/// `?playlists=Groovy` returns track 1 (Title One, Artist A).
#[tokio::test]
async fn tracks_filter_playlists() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?playlists=Groovy&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    assert_eq!(
        tracks.len(),
        1,
        "expected 1 track for playlists=Groovy, got {}",
        tracks.len()
    );
    assert_eq!(tracks[0]["title"], "Title One");
    assert_eq!(tracks[0]["artist"], "Artist A");
    assert!(tracks[0]["isrc"].as_str().map_or(false, |s| s == "US001"));
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: tags (via track_resolved_tags materialized table)
// ═════════════════════════════════════════════════════════════════════════

/// `?tags=Groovy` returns track 1 (the track in the "Groovy" playlist,
/// which matches the "Groovy" tag case-insensitively).
#[tokio::test]
async fn tracks_filter_tags() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?tags=Groovy&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    assert_eq!(
        tracks.len(),
        1,
        "expected 1 track for tags=Groovy, got {}",
        tracks.len()
    );
    assert_eq!(tracks[0]["title"], "Title One");
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: search
// ═════════════════════════════════════════════════════════════════════════

/// `?search=Orphan` returns track 3 (Orphan Demo).
#[tokio::test]
async fn tracks_filter_search() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?search=Orphan&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"]
        .as_array()
        .unwrap_or_else(|| panic!("expected 'data' array in response, got: {:#}", json));
    assert_eq!(
        tracks.len(),
        1,
        "expected 1 track for search=Orphan, got {}",
        tracks.len()
    );

    // Normalise case: the search might return a partial match, but Title should contain "Orphan"
    let title = tracks[0]["title"].as_str().unwrap();
    assert!(
        title.to_lowercase().contains("orphan"),
        "expected title to contain 'Orphan', got '{}'",
        title
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Sort
// ═════════════════════════════════════════════════════════════════════════

/// `?sort=title&order=asc` returns tracks alphabetically:
///   Orphan Demo → Title One → Track Two
#[tokio::test]
async fn tracks_filter_sort() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?sort=title&order=asc&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    assert_eq!(tracks.len(), 3);

    // Use debug print if assertions fail
    let titles: Vec<&str> = tracks
        .iter()
        .map(|t| t["title"].as_str().unwrap_or("(null)"))
        .collect();

    assert_eq!(
        titles,
        vec!["Orphan Demo", "Title One", "Track Two"],
        "tracks not sorted alphabetically by title ascending — got: {:?}",
        titles
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Track detail
// ═════════════════════════════════════════════════════════════════════════

/// `GET /api/tracks/1/detail` returns full metadata, linked files, tags, and
/// playlist info.
#[tokio::test]
async fn tracks_detail() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks/1/detail", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let detail = &json["data"];

    // Debug helper: uncomment to inspect the response shape on failure
    // eprintln!("{:#}", detail);

    assert_eq!(detail["title"], "Title One");
    assert_eq!(detail["artist"], "Artist A");
    assert_eq!(detail["isrc"], "US001");
    assert_eq!(detail["service"], "spotify");

    // Should have inBackpack field
    assert!(
        detail.get("inBackpack").is_some(),
        "inBackpack field missing"
    );

    // Should have linked files (via v_file_track_link: ISRC US001 → files 1 & 2)
    let files = detail["files"].as_array().unwrap();
    assert!(
        !files.is_empty(),
        "expected at least 1 linked file for track 1"
    );

    // Track 1 (US001) should be linked to both file 1 (FLAC) and file 2 (stem.m4a)
    let file_ids: Vec<i64> = files.iter().map(|f| f["id"].as_i64().unwrap()).collect();
    assert!(
        file_ids.contains(&1),
        "expected file id=1 (FLAC) for US001, got file ids: {:?}",
        file_ids
    );
    assert!(
        file_ids.contains(&2),
        "expected file id=2 (stem.m4a) for US001, got file ids: {:?}",
        file_ids
    );

    // Should have playlist info
    assert!(detail.get("playlists").is_some(), "playlists field missing");

    // Should have tags
    assert!(detail.get("tags").is_some(), "tags field missing");
}

// ═════════════════════════════════════════════════════════════════════════
// Track detail — inBackpack
// ═════════════════════════════════════════════════════════════════════════

/// Track 1 is in playlist "Groovy" → tag "Groovy" has backpack=0 → inBackpack=false.
/// Track 2 is in playlist "Deep Mix" → no matching tag (name ≠ "Deep Mix") → inBackpack=false.
/// Track 3 (orphan) → no playlists → inBackpack=false.
#[tokio::test]
async fn tracks_detail_has_in_backpack() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    // Track 1: tag "Groovy" has backpack=0
    let resp = client
        .get(format!("{}/api/tracks/1/detail", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let detail = &json["data"];
    assert_eq!(
        detail["inBackpack"], false,
        "track 1 (tag Groovy, backpack=0) should have inBackpack=false, got: {:#}",
        detail["inBackpack"]
    );

    // Track 2: playlist "Deep Mix" doesn't exactly match tag "Deep" → no tag → backpack=false
    let resp = client
        .get(format!("{}/api/tracks/2/detail", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let detail = &json["data"];
    assert_eq!(
        detail["inBackpack"], false,
        "track 2 (no matching tag) should have inBackpack=false, got: {:#}",
        detail["inBackpack"]
    );

    // Track 3: orphan, no playlists → no tags → backpack=false
    let resp = client
        .get(format!("{}/api/tracks/3/detail", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let detail = &json["data"];
    assert_eq!(
        detail["inBackpack"], false,
        "track 3 (orphan, no tags) should have inBackpack=false, got: {:#}",
        detail["inBackpack"]
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Count parity
// ═════════════════════════════════════════════════════════════════════════

/// `/api/tracks/count` with the same filter params should match the length
/// of the list endpoint (for simple queries without pagination).
#[tokio::test]
async fn tracks_count_parity() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    // Fetch all tracks (no filters)
    let list_resp = client
        .get(format!("{}/api/tracks?limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(list_resp.status(), 200);
    let list_json: serde_json::Value = list_resp.json().await.unwrap();
    let list_count = list_json["data"].as_array().unwrap().len() as i64;

    // Fetch count (no filters)
    let count_resp = client
        .get(format!("{}/api/tracks/count", base))
        .send()
        .await
        .unwrap();
    assert_eq!(count_resp.status(), 200);
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap_or_else(|| {
        panic!(
            "expected 'data' to be an integer in count response, got: {:#}",
            count_json
        )
    });

    assert_eq!(
        list_count, count,
        "list returned {} tracks but count returned {}",
        list_count, count
    );

    // Filtered: services=spotify
    let list_resp = client
        .get(format!("{}/api/tracks?services=spotify&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(list_resp.status(), 200);
    let list_json: serde_json::Value = list_resp.json().await.unwrap();
    let list_count = list_json["data"].as_array().unwrap().len() as i64;

    let count_resp = client
        .get(format!("{}/api/tracks/count?services=spotify", base))
        .send()
        .await
        .unwrap();
    assert_eq!(count_resp.status(), 200);
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        list_count, count,
        "services=spotify: list={} ≠ count={}",
        list_count, count
    );

    // Filtered: playlists=Groovy
    let list_resp = client
        .get(format!("{}/api/tracks?playlists=Groovy&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(list_resp.status(), 200);
    let list_json: serde_json::Value = list_resp.json().await.unwrap();
    let list_count = list_json["data"].as_array().unwrap().len() as i64;

    let count_resp = client
        .get(format!("{}/api/tracks/count?playlists=Groovy", base))
        .send()
        .await
        .unwrap();
    assert_eq!(count_resp.status(), 200);
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        list_count, count,
        "playlists=Groovy: list={} ≠ count={}",
        list_count, count
    );

    // Filtered: search=Orphan
    let list_resp = client
        .get(format!("{}/api/tracks?search=Orphan&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(list_resp.status(), 200);
    let list_json: serde_json::Value = list_resp.json().await.unwrap();
    let list_count = list_json["data"].as_array().unwrap().len() as i64;

    let count_resp = client
        .get(format!("{}/api/tracks/count?search=Orphan", base))
        .send()
        .await
        .unwrap();
    assert_eq!(count_resp.status(), 200);
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        list_count, count,
        "search=Orphan: list={} ≠ count={}",
        list_count, count
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Track detail — WAV source variants
// ═════════════════════════════════════════════════════════════════════════

/// When `seed_wav_variant_data` is called, 5 WAV files (ids 20–24) are created
/// with `source_of=2` (the stem.m4a file linked to track 1 via ISRC US001).
/// The track detail endpoint should include these WAV files with `stemType` set.
#[tokio::test]
async fn tracks_detail_wav_variants() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_wav_variant_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks/1/detail", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let detail = &json["data"];

    let files = detail["files"]
        .as_array()
        .unwrap_or_else(|| panic!("expected 'files' array in track detail, got: {:#}", json));

    // Debug: uncomment to inspect the response shape on failure
    // eprintln!("Track 1 total files: {}", files.len());
    // for f in files {
    //     eprintln!(
    //         "  id={}, fileType={:?}, stemType={:?}, filePath={}",
    //         f["id"], f["fileType"], f["stemType"], f["filePath"]
    //     );
    // }

    // Collect all file IDs to verify WAV sources are present
    let file_ids: Vec<i64> = files.iter().map(|f| f["id"].as_i64().unwrap()).collect();

    // Should include the 5 WAV source files (ids 20-24)
    let wav_ids: Vec<i64> = (20..=24).collect();
    for wid in &wav_ids {
        assert!(
            file_ids.contains(wid),
            "expected WAV file id={} to be in track 1 detail files, got file ids: {:?}",
            wid,
            file_ids
        );
    }

    // WAVs with IDs in 20..=24 may appear multiple times due to dual traversal
    // (ISRC match + source_of traversal). Check the SET of unique IDs covers all 5.
    let unique_wav_ids: std::collections::BTreeSet<i64> = files
        .iter()
        .filter_map(|f| f["id"].as_i64())
        .filter(|id| (20..=24).contains(id))
        .collect();

    assert_eq!(
        unique_wav_ids.len(),
        5,
        "expected 5 unique WAV file ids in track detail, got {}: {:?}",
        unique_wav_ids.len(),
        unique_wav_ids
    );

    // Check all 5 expected stem types are present (across all WAV entries)
    let expected_stem_types = ["vocals", "bass", "drums", "instrumental", "other"];
    let unique_stem_types: std::collections::BTreeSet<&str> = files
        .iter()
        .filter(|f| f["id"].as_i64().map_or(false, |id| (20..=24).contains(&id)))
        .filter_map(|f| f["stemType"].as_str())
        .collect();

    for expected in &expected_stem_types {
        assert!(
            unique_stem_types.contains(expected),
            "expected stemType '{}' not found among WAV files, got: {:?}",
            expected,
            unique_stem_types
        );
    }

    // Each WAV should be file_type 'wav' and backed_up
    for f in files
        .iter()
        .filter(|f| f["id"].as_i64().map_or(false, |id| (20..=24).contains(&id)))
    {
        assert_eq!(
            f["fileType"], "wav",
            "WAV file {} has unexpected fileType={:?}",
            f["id"], f["fileType"]
        );
        assert_eq!(
            f["backedUp"], true,
            "WAV file {} should be backedUp=true, got backedUp={:?}",
            f["id"], f["backedUp"]
        );
    }

    // Track 1 should still have the original linked files (ids 1 and 2)
    assert!(
        file_ids.contains(&1),
        "expected file id=1 (FLAC) for US001, got file ids: {:?}",
        file_ids
    );
    assert!(
        file_ids.contains(&2),
        "expected file id=2 (stem.m4a) for US001, got file ids: {:?}",
        file_ids
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: hasLocal / hasBackup
// ═════════════════════════════════════════════════════════════════════════

/// `?hasLocal=true` returns track 1 only.
///
/// Seed: track 1 (ISRC US001) has two linked files (1+2) both with
/// `file_locations.local` entries. track 2 (ISRC US002) has one linked file
/// (3) with backup-only (no local). track 3 has no linked local files.
#[tokio::test]
async fn tracks_filter_has_local_true() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?hasLocal=true", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    assert_eq!(
        tracks.len(),
        1,
        "expected 1 track with hasLocal=true, got {}: {:?}",
        tracks.len(),
        tracks
            .iter()
            .map(|t| format!("id={} title={:?}", t["id"], t["title"]))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        tracks[0]["id"], 1,
        "should be track 1 (linked to local files 1+2)"
    );
    assert_eq!(tracks[0]["title"], "Title One");
}

/// `?hasBackup=true` returns tracks 1 and 2.
///
/// Seed: track 1 (files 1+2, both backed up), track 2 (file 3, backed up).
/// track 3 has no linked files with backup entries.
#[tokio::test]
async fn tracks_filter_has_backup_true() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?hasBackup=true", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    assert_eq!(
        tracks.len(),
        2,
        "expected 2 tracks with hasBackup=true, got {}: {:?}",
        tracks.len(),
        tracks
            .iter()
            .map(|t| format!("id={} title={:?}", t["id"], t["title"]))
            .collect::<Vec<_>>()
    );

    let ids: Vec<i64> = tracks.iter().map(|t| t["id"].as_i64().unwrap()).collect();
    assert!(
        ids.contains(&1),
        "track 1 should be present (files 1+2 backed up)"
    );
    assert!(
        ids.contains(&2),
        "track 2 should be present (file 3 backed up)"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Single track by ID
// ═════════════════════════════════════════════════════════════════════════

/// `GET /api/tracks/{id}` returns a single track object with expected fields.
#[tokio::test]
async fn tracks_single_by_id() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks/1", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let track = &json["data"];

    assert_eq!(track["id"], 1);
    assert_eq!(track["title"], "Title One");
    assert_eq!(track["artist"], "Artist A");
    assert_eq!(track["service"], "spotify");
    assert_eq!(track["isrc"], "US001");
}

// ═════════════════════════════════════════════════════════════════════════
// Error states
// ═════════════════════════════════════════════════════════════════════════

/// `GET /api/tracks/9999` returns 404 with an error message.
#[tokio::test]
async fn tracks_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks/9999", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "expected 404 for non-existent track");

    let json: serde_json::Value = resp.json().await.unwrap();
    assert!(
        json.get("error").is_some(),
        "response should have 'error' field, got: {:#}",
        json
    );
}

/// `POST /api/tracks/needs-comment-count` with `{"trackIds": [1]}` returns
/// the count of selected tracks whose linked files need comment updates.
#[tokio::test]
async fn tracks_needs_comment_count() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .post(format!("{}/api/tracks/needs-comment-count", base))
        .json(&serde_json::json!({"trackIds": [1]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let data = &json["data"];

    // Should have totalTracks, tracksNeedingUpdate, filesNeedingUpdate
    assert!(data.get("totalTracks").is_some(), "totalTracks missing");
    assert!(
        data.get("tracksNeedingUpdate").is_some(),
        "tracksNeedingUpdate missing"
    );
    assert!(
        data.get("filesNeedingUpdate").is_some(),
        "filesNeedingUpdate missing"
    );

    // Track 1 has linked files — at minimum the response should be well-formed
    assert_eq!(data["totalTracks"], 1, "should report 1 total track");

    // tracksNeedingUpdate should be a non-negative integer
    let tracks_needing = data["tracksNeedingUpdate"]
        .as_i64()
        .expect("tracksNeedingUpdate should be an integer");
    assert!(
        tracks_needing >= 0,
        "tracksNeedingUpdate should be >= 0, got {}",
        tracks_needing
    );

    // filesNeedingUpdate should be a non-negative integer
    let files_needing = data["filesNeedingUpdate"]
        .as_i64()
        .expect("filesNeedingUpdate should be an integer");
    assert!(
        files_needing >= 0,
        "filesNeedingUpdate should be >= 0, got {}",
        files_needing
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Mutation: write-comments
// ═════════════════════════════════════════════════════════════════════════

/// `POST /api/tracks/write-comments` with `{"trackIds": [1]}` returns a taskId
/// and file_count for writing comments to linked files.
#[tokio::test]
pub async fn tracks_write_comments() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .post(format!("{}/api/tracks/write-comments", base))
        .json(&serde_json::json!({"trackIds": [1]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let data = &json["data"];
    let task_id = data["taskId"].as_str().unwrap_or("");
    assert!(
        !task_id.is_empty(),
        "write-comments should return non-empty taskId"
    );
    assert!(
        data["fileCount"].as_i64().is_some(),
        "fileCount should be present"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Mutation: needs-refresh-count
// ═════════════════════════════════════════════════════════════════════════

/// `POST /api/tracks/needs-refresh-count` with `{"trackIds": [1]}` returns
/// counts of tracks and files needing a comment refresh.
#[tokio::test]
pub async fn tracks_needs_refresh_count() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .post(format!("{}/api/tracks/needs-refresh-count", base))
        .json(&serde_json::json!({"trackIds": [1]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let data = &json["data"];
    assert!(data.get("totalTracks").is_some(), "totalTracks missing");
    assert!(
        data.get("tracksNeedingRefresh").is_some(),
        "tracksNeedingRefresh missing"
    );
    assert!(data.get("filesTotal").is_some(), "filesTotal missing");
    assert!(
        data.get("filesNeedingRefresh").is_some(),
        "filesNeedingRefresh missing"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Mutation: refresh-comments
// ═════════════════════════════════════════════════════════════════════════

/// `POST /api/tracks/refresh-comments` with `{"trackIds": [1]}` refreshes
/// comments and returns refreshed/file counts.
#[tokio::test]
pub async fn tracks_refresh_comments() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .post(format!("{}/api/tracks/refresh-comments", base))
        .json(&serde_json::json!({"trackIds": [1]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let data = &json["data"];
    assert!(
        data.get("refreshedCount").is_some(),
        "refreshedCount missing"
    );
    assert!(data.get("fileCount").is_some(), "fileCount missing");
}

// ═════════════════════════════════════════════════════════════════════════
// Mutation: backpack toggle
// ═════════════════════════════════════════════════════════════════════════

/// `POST /api/tracks/{id}/backpack` toggles the backpack status of a track.
/// First call adds to backpack (inBackpack=true), second call removes (inBackpack=false).
#[tokio::test]
pub async fn tracks_backpack_toggle() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    // First call: add to backpack
    let resp = client
        .post(format!("{}/api/tracks/1/backpack", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "toggle backpack should return 200");
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        json["data"]["inBackpack"].as_bool(),
        Some(true),
        "first toggle should add to backpack"
    );

    // Verify via detail endpoint
    let detail_resp = client
        .get(format!("{}/api/tracks/1/detail", base))
        .send()
        .await
        .unwrap();
    assert_eq!(detail_resp.status(), 200);
    let detail_json: serde_json::Value = detail_resp.json().await.unwrap();
    assert_eq!(
        detail_json["data"]["inBackpack"].as_bool(),
        Some(true),
        "track 1 should be in backpack"
    );

    // Second call: remove from backpack
    let resp2 = client
        .post(format!("{}/api/tracks/1/backpack", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);
    let json2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(
        json2["data"]["inBackpack"].as_bool(),
        Some(false),
        "second toggle should remove from backpack"
    );

    // Verify via detail
    let detail_resp2 = client
        .get(format!("{}/api/tracks/1/detail", base))
        .send()
        .await
        .unwrap();
    let detail_json2: serde_json::Value = detail_resp2.json().await.unwrap();
    assert_eq!(
        detail_json2["data"]["inBackpack"].as_bool(),
        Some(false),
        "track 1 should no longer be in backpack"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: pmvCategories
// ═════════════════════════════════════════════════════════════════════════

/// `?pmvCategories=m` filters tracks whose resolved tags include a Mood (prefix 'm') tag.
/// Track 1 has tag "Groovy" (Mood, prefix 'm'); track 2 has tag "Deep" (Mood, prefix 'm').
#[tokio::test]
pub async fn tracks_filter_pmv_categories() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?pmvCategories=m&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    // Track 1 and 2 have Mood tags, track 3 has no playlist link
    assert!(
        !tracks.is_empty(),
        "expected at least 1 track with Mood tag"
    );

    let ids: Vec<i64> = tracks.iter().map(|t| t["id"].as_i64().unwrap()).collect();
    assert!(ids.contains(&1), "track 1 should match Mood category");
    // Track 2's playlist is "Deep Mix" - there is no Mood tag named "Deep Mix" (only "Deep")
    // so track 2 won't match. Only track 1 ("Groovy" playlist → "Groovy" Mood tag) matches.
    assert!(
        !ids.contains(&3),
        "track 3 should NOT match Mood category (no playlist)"
    );

    // Count parity
    let count_resp = client
        .get(format!("{}/api/tracks/count?pmvCategories=m", base))
        .send()
        .await
        .unwrap();
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        count as usize,
        tracks.len(),
        "count should match list length"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: pmvAggregate=full
// ═════════════════════════════════════════════════════════════════════════

/// `?pmvAggregate=full` returns tracks with all three PMV categories (Phase, Mood, Vibe).
/// With basic seed data, no track has all three, so returns 0.
#[tokio::test]
pub async fn tracks_filter_pmv_aggregate() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?pmvAggregate=full&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    // No track has all three PMV categories with basic seed data
    // Track 1: Mood only; Track 2: Mood only; Track 3: none
    assert_eq!(
        tracks.len(),
        0,
        "no track should match pmvAggregate=full with basic seed data"
    );

    // Count parity
    let count_resp = client
        .get(format!("{}/api/tracks/count?pmvAggregate=full", base))
        .send()
        .await
        .unwrap();
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        count as usize,
        tracks.len(),
        "count should match list length"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: fileTypes
// ═════════════════════════════════════════════════════════════════════════

/// `?fileTypes=flac` returns tracks whose linked files include a FLAC version.
#[tokio::test]
pub async fn tracks_filter_file_types() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?fileTypes=flac&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();

    // Track 1 -> files 1 (flac) and 2 (stem.m4a)
    // Track 2 -> file 3 (flac)
    // Track 3 -> no linked files
    let ids: Vec<i64> = tracks.iter().map(|t| t["id"].as_i64().unwrap()).collect();
    assert!(ids.contains(&1), "track 1 has a FLAC file");
    assert!(ids.contains(&2), "track 2 has a FLAC file");
    assert!(!ids.contains(&3), "track 3 has no file");

    // Count parity
    let count_resp = client
        .get(format!("{}/api/tracks/count?fileTypes=flac", base))
        .send()
        .await
        .unwrap();
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        count as usize,
        tracks.len(),
        "count should match list length"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: fileTypeAgg
// ═════════════════════════════════════════════════════════════════════════

/// `?fileTypeAgg=any` returns tracks that have at least one linked file.
/// `?fileTypeAgg=none` returns tracks with no linked files.
#[tokio::test]
pub async fn tracks_filter_file_type_agg() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    // fileTypeAgg=any — tracks with at least one linked file
    let resp = client
        .get(format!("{}/api/tracks?fileTypeAgg=any&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    assert!(!tracks.is_empty(), "expected some tracks with linked files");

    // Count parity
    let count_resp = client
        .get(format!("{}/api/tracks/count?fileTypeAgg=any", base))
        .send()
        .await
        .unwrap();
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        count as usize,
        tracks.len(),
        "count should match list length"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: importedAfterDays
// ═════════════════════════════════════════════════════════════════════════

/// `?importedAfterDays=365` returns tracks imported within the last 365 days.
/// Seeded tracks have imported_at=1700000000 (Nov 2023), so they won't match.
#[tokio::test]
pub async fn tracks_filter_date_imported() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!(
            "{}/api/tracks?importedAfterDays=365&limit=10",
            base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    // All seeded tracks are from 2023, not in the last 365 days
    assert_eq!(
        tracks.len(),
        0,
        "no seeded tracks were imported in the last 365 days"
    );

    // Count parity
    let count_resp = client
        .get(format!("{}/api/tracks/count?importedAfterDays=365", base))
        .send()
        .await
        .unwrap();
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        count as usize,
        tracks.len(),
        "count should match list length"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: addedAfterDays
// ═════════════════════════════════════════════════════════════════════════

/// `?addedAfterDays=365` returns tracks whose latest playlist add was within last 365 days.
/// Seeded spt rows have added_at=1700000000 (Nov 2023), so they won't match.
#[tokio::test]
pub async fn tracks_filter_date_added() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?addedAfterDays=365&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();
    // All seeded spt.added_at values are 1700000000 (2023)
    assert_eq!(
        tracks.len(),
        0,
        "no seeded tracks were added in the last 365 days"
    );

    // Count parity
    let count_resp = client
        .get(format!("{}/api/tracks/count?addedAfterDays=365", base))
        .send()
        .await
        .unwrap();
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        count as usize,
        tracks.len(),
        "count should match list length"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Filter: playlistId (single playlist param)
// ═════════════════════════════════════════════════════════════════════════

/// `?playlistId=1` returns tracks belonging to playlist 1 ("Groovy").
#[tokio::test]
pub async fn tracks_filter_playlist_id() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!("{}/api/tracks?playlistId=1&limit=10", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();

    // Playlist 1 has track 1
    let ids: Vec<i64> = tracks.iter().map(|t| t["id"].as_i64().unwrap()).collect();
    assert!(ids.contains(&1), "track 1 should be in playlist 1");
    assert!(!ids.contains(&2), "track 2 should NOT be in playlist 1");

    // Count parity
    let count_resp = client
        .get(format!("{}/api/tracks/count?playlistId=1", base))
        .send()
        .await
        .unwrap();
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        count as usize,
        tracks.len(),
        "count should match list length"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Filter combinations
// ═════════════════════════════════════════════════════════════════════════

/// `?hasLocal=true&hasBackup=true` returns tracks that have both local and backup files.
#[tokio::test]
pub async fn tracks_filter_has_local_and_backup() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!(
            "{}/api/tracks?hasLocal=true&hasBackup=true&limit=10",
            base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();

    // Track 1 -> files 1 (local+backup) and 2 (local+backup) — has both local and backup
    // Track 2 -> file 3 (backup only) — no local, so excluded
    // Track 3 -> no files, excluded
    let ids: Vec<i64> = tracks.iter().map(|t| t["id"].as_i64().unwrap()).collect();
    assert!(ids.contains(&1), "track 1 has both local+backup files");
    assert!(!ids.contains(&2), "track 2 has backup only, no local file");

    // Count parity
    let count_resp = client
        .get(format!(
            "{}/api/tracks/count?hasLocal=true&hasBackup=true",
            base
        ))
        .send()
        .await
        .unwrap();
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        count as usize,
        tracks.len(),
        "count should match list length"
    );
}

/// `?pmvCategories=m,v&hasLocal=true` returns local tracks with Mood or Vibe tags.
#[tokio::test]
pub async fn tracks_filter_pmv_and_local() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_track_resolved_tags(&pool).await.unwrap();

    let resp = client
        .get(format!(
            "{}/api/tracks?pmvCategories=m,v&hasLocal=true&limit=10",
            base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let tracks = json["data"].as_array().unwrap();

    // Track 1 has Mood tag (Groovy) and linked file 1 (local) — matches both conditions
    // Track 2 has Mood tag (Deep) but linked file 3 is NOT local — excluded by hasLocal
    // Track 3 has no tags — excluded by pmvCategories
    let ids: Vec<i64> = tracks.iter().map(|t| t["id"].as_i64().unwrap()).collect();
    assert!(
        ids.contains(&1),
        "track 1 should match (Mood tag + local files)"
    );

    // Count parity
    let count_resp = client
        .get(format!(
            "{}/api/tracks/count?pmvCategories=m,v&hasLocal=true",
            base
        ))
        .send()
        .await
        .unwrap();
    let count_json: serde_json::Value = count_resp.json().await.unwrap();
    let count = count_json["data"].as_i64().unwrap();
    assert_eq!(
        count as usize,
        tracks.len(),
        "count should match list length"
    );
}
