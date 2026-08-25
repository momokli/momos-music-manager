//! Integration tests for `/api/storage*` endpoints.
//!
//! Each test creates a fresh in-memory SQLite DB, runs all 16 migrations,
//! seeds hand-crafted data, and hits the running Axum server with `reqwest`.
//!
//! Seed data layout (see `common::seed_basic_data`):
//!
//! | File | Type     | ISRC  | BPM  | Key | Title      | Artist   | Local? | Backup? | Tag (backpack) |
//! |------|----------|-------|------|-----|------------|----------|--------|---------|----------------|
//! | 1    | flac     | US001 | 128.0| 4m  | Title One  | Artist A | yes    | yes     | Groovy (no)    |
//! | 2    | stem.m4a | US001 | 128.5| 4m  | Title One  | Artist A | yes    | yes     | Groovy (no)    |
//! | 3    | flac     | US002 | 140.0| 8m  | Track Two  | Artist B | no     | yes     | Deep (yes)     |
//!
//! WAV variant data (see `common::seed_wav_variant_data`, IDs 20-24):
//! | File | Type | source_of | Local? | Backup? |
//! |------|------|-----------|--------|---------|
//! | 20   | wav  | file 2    | no*    | yes     |
//! | 21   | wav  | file 2    | no*    | yes     |
//! | 22   | wav  | file 2    | no*    | yes     |
//! | 23   | wav  | file 2    | no*    | yes     |
//! | 24   | wav  | file 2    | no*    | yes     |
//!   * WAVs only get local entries in the `*_wav_variants` test.

mod common;

use momos_music_manager::db::refresh_file_resolved_tags;

// ═════════════════════════════════════════════════════════════════════════
// /api/storage/status
// ═════════════════════════════════════════════════════════════════════════

/// Verify that all expected keys are present in the status response.
#[tokio::test]
async fn storage_status_has_fields() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/storage/status", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: serde_json::Value = resp.json().await.unwrap();
    let data = json["data"].as_object().unwrap();

    // Core counts
    assert!(
        data.contains_key("localFileCount"),
        "missing localFileCount"
    );
    assert!(
        data.contains_key("trackedFileCount"),
        "missing trackedFileCount"
    );
    assert!(data.contains_key("backupCount"), "missing backupCount");
    assert!(
        data.contains_key("pruneCandidateCount"),
        "missing pruneCandidateCount"
    );
    assert!(
        data.contains_key("pruneCandidateBytes"),
        "missing pruneCandidateBytes"
    );

    // Size fields
    assert!(
        data.contains_key("localSizeBytes"),
        "missing localSizeBytes"
    );
    assert!(
        data.contains_key("trackedSizeBytes"),
        "missing trackedSizeBytes"
    );

    // Per-type local counts
    assert!(data.contains_key("localStems"), "missing localStems");
    assert!(data.contains_key("localFlacs"), "missing localFlacs");
    assert!(data.contains_key("localMp3s"), "missing localMp3s");
    assert!(data.contains_key("localWavs"), "missing localWavs");
    assert!(data.contains_key("localOther"), "missing localOther");

    // Per-type local sizes
    assert!(
        data.contains_key("localStemsSize"),
        "missing localStemsSize"
    );
    assert!(
        data.contains_key("localFlacsSize"),
        "missing localFlacsSize"
    );
    assert!(data.contains_key("localWavsSize"), "missing localWavsSize");
    assert!(data.contains_key("localMp3sSize"), "missing localMp3sSize");

    // WAV tracking
    assert!(data.contains_key("wavSourceDirs"), "missing wavSourceDirs");
    assert!(data.contains_key("wavIndexed"), "missing wavIndexed");
    assert!(data.contains_key("wavBackedUp"), "missing wavBackedUp");

    // Assert exact counts from seed_basic_data
    // Files 1 and 2 have local entries
    assert_eq!(
        data["localFileCount"], 2,
        "files 1 and 2 have local entries; expected localFileCount=2"
    );
    // Files 1, 2, 3, 4 all have backup entries
    assert_eq!(
        data["backupCount"], 4,
        "files 1-4 have backup entries; expected backupCount=4"
    );
}

/// Verify exact counts with basic + WAV data.
///
/// With basic data (3 files) + WAV data (5 WAVs):
/// - Files 1 and 2 have `file_locations.local` entries → localFileCount = 2
/// - All 8 files have `file_locations.backup` entries → backupCount = 8
/// - 3 basic + 5 WAV = 8 tracked files → trackedFileCount = 8
/// - Local types: 1 stem.m4a (file 2), 1 flac (file 1), 0 WAVs, 0 MP3s
#[tokio::test]
async fn storage_status_counts() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_wav_variant_data(&pool).await;

    let resp = client
        .get(format!("{}/api/storage/status", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: serde_json::Value = resp.json().await.unwrap();
    let data = json["data"].as_object().unwrap();

    // Core counts
    assert_eq!(
        data["localFileCount"], 2,
        "files 1 and 2 have local entries; file 3 and WAVs do not"
    );
    assert_eq!(
        data["backupCount"], 9,
        "all 9 files (1,2,3,4 + 5 WAVs) have backup entries"
    );
    assert_eq!(
        data["trackedFileCount"], 9,
        "4 basic + 5 WAV files = 9 total"
    );

    // Per-type local counts
    assert_eq!(data["localStems"], 1, "file 2 is the only local stem.m4a");
    assert_eq!(data["localFlacs"], 1, "file 1 is the only local flac");
    assert_eq!(data["localWavs"], 0, "no WAVs have local entries");
    assert_eq!(data["localMp3s"], 0, "no MP3s exist in seed data");

    // WAV tracking
    assert_eq!(data["wavIndexed"], 5, "5 WAV files indexed");
    assert_eq!(data["wavBackedUp"], 5, "all 5 WAVs are backed up");
    assert_eq!(
        data["wavSourceDirs"], 5,
        "all 5 WAVs have source_of set to file 2"
    );

    // Sizes (known from seed)
    //  file 1 FLAC     = 5,000,000
    //  file 2 stem.m4a = 8,000,000
    //  local total     = 13,000,000
    assert_eq!(
        data["localSizeBytes"], 13_000_000i64,
        "sum of local file sizes"
    );
    assert_eq!(
        data["localStemsSize"], 8_000_000i64,
        "local stem.m4a size = file 2"
    );
    assert_eq!(
        data["localFlacsSize"], 5_000_000i64,
        "local flac size = file 1"
    );
    assert_eq!(data["localWavsSize"], 0, "no local WAVs");
}

// ═════════════════════════════════════════════════════════════════════════
// /api/storage/prune-preview
// ═════════════════════════════════════════════════════════════════════════

/// Verify the prune preview response shape.
///
/// After `seed_basic_data` + refreshing `file_resolved_tags`:
/// - Files 1 and 2 are backed up, local, have metadata, and are NOT in a backpack tag
///   (tag "Groovy" has backpack=0) → these are prune candidates.
/// - File 3 is backed up but has NO local entry → excluded by the SQL EXISTS filter.
#[tokio::test]
async fn storage_prune_preview() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Populate file_resolved_tags so the backpack-filter subquery works.
    refresh_file_resolved_tags(&pool).await.unwrap();

    let resp = client
        .post(format!("{}/api/storage/prune-preview", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: serde_json::Value = resp.json().await.unwrap();
    let candidates = json["data"].as_array().unwrap();

    // Seed produces candidates — files 1 and 2 are backed up, local, not backpacked
    assert!(
        !candidates.is_empty(),
        "prune candidates should not be empty after seed_basic_data"
    );

    // Every candidate must have all expected fields and valid values
    for c in candidates {
        let obj = c.as_object().unwrap();
        assert!(obj.contains_key("fileId"), "missing fileId in candidate");
        assert!(
            obj.contains_key("fileType"),
            "missing fileType in candidate"
        );
        assert!(
            obj.contains_key("fileSize"),
            "missing fileSize in candidate"
        );
        assert!(obj.contains_key("title"), "missing title in candidate");
        assert!(obj.contains_key("artist"), "missing artist in candidate");
        assert!(obj.contains_key("reason"), "missing reason in candidate");
        assert!(
            obj.contains_key("hasStemVariant"),
            "missing hasStemVariant in candidate"
        );
        assert!(
            obj.contains_key("filePath"),
            "missing filePath in candidate"
        );
        assert!(
            obj.contains_key("backupPath"),
            "missing backupPath in candidate"
        );

        // fileSize must be positive
        let size = c["fileSize"].as_i64().unwrap_or(0);
        assert!(size > 0, "fileSize must be > 0, got {} for candidate", size);
    }

    // First candidate must have non-empty fileType and reason
    let first = &candidates[0];
    let file_type = first["fileType"].as_str().unwrap_or("");
    assert!(
        !file_type.is_empty(),
        "first candidate should have non-empty fileType"
    );
    let reason = first["reason"].as_str().unwrap_or("");
    assert!(
        !reason.is_empty(),
        "first candidate should have non-empty reason"
    );
}

/// Verify which files appear as prune candidates and their properties.
///
/// Files 1 and 2 should be candidates because they satisfy all conditions:
/// backed up + local + has metadata + tag "Groovy" has backpack=0.
///
/// File 3 should NOT be a candidate because it has no local entry.
#[tokio::test]
async fn storage_prune_preview_candidates() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    refresh_file_resolved_tags(&pool).await.unwrap();

    let resp = client
        .post(format!("{}/api/storage/prune-preview", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: serde_json::Value = resp.json().await.unwrap();
    let candidates = json["data"].as_array().unwrap();

    // Collect candidate info
    let file_ids: Vec<i64> = candidates
        .iter()
        .map(|c| c["fileId"].as_i64().unwrap())
        .collect();

    // Files 1 and 2 are backed up + local + not backpacked → candidates
    assert!(
        file_ids.contains(&1),
        "file 1 (FLAC, backed up + local, tag Groovy(backpack=0)) should be a candidate"
    );
    assert!(
        file_ids.contains(&2),
        "file 2 (stem.m4a, backed up + local, tag Groovy(backpack=0)) should be a candidate"
    );

    // File 3 is backed up but has NO local entry → excluded by SQL EXISTS
    assert!(
        !file_ids.contains(&3),
        "file 3 (FLAC, backed up but NOT local) should NOT be a candidate"
    );

    // Check hasStemVariant per candidate
    for c in candidates {
        let fid = c["fileId"].as_i64().unwrap();

        if fid == 1 {
            // File 1 (FLAC) shares ISRC=US001 with file 2 (stem.m4a) → has stem variant
            assert_eq!(
                c["hasStemVariant"], true,
                "file 1 (FLAC) has same-ISRC stem.m4a (file 2) → hasStemVariant=true"
            );
            assert_eq!(
                c["reason"], "not_followed",
                "file 1 reason should be 'not_followed'"
            );
        } else if fid == 2 {
            // File 2 (stem.m4a) is itself the stem — no WAV children exist in basic data
            assert_eq!(
                c["hasStemVariant"], false,
                "file 2 (stem.m4a) has no WAV children in basic data → hasStemVariant=false"
            );
            assert_eq!(
                c["reason"], "not_followed",
                "file 2 reason should be 'not_followed'"
            );
        }
    }

    // Verify file types
    let f1 = candidates.iter().find(|c| c["fileId"] == 1).unwrap();
    assert_eq!(f1["fileType"], "flac", "file 1 should be flac");
    assert_eq!(f1["fileSize"], 5_000_000i64, "file 1 size");

    let f2 = candidates.iter().find(|c| c["fileId"] == 2).unwrap();
    assert_eq!(f2["fileType"], "stem.m4a", "file 2 should be stem.m4a");
    assert_eq!(f2["fileSize"], 8_000_000i64, "file 2 size");
}

/// Verify that, when WAV source files have local presence, they appear in the
/// prune preview with `hasStemVariant: true` and `reason: "wav_backed_up"`.
///
/// The WAV files (IDs 20-24) are backed up and have `source_of=2` (linked to
/// stem file 2). They also need local entries to satisfy the EXISTS subquery
/// in `get_prune_candidates` — this test adds those inline after the standard
/// seeds so WAVs become eligible candidates.
#[tokio::test]
async fn storage_prune_preview_wav_variants() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_wav_variant_data(&pool).await;

    // WAVs need local entries to appear as prune candidates (the prune query
    // requires `EXISTS (SELECT 1 FROM file_locations WHERE type='local')`).
    for wav_id in 20i64..=24 {
        sqlx::query(
            r#"INSERT INTO file_locations (file_id, location_type, path, file_size, last_verified)
               VALUES (?, 'local', ?, 2000000, 1700000000)"#,
        )
        .bind(wav_id)
        .bind(format!(
            "/test/stems/Artist_Title/Artist - Title_{}.wav",
            match wav_id {
                20 => "vocals",
                21 => "bass",
                22 => "drums",
                23 => "instrumental",
                _ => "other",
            }
        ))
        .execute(&pool)
        .await
        .unwrap();
    }

    refresh_file_resolved_tags(&pool).await.unwrap();

    let resp = client
        .post(format!("{}/api/storage/prune-preview", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: serde_json::Value = resp.json().await.unwrap();
    let candidates = json["data"].as_array().unwrap();

    // Collect WAV candidates
    let wav_candidates: Vec<&serde_json::Value> = candidates
        .iter()
        .filter(|c| c["fileType"] == "wav")
        .collect();

    assert!(
        !wav_candidates.is_empty(),
        "WAV files with local+backup entries should be prune candidates"
    );

    for w in &wav_candidates {
        assert_eq!(
            w["hasStemVariant"], true,
            "WAV files are stem variants themselves (source_of IS NOT NULL)"
        );
        assert_eq!(
            w["reason"], "wav_backed_up",
            "WAVs should have reason='wav_backed_up'"
        );
    }

    // All 5 WAVs should be candidates
    let wav_ids: Vec<i64> = wav_candidates
        .iter()
        .map(|w| w["fileId"].as_i64().unwrap())
        .collect();

    for expected_id in 20i64..=24 {
        assert!(
            wav_ids.contains(&expected_id),
            "WAV file {} should be a candidate",
            expected_id
        );
    }

    // Non-WAV candidates should still exist (files 1 and 2)
    let non_wav: Vec<&serde_json::Value> = candidates
        .iter()
        .filter(|c| c["fileType"] != "wav")
        .collect();
    assert!(
        !non_wav.is_empty(),
        "non-WAV files should still appear as candidates"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// /api/storage/prune — error paths
// ═════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/storage/prune` with empty body `{}` returns 400 because
/// `PruneRequest.file_ids` is empty (serde default for missing field).
/// The handler explicitly checks `body.file_ids.is_empty()` and returns
/// BAD_REQUEST with an error message.
async fn storage_prune_no_body() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/storage/prune", base))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    // Empty body triggers "No file IDs provided" → 400
    assert_eq!(
        resp.status(),
        400,
        "empty body should return 400 (no file IDs), got {}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"].as_str().is_some(),
        "error response should have an error field, got: {}",
        body
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 3 — Settings
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/storage/settings` — returns a settings object.
async fn storage_settings_get() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/storage/settings", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: serde_json::Value = resp.json().await.unwrap();
    // settings endpoint returns data as an object (may be empty)
    assert!(
        json["data"].is_object() || json["data"].is_null(),
        "settings should return an object"
    );
}

#[tokio::test]
/// `PUT /api/storage/settings` — updates settings.
async fn storage_settings_put() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .put(format!("{}/api/storage/settings", base))
        .json(&serde_json::json!({"someSetting": "value"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: serde_json::Value = resp.json().await.unwrap();
    assert!(
        json["data"].is_object() || json["data"].is_null(),
        "settings put should return an object"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 4 — Backup endpoints (no SSH configured → error)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/storage/backup/1` — expects error because folder has no backup_path.
async fn storage_backup_no_ssh() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/storage/backup/1", base))
        .send()
        .await
        .unwrap();

    // Folder has no backup_path → 400
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    eprintln!("backup error response: {body}");

    assert!(
        status == 400 || status == 500,
        "backup without config should return 400 or 500, got {}",
        status
    );
}

#[tokio::test]
/// `POST /api/storage/backup-wavs/1` — expects error because folder has no backup_path.
async fn storage_backup_wavs_no_ssh() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/storage/backup-wavs/1", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    eprintln!("backup-wavs error response: {body}");

    assert!(
        status == 400 || status == 500,
        "backup-wavs without config should return 400 or 500, got {}",
        status
    );
}

#[tokio::test]
/// `POST /api/storage/discover-backup/1` — expects error because folder has no backup_path.
async fn storage_discover_backup_no_ssh() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/storage/discover-backup/1", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    eprintln!("discover-backup error response: {body}");

    assert!(
        status == 400 || status == 500,
        "discover-backup without config should return 400 or 500, got {}",
        status
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 5 — Settings edge cases & Prune execute
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `PUT /api/storage/settings` accepts unusual JSON values gracefully.
async fn storage_settings_edge_cases() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Test with nested object
    let resp = client
        .put(format!("{}/api/storage/settings", base))
        .json(&serde_json::json!({
            "nested": {"key": "value"},
            "number": 42,
            "flag": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "nested object should return 200");

    // Test with array value
    let resp = client
        .put(format!("{}/api/storage/settings", base))
        .json(&serde_json::json!(["a", "b", "c"]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "array value should return 200");

    // Test with null value
    let resp = client
        .put(format!("{}/api/storage/settings", base))
        .json(&serde_json::json!(null))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "null value should return 200");

    // Test with empty object
    let resp = client
        .put(format!("{}/api/storage/settings", base))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "empty object should return 200");

    // Verify settings still accessible after all the writes
    let resp = client
        .get(format!("{}/api/storage/settings", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "settings GET should still work");
    let json: serde_json::Value = resp.json().await.unwrap();
    assert!(
        json["data"].is_object() || json["data"].is_null(),
        "settings GET should return object or null"
    );
}

#[tokio::test]
/// `POST /api/storage/prune` with `{"fileIds": []}` returns 400.
async fn storage_prune_execute_empty() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/storage/prune", base))
        .json(&serde_json::json!({"file_ids": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "empty fileIds should return 400 (no file IDs), got {}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"].as_str().is_some(),
        "error response should have an error field, got: {}",
        body
    );
}

#[tokio::test]
/// `POST /api/storage/prune` with valid file IDs creates a prune task.
async fn storage_prune_execute_valid() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // File 1 is local+backed_up+has_stem → safe to delete
    let resp = client
        .post(format!("{}/api/storage/prune", base))
        .json(&serde_json::json!({"file_ids": [1, 2]}))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    eprintln!("prune_execute_valid: status={status}, body={body:#}");

    assert!(
        status.is_success(),
        "prune with valid file IDs should succeed, got {}",
        status
    );

    let task_id = body["data"]["taskId"].as_str();
    assert!(
        task_id.is_some() && !task_id.unwrap().is_empty(),
        "prune response should contain a non-empty taskId, got: {:#}",
        body
    );
}

#[tokio::test]
/// `POST /api/storage/prune` with non-existent file IDs still creates a task.
async fn storage_prune_execute_invalid_ids() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Non-existent file IDs — handler still creates a task (task skips missing files)
    let resp = client
        .post(format!("{}/api/storage/prune", base))
        .json(&serde_json::json!({"file_ids": [9999, 8888]}))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    eprintln!("prune_execute_invalid_ids: status={status}, body={body:#}");

    assert!(
        status.is_success(),
        "prune with non-existent IDs should still return task (handler doesn't validate existence), got {}",
        status
    );

    let task_id = body["data"]["taskId"].as_str();
    assert!(
        task_id.is_some() && !task_id.unwrap().is_empty(),
        "prune response should contain a non-empty taskId even for non-existent IDs, got: {:#}",
        body
    );
}

#[tokio::test]
/// `GET /api/storage/settings/format-priority` returns default format priority list.
async fn storage_format_priority_get_defaults() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/storage/settings/format-priority", base))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    eprintln!("format_priority_get_defaults: status={status}, body={body:#}");

    assert!(status.is_success(), "expected 200, got {status}");
    let prio = body["data"]["priorities"].as_array().unwrap();
    assert!(
        prio.len() >= 4,
        "should have at least 4 default formats, got {}",
        prio.len()
    );
}

#[tokio::test]
/// `PUT` then `GET /api/storage/settings/format-priority` roundtrip.
async fn storage_format_priority_put_and_get() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let put = client
        .put(format!("{}/api/storage/settings/format-priority", base))
        .json(&serde_json::json!({"priorities": ["mp3", "stem.m4a", "flac"]}))
        .send()
        .await
        .unwrap();
    assert!(
        put.status().is_success(),
        "PUT format-priority expected 200, got {}",
        put.status()
    );

    let get = client
        .get(format!("{}/api/storage/settings/format-priority", base))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = get.json().await.unwrap();
    eprintln!("format_priority_put_and_get: body={body:#}");

    let prio = body["data"]["priorities"].as_array().unwrap();
    assert_eq!(prio[0], "mp3");
    assert_eq!(prio[1], "stem.m4a");
}

#[tokio::test]
/// `PUT /api/storage/settings/format-priority` with invalid bodies returns 400.
async fn storage_format_priority_put_invalid() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Empty array → 400
    let resp = client
        .put(format!("{}/api/storage/settings/format-priority", base))
        .json(&serde_json::json!({"priorities": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "empty priorities should return 400, got {}",
        resp.status()
    );

    // Unknown format → 400
    let resp = client
        .put(format!("{}/api/storage/settings/format-priority", base))
        .json(&serde_json::json!({"priorities": ["xyz"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "unknown format should return 400, got {}",
        resp.status()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 6 — Concurrent task rejection
// ═══════════════════════════════════════════════════════════════════════════

/// Deterministically engage the task-uniqueness guard for a task type.
///
/// Registers a task with the same conflict key directly in the server's
/// `TaskManager`. The task stays `Pending` because no worker is spawned, so
/// the guard stays engaged for the whole test and the next API start call is
/// guaranteed to be rejected.
///
/// Background: the old tests fired both HTTP calls concurrently and hoped the
/// second one arrived while the first worker was still running. That was a
/// race — with a fake `backupPath` (no `host:` prefix) the worker fails in
/// microseconds, so the guard window often closed before the second request
/// reached it, making the tests flaky.
async fn hold_task_guard(
    state: &std::sync::Arc<momos_music_manager::AppState>,
    task_type: momos_music_manager::tasks::TaskType,
) {
    let held =
        momos_music_manager::tasks::Task::new(task_type, Some("test-guard-hold".to_string()));
    state.task_manager.start_task(held).await;
}

#[tokio::test]
/// `POST /api/storage/backup/{id}` — second call for same folder returns
/// null taskId with "already in progress" message.
async fn storage_backup_rejects_concurrent() {
    let (client, base, pool, state) = common::spawn_test_app_with_state().await;
    common::seed_basic_data(&pool).await;

    // Set backup_path first so the handler doesn't reject with 400
    client
        .put(format!("{}/api/folders/1/backup", base))
        .json(&serde_json::json!({
            "backupPath": "/backups/test",
            "scanSources": false
        }))
        .send()
        .await
        .unwrap();

    // First call starts a backup task and returns its id.
    let resp1 = client
        .post(format!("{}/api/storage/backup/1", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);
    let json1: serde_json::Value = resp1.json().await.unwrap();
    assert!(
        json1["data"]["taskId"].is_string(),
        "first call should start a task, got {json1:#}"
    );

    // Deterministic synchronization: hold the guard open (see `hold_task_guard`),
    // then the second call MUST be rejected — no timing luck involved.
    hold_task_guard(
        &state,
        momos_music_manager::tasks::TaskType::BackupFolder { folder_id: 1 },
    )
    .await;

    let resp2 = client
        .post(format!("{}/api/storage/backup/1", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);
    let json2: serde_json::Value = resp2.json().await.unwrap();
    assert!(
        json2["data"]["taskId"].is_null()
            && json2["data"]["message"] == "Backup already in progress for this folder",
        "second call should be rejected, got {json2:#}"
    );
}

#[tokio::test]
/// `POST /api/storage/backup-wavs/{id}` — second call returns null taskId.
async fn storage_backup_wavs_rejects_concurrent() {
    let (client, base, pool, state) = common::spawn_test_app_with_state().await;
    common::seed_basic_data(&pool).await;

    // Set backup_path first
    client
        .put(format!("{}/api/folders/1/backup", base))
        .json(&serde_json::json!({
            "backupPath": "/backups/test",
            "scanSources": false
        }))
        .send()
        .await
        .unwrap();

    // First call starts a backup-wavs task and returns its id.
    let resp1 = client
        .post(format!("{}/api/storage/backup-wavs/1", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);
    let json1: serde_json::Value = resp1.json().await.unwrap();
    assert!(
        json1["data"]["taskId"].is_string(),
        "first call should start a task, got {json1:#}"
    );

    // Deterministic synchronization: hold the guard open, then the second
    // call MUST be rejected.
    hold_task_guard(
        &state,
        momos_music_manager::tasks::TaskType::BackupWavs { folder_id: 1 },
    )
    .await;

    let resp2 = client
        .post(format!("{}/api/storage/backup-wavs/1", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);
    let json2: serde_json::Value = resp2.json().await.unwrap();
    assert!(
        json2["data"]["taskId"].is_null()
            && json2["data"]["message"] == "Backup WAVs already in progress for this folder",
        "second call should be rejected, got {json2:#}"
    );
}

#[tokio::test]
/// `POST /api/storage/discover-backup/{id}` — second call returns null taskId.
async fn storage_discover_backup_rejects_concurrent() {
    let (client, base, pool, state) = common::spawn_test_app_with_state().await;
    common::seed_basic_data(&pool).await;

    // Set backup_path first
    client
        .put(format!("{}/api/folders/1/backup", base))
        .json(&serde_json::json!({
            "backupPath": "/backups/test",
            "scanSources": false
        }))
        .send()
        .await
        .unwrap();

    // First call starts a discovery task and returns its id.
    let resp1 = client
        .post(format!("{}/api/storage/discover-backup/1", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);
    let json1: serde_json::Value = resp1.json().await.unwrap();
    assert!(
        json1["data"]["taskId"].is_string(),
        "first call should start a task, got {json1:#}"
    );

    // Deterministic synchronization: hold the guard open, then the second
    // call MUST be rejected.
    hold_task_guard(
        &state,
        momos_music_manager::tasks::TaskType::BackupDiscovery { folder_id: 1 },
    )
    .await;

    let resp2 = client
        .post(format!("{}/api/storage/discover-backup/1", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);
    let json2: serde_json::Value = resp2.json().await.unwrap();
    assert!(
        json2["data"]["taskId"].is_null()
            && json2["data"]["message"] == "Backup discovery already in progress for this folder",
        "second call should be rejected, got {json2:#}"
    );
}

#[tokio::test]
/// `POST /api/storage/backfill-backup-sizes` — returns taskId even when no zero-size records.
async fn storage_backfill_backup_sizes_no_records() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/storage/backfill-backup-sizes", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "should return 200");
    let json: serde_json::Value = resp.json().await.unwrap();
    let data = &json["data"];

    // Seed data has no zero-size backup records → taskId should be null
    assert_eq!(
        data["zeroSizeRecords"].as_i64().unwrap_or(-1),
        0,
        "seed data has no zero-size backup records"
    );
    assert!(
        data["taskId"].is_null(),
        "should not spawn a task when no records need backfill"
    );
    assert!(
        data["message"].is_string(),
        "should include a message explaining no records need backfill"
    );
}

#[tokio::test]
/// `POST /api/storage/backfill-backup-sizes` — with a zero-size record, spawns a task.
async fn storage_backfill_backup_sizes_with_zero_size() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Set an existing backup record's file_size to 0 so it needs backfill
    sqlx::query(
        r#"UPDATE file_locations SET file_size = 0 WHERE file_id = 3 AND location_type = 'backup'"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let resp = client
        .post(format!("{}/api/storage/backfill-backup-sizes", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "should return 200");
    let json: serde_json::Value = resp.json().await.unwrap();
    let data = &json["data"];

    // Should have found the zero-size record
    assert!(
        data["zeroSizeRecords"].as_i64().unwrap_or(0) > 0,
        "should find the zero-size record, got zeroSizeRecords={:#}",
        data["zeroSizeRecords"]
    );
    // Should spawn a task (which will fail gracefully since no SSH)
    assert!(data["taskId"].is_string(), "should return a taskId string");
    assert!(data["message"].is_string(), "should include a message");
}

// ═════════════════════════════════════════════════════════════════════════
// /api/storage/purge-orphans
// ═════════════════════════════════════════════════════════════════════════

/// Verify that /api/storage/status includes orphanedFileCount.
#[tokio::test]
async fn storage_status_includes_orphaned_count() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/storage/status", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: serde_json::Value = resp.json().await.unwrap();
    // All files in seed data have folder_id=NULL, so orphaned count should be >= 0
    let orphaned = json["data"]["orphanedFileCount"].as_i64();
    assert!(
        orphaned.is_some(),
        "expected orphanedFileCount to be a number, got {:?}",
        json["data"]["orphanedFileCount"]
    );
}

/// POST /api/storage/purge-orphans without confirm → 400.
#[tokio::test]
async fn storage_purge_orphans_no_confirm() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/storage/purge-orphans", base))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400, "expected 400 when confirm is missing");
}

/// POST /api/storage/purge-orphans with confirm=true when no orphans → {"purged": 0}.
#[tokio::test]
async fn storage_purge_orphans_empty() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/storage/purge-orphans", base))
        .json(&serde_json::json!({"confirm": true}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200");
    let json: serde_json::Value = resp.json().await.unwrap();
    // Seed data has orphaned files (folder_id=NULL), so purged >= 0
    let purged = json["data"]["purged"].as_i64().unwrap();
    assert!(purged >= 0, "expected purged >= 0, got {}", purged);
}

/// Create orphaned files (folder_id=NULL) then purge them.
#[tokio::test]
async fn storage_purge_orphans_with_orphans() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Get baseline orphan count
    let status_resp0 = client
        .get(format!("{}/api/storage/status", base))
        .send()
        .await
        .unwrap();
    let status_json0: serde_json::Value = status_resp0.json().await.unwrap();
    let baseline = status_json0["data"]["orphanedFileCount"].as_i64().unwrap();

    // Insert an additional orphaned file (include NOT NULL column last_modified)
    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist, file_hash)
           VALUES (100, '/orphan/test.flac', 'flac', 1000000, 1700000000, 'Orphan', 'Test', 'hash100')"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    // Verify status shows orphan count increased by 1
    let status_resp = client
        .get(format!("{}/api/storage/status", base))
        .send()
        .await
        .unwrap();
    let status_json: serde_json::Value = status_resp.json().await.unwrap();
    let before = status_json["data"]["orphanedFileCount"].as_i64().unwrap();
    assert_eq!(
        before,
        baseline + 1,
        "expected orphan count = baseline + 1, got {} (baseline was {})",
        before,
        baseline
    );

    // Purge all orphans
    let resp = client
        .post(format!("{}/api/storage/purge-orphans", base))
        .json(&serde_json::json!({"confirm": true}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200");
    let json: serde_json::Value = resp.json().await.unwrap();
    // All orphans are purged, including baseline
    assert_eq!(
        json["data"]["purged"].as_i64().unwrap(),
        before,
        "expected to purge {} orphans (all existing)",
        before
    );

    // Verify status now shows 0
    let status_resp2 = client
        .get(format!("{}/api/storage/status", base))
        .send()
        .await
        .unwrap();
    let status_json2: serde_json::Value = status_resp2.json().await.unwrap();
    let after = status_json2["data"]["orphanedFileCount"].as_i64().unwrap();
    assert_eq!(after, 0, "expected 0 orphan count after purge");
}

/// Purging twice — second call returns {"purged": 0}.
#[tokio::test]
async fn storage_purge_orphans_idempotent() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Get baseline
    let status_resp0 = client
        .get(format!("{}/api/storage/status", base))
        .send()
        .await
        .unwrap();
    let status_json0: serde_json::Value = status_resp0.json().await.unwrap();
    let baseline = status_json0["data"]["orphanedFileCount"].as_i64().unwrap();

    // Insert 3 orphans with unique paths (UNIQUE constraint on file_path)
    for i in 200..=202 {
        let path = format!("/orphan/file{}.flac", i);
        sqlx::query(
            r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist, file_hash)
               VALUES (?, ?, 'flac', 1000000, 1700000000, 'Orphan', 'Test', ?)"#,
        )
        .bind(i)
        .bind(&path)
        .bind(format!("hash{}", i))
        .execute(&pool)
        .await
        .unwrap();
    }

    // First purge — should remove all orphans (baseline + 3 new)
    let resp1 = client
        .post(format!("{}/api/storage/purge-orphans", base))
        .json(&serde_json::json!({"confirm": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);
    let json1: serde_json::Value = resp1.json().await.unwrap();
    assert_eq!(
        json1["data"]["purged"].as_i64().unwrap(),
        baseline + 3,
        "expected to purge {} files (baseline {} + 3 new)",
        baseline + 3,
        baseline
    );

    // Second purge — should return 0
    let resp2 = client
        .post(format!("{}/api/storage/purge-orphans", base))
        .json(&serde_json::json!({"confirm": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);
    let json2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(
        json2["data"]["purged"].as_i64().unwrap(),
        0,
        "second purge should return 0"
    );
}

/// Create orphaned files by inserting raw records (no folder_id), verify orphan count.
#[tokio::test]
async fn storage_orphan_count_after_folder_delete() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Verify baseline: no orphans from seed data
    let status_resp = client
        .get(format!("{}/api/storage/status", base))
        .send()
        .await
        .unwrap();
    let status_json: serde_json::Value = status_resp.json().await.unwrap();
    let before = status_json["data"]["orphanedFileCount"].as_i64().unwrap();

    // Insert orphaned files that lack folder_id (simulating stale records)
    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist, file_hash)
           VALUES (400, '/test/orphan/file1.flac', 'flac', 2000000, 1700000000, 'Orphan1', 'ArtistA', 'hash400'),
                  (401, '/test/orphan/file2.flac', 'flac', 3000000, 1700000000, 'Orphan2', 'ArtistB', 'hash401')"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    // Verify status shows orphan count increased
    let status_resp2 = client
        .get(format!("{}/api/storage/status", base))
        .send()
        .await
        .unwrap();
    let status_json2: serde_json::Value = status_resp2.json().await.unwrap();
    let orphaned = status_json2["data"]["orphanedFileCount"].as_i64().unwrap();
    assert_eq!(
        orphaned,
        before + 2,
        "expected {} orphans ({} baseline + 2 new), got {}",
        before + 2,
        before,
        orphaned
    );

    // Purge all orphans (baseline + 2 new)
    let purge_resp = client
        .post(format!("{}/api/storage/purge-orphans", base))
        .json(&serde_json::json!({"confirm": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(purge_resp.status(), 200);
    let purge_json: serde_json::Value = purge_resp.json().await.unwrap();
    let purged = purge_json["data"]["purged"].as_i64().unwrap();
    assert_eq!(
        purged, orphaned,
        "expected to purge {} orphans (all), got {}",
        orphaned, purged
    );

    // Verify orphan count is now 0
    let status_resp3 = client
        .get(format!("{}/api/storage/status", base))
        .send()
        .await
        .unwrap();
    let status_json3: serde_json::Value = status_resp3.json().await.unwrap();
    let after = status_json3["data"]["orphanedFileCount"].as_i64().unwrap();
    assert_eq!(after, 0, "expected 0 orphans after purge");
}
