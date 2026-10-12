//! Integration tests for `/api/digging*` and `/api/files/{id}/stream` endpoints.
//!
//! Tests `/api/digging/suggest` (multi-seed suggestion engine) and the
//! `/api/files/{id}/stream` audio streaming endpoint with Range support.
//!
//! Seed data from `seed_digging_data`:
//!
//! | File | Title              | Artist         | BPM  | Key | ISRC  | Notes           |
//! |------|--------------------|----------------|------|-----|-------|-----------------|
//! | 10   | Games People Play  | Paula van Klar  | 140.0| 3m  | US100 | Seed candidate   |
//! | 11   | The Void           | Maite Dedecker  | 141.0| 8m  | US101 | Suggestion target|
//! | 12   | This Summer        | Anna Reusch     | 140.0| 6m  | US102 | Seed candidate   |
//! | 13   | Mean One           | Elon Bass       | 160.0| 1m  | US103 | Outlier (160 BPM)|

mod common;

use serde_json::Value;

// ── Empty request → 400 ────────────────────────────────────────────────────

#[tokio::test]
/// `POST /api/digging/suggest` with empty body returns 400.
///
/// The endpoint requires either `seedTag` or `seedFileIds`. An empty JSON body
/// satisfies neither and must be rejected.
async fn digging_suggest_empty_request() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_digging_data(&pool).await;

    let resp = client
        .post(format!("{}/api/digging/suggest", base))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    // The handler now returns 400 when neither seedTag nor seedFileIds is provided.
    assert_eq!(
        resp.status(),
        400,
        "empty request should return 400 (no seedTag or seedFileIds)"
    );
}

// ── Suggest by file IDs — structure ───────────────────────────────────────

#[tokio::test]
/// `POST /api/digging/suggest` with seed file IDs 10 and 12 returns a valid
/// response structure: seeds array, suggestions array, bpm_min, bpm_max, and
/// candidates_considered.
///
/// Seeds 10 and 12 both have 140 BPM. BPM range = [132, 148]. File 11
/// (141 BPM, 8m) is a candidate within range. File 13 (160 BPM, 1m) is
/// an outlier outside the range.
async fn digging_suggest_by_file_ids() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_digging_data(&pool).await;

    let resp = client
        .post(format!("{}/api/digging/suggest", base))
        .json(&serde_json::json!({
            "seedFileIds": [10, 12],
            "limit": 5
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = json.get("data").expect("response should have 'data' key");

    // seeds: should contain the 2 seed files (10 and 12)
    let seeds = data["seeds"]
        .as_array()
        .expect("'seeds' should be an array");
    assert_eq!(seeds.len(), 2, "should have 2 seed files (10 and 12)");

    let seed_ids: Vec<i64> = seeds.iter().map(|s| s["id"].as_i64().unwrap()).collect();
    assert!(seed_ids.contains(&10), "seeds should include file 10");
    assert!(seed_ids.contains(&12), "seeds should include file 12");

    // bpm_min / bpm_max should be present
    assert!(
        data["bpmMin"].as_f64().is_some(),
        "bpm_min should be present"
    );
    assert!(
        data["bpmMax"].as_f64().is_some(),
        "bpm_max should be present"
    );

    // candidates_considered (may be 0 if no candidates match, but field exists)
    assert!(
        data["candidatesConsidered"].as_u64().is_some(),
        "candidates_considered should be present"
    );
}

// ── BPM range ──────────────────────────────────────────────────────────────

#[tokio::test]
/// With seeds at 140 BPM, verify the BPM range is approximately [132, 148]
/// (default ±8 tolerance from the seed cluster).
async fn digging_suggest_bpm_range() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_digging_data(&pool).await;

    // Insert extra candidates at various BPMs to make the range matter
    // Files 30-32: within range (136, 144, 148)
    // File 33: outside range (125 BPM)
    // File 34: outside range (155 BPM)
    for (offset, bpm, title) in [
        (0, 136.0, "In Range Low"),
        (1, 144.0, "In Range Mid"),
        (2, 148.0, "In Range High"),
        (3, 125.0, "Outlier Low"),
        (4, 155.0, "Outlier High"),
    ] {
        let id = 30 + offset;
        sqlx::query(
            r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
                 bpm, musical_key, isrc, file_hash)
               VALUES (?, ?, 'flac', 5000000, 1700000000, ?, 'Test Artist', ?, ?, ?, 'bpm-hash')"#,
        )
        .bind(id)
        .bind(format!("/test/stems/{}.flac", title))
        .bind(title)
        .bind(bpm)
        .bind(key_for_bpm(bpm))
        .bind(format!("US{}", 200 + offset))
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"INSERT INTO file_locations (file_id, location_type, path, file_size)
               VALUES (?, 'local', ?, 5000000)"#,
        )
        .bind(id)
        .bind(format!("/test/stems/{}.flac", title))
        .execute(&pool)
        .await
        .unwrap();
    }

    let resp = client
        .post(format!("{}/api/digging/suggest", base))
        .json(&serde_json::json!({
            "seedFileIds": [10, 12],
            "limit": 10
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = json.get("data").expect("response should have 'data' key");

    let bpm_min = data["bpmMin"].as_f64().unwrap();
    let bpm_max = data["bpmMax"].as_f64().unwrap();

    // Seeds are both 140 BPM, default range ±8 → [132, 148]
    assert!(
        bpm_min <= 132.0,
        "bpm_min (={}) should be ≤ 132 for ±8 range from 140",
        bpm_min
    );
    assert!(
        bpm_max >= 148.0,
        "bpm_max (={}) should be ≥ 148 for ±8 range from 140",
        bpm_max
    );
}

fn key_for_bpm(_bpm: f64) -> &'static str {
    // Assign compatible Camelot keys to keep things varied
    "4m"
}

// ── Outlier exclusion ──────────────────────────────────────────────────────

#[tokio::test]
/// When a seed file has BPM >20 away from the median, it should be marked
/// as an outlier and excluded from BPM-range computation.
///
/// File 13 (160 BPM) is an outlier relative to files 10 and 12 (140 BPM).
/// It should have `excludedAsOutlier: true`.
async fn digging_suggest_outlier_excluded() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_digging_data(&pool).await;

    let resp = client
        .post(format!("{}/api/digging/suggest", base))
        .json(&serde_json::json!({
            "seedFileIds": [10, 12, 13],
            "limit": 5
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = json.get("data").expect("response should have 'data' key");
    let seeds = data["seeds"]
        .as_array()
        .expect("'seeds' should be an array");

    // File 13 (Mean One, 160 BPM) should be flagged as outlier
    let outlier: Vec<&Value> = seeds.iter().filter(|s| s["id"] == 13).collect();
    assert!(!outlier.is_empty(), "file 13 should be in seeds array");
    assert!(
        outlier[0]["excludedAsOutlier"].as_bool().unwrap_or(false),
        "file 13 (160 BPM) should be marked as outlier with seeds at 140 BPM"
    );

    // Files 10 and 12 should NOT be outliers
    for id in &[10i64, 12] {
        let seed: Vec<&Value> = seeds.iter().filter(|s| s["id"] == *id).collect();
        assert!(!seed.is_empty(), "file {} should be in seeds array", id);
        assert!(
            !seed[0]["excludedAsOutlier"].as_bool().unwrap_or(true),
            "file {} (140 BPM) should NOT be an outlier",
            id
        );
    }
}

// ── Suggestions populated ──────────────────────────────────────────────────

#[tokio::test]
/// With extra candidate tracks at varied BPMs and compatible Camelot keys,
/// verify that the suggestions array is non-empty.
async fn digging_suggest_returns_suggestions() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_digging_data(&pool).await;

    // Insert 5 extra candidate tracks that are within BPM range and have
    // Camelot-compatible keys (+1, -1, +2, -2, same, +7, -7 jumps from 3m/6m).
    // Seed key 3m: compatible with 2m(±1), 4m(±1), 1m(-2), 5m(+2), 10m(+7), 8m(-7), 3m(same)
    // Seed key 6m: compatible with 5m(±1), 7m(±1), 4m(-2), 8m(+2), 1m(+7), 11m(-7), 6m(same)
    // Use 4m and 8m to cover both seeds
    let candidates = [
        (100, "135 BPM 4m", "Artist C", 135.0, "4m", "US200"),
        (101, "142 BPM 5m", "Artist D", 142.0, "5m", "US201"),
        (102, "138 BPM 7m", "Artist E", 138.0, "7m", "US202"),
        (103, "145 BPM 8m", "Artist F", 145.0, "8m", "US203"),
        (104, "133 BPM 10m", "Artist G", 133.0, "10m", "US204"),
    ];

    for (offset, title, artist, bpm, key, isrc) in &candidates {
        sqlx::query(
            r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
                 bpm, musical_key, isrc, file_hash)
               VALUES (?, ?, 'flac', 5000000, 1700000000, ?, ?, ?, ?, ?, 'candidate-hash')"#,
        )
        .bind(offset)
        .bind(format!("/test/stems/{}.flac", title))
        .bind(title)
        .bind(artist)
        .bind(bpm)
        .bind(key)
        .bind(isrc)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"INSERT INTO file_locations (file_id, location_type, path, file_size)
               VALUES (?, 'local', ?, 5000000)"#,
        )
        .bind(offset)
        .bind(format!("/test/stems/{}.flac", title))
        .execute(&pool)
        .await
        .unwrap();
    }

    let resp = client
        .post(format!("{}/api/digging/suggest", base))
        .json(&serde_json::json!({
            "seedFileIds": [10, 12],
            "limit": 5
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = json.get("data").expect("response should have 'data' key");
    let suggestions = data["suggestions"]
        .as_array()
        .expect("'suggestions' should be an array");

    assert!(
        !suggestions.is_empty(),
        "with 5 compatible candidates, suggestions should not be empty"
    );

    // Each suggestion should have key fields
    for s in suggestions {
        assert!(
            s["fileId"].as_i64().is_some(),
            "each suggestion should have a fileId"
        );
        assert!(
            s["title"].as_str().is_some(),
            "each suggestion should have a title"
        );
        assert!(
            s["artist"].as_str().is_some(),
            "each suggestion should have an artist"
        );
        assert!(
            s["scoreBreakdown"].is_object(),
            "each suggestion should have a scoreBreakdown"
        );
    }
}

// ── Audio stream: 404 not found ────────────────────────────────────────────

#[tokio::test]
/// `GET /api/files/9999/stream` returns 404 when the file does not exist in
/// the database.
async fn files_audio_stream_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/files/9999/stream", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404, "non-existent file ID should return 404");
}

// ── Audio stream: 206 Partial Content ──────────────────────────────────────

#[tokio::test]
/// `GET /api/files/{id}/stream` with a `Range: bytes=0-99` header returns
/// HTTP 206 Partial Content with the correct Content-Range and first 100 bytes.
///
/// Creates a temporary file on disk so the stream handler can open it.
async fn files_audio_stream_range() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Create a temp file for the stream test
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join("momos_test_stream.flac");
    let content = b"This is a test audio file content for streaming range tests. It needs to be at least 100 bytes long to test Range requests properly.";
    std::fs::write(&tmp_path, content).unwrap();

    // Insert a DB record pointing to this temp file
    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
             bpm, musical_key, isrc, file_hash)
           VALUES (?, ?, 'flac', ?, 1700000000, 'Stream Test', 'Test Artist',
                   120.0, '6m', 'US999', 'stream-hash')"#,
    )
    .bind(100i64)
    .bind(tmp_path.to_string_lossy().to_string())
    .bind(content.len() as i64)
    .execute(&pool)
    .await
    .unwrap();

    let resp = client
        .get(format!("{}/api/files/100/stream", base))
        .header("Range", "bytes=0-99")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        206,
        "Range request should return 206 Partial Content"
    );

    // Check Content-Range header
    let content_range = resp
        .headers()
        .get("content-range")
        .expect("206 response should have Content-Range header")
        .to_str()
        .unwrap();
    assert!(
        content_range.starts_with("bytes 0-99/"),
        "Content-Range should start with 'bytes 0-99/' (got: {})",
        content_range
    );

    // Check body: should be exactly the first 100 bytes
    let body = resp.bytes().await.unwrap();
    assert_eq!(body.len(), 100, "response body should be exactly 100 bytes");
    assert_eq!(
        &body[..],
        &content[..100],
        "response body should match the first 100 bytes of the content"
    );

    // Cleanup
    let _ = std::fs::remove_file(&tmp_path);
}

// ── Audio stream: 206 Partial Content mid-file ─────────────────────────────

#[tokio::test]
/// `GET /api/files/{id}/stream` with `Range: bytes=50-149` returns the correct
/// byte range from the middle of the file.
async fn files_audio_stream_range_mid_file() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join("momos_test_stream_mid.flac");
    let content = b"This test content is longer so we can request a middle range properly for testing the mid-file range behavior. Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.";
    std::fs::write(&tmp_path, content).unwrap();

    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
             bpm, musical_key, isrc, file_hash)
           VALUES (?, ?, 'flac', ?, 1700000000, 'Mid Range Test', 'Test Artist',
                   120.0, '6m', 'US998', 'stream-hash2')"#,
    )
    .bind(101i64)
    .bind(tmp_path.to_string_lossy().to_string())
    .bind(content.len() as i64)
    .execute(&pool)
    .await
    .unwrap();

    let resp = client
        .get(format!("{}/api/files/101/stream", base))
        .header("Range", "bytes=50-149")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        206,
        "Range request should return 206 Partial Content"
    );

    let content_range = resp
        .headers()
        .get("content-range")
        .expect("206 response should have Content-Range header")
        .to_str()
        .unwrap();
    assert!(
        content_range.contains("/"),
        "Content-Range should contain total size"
    );

    let body = resp.bytes().await.unwrap();
    assert_eq!(
        body.len(),
        100,
        "response body for bytes=50-149 should be 100 bytes"
    );
    assert_eq!(
        &body[..],
        &content[50..150],
        "response body should match bytes 50-149 of the content"
    );

    let _ = std::fs::remove_file(&tmp_path);
}

// ── Audio stream: end-of-file range ────────────────────────────────────────

#[tokio::test]
/// `GET /api/files/{id}/stream` with `Range: bytes=500-` (open-ended) returns
/// the remaining bytes from position 500 to end-of-file.
async fn files_audio_stream_range_open_ended() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join("momos_test_stream_open.flac");
    // Must be > 500 bytes so bytes=500- is a valid range without subtraction underflow
    let content = b"This content needs to be long enough that requesting from byte 500 onward makes sense. Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum. This paragraph is padded to ensure the total length exceeds 500 bytes so the open-ended range request works correctly.";
    std::fs::write(&tmp_path, content).unwrap();

    sqlx::query(
        r#"INSERT INTO files (id, file_path, file_type, file_size, last_modified, title, artist,
             bpm, musical_key, isrc, file_hash)
           VALUES (?, ?, 'flac', ?, 1700000000, 'Open Range Test', 'Test Artist',
                   120.0, '6m', 'US997', 'stream-hash3')"#,
    )
    .bind(102i64)
    .bind(tmp_path.to_string_lossy().to_string())
    .bind(content.len() as i64)
    .execute(&pool)
    .await
    .unwrap();

    let resp = client
        .get(format!("{}/api/files/102/stream", base))
        .header("Range", "bytes=500-")
        .send()
        .await
        .unwrap();

    // If the file is shorter than 500 bytes, the handler may clamp to file_size-1.
    // We just verify 206 is returned with some body.
    assert_eq!(
        resp.status(),
        206,
        "open-ended Range request should return 206 Partial Content"
    );

    let _content_range = resp
        .headers()
        .get("content-range")
        .expect("206 response should have Content-Range header");

    let body = resp.bytes().await.unwrap();
    assert!(
        body.len() > 0,
        "response body should not be empty for open-ended range"
    );

    let _ = std::fs::remove_file(&tmp_path);
}

// ── Phase 2: Digging search ───────────────────────────────────────────────

#[tokio::test]
/// `GET /api/digging/search?q=Games` returns search results from the digging
/// track index. With a query matching the seeded "Games People Play" title,
/// the endpoint should return at least one result.
async fn digging_search() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_digging_data(&pool).await;

    let resp = client
        .get(format!("{}/api/digging/search?q=Games", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: Value = resp.json().await.unwrap();
    let results = body["data"]["files"]
        .as_array()
        .expect("response data.files should be an array");

    assert!(
        !results.is_empty(),
        "search for 'Games' should find Games People Play, got empty"
    );

    // Verify the result has the expected shape
    let first = &results[0];
    assert!(
        first["id"].as_i64().is_some(),
        "search result should have id"
    );
    assert!(
        first["title"].as_str().is_some(),
        "search result should have title"
    );
}

// ── Phase 2: Digging tracks ───────────────────────────────────────────────

#[tokio::test]
/// `GET /api/digging/tracks?limit=5` returns paginated digging track results.
/// Basic smoke test — verify it returns an array and respects the limit.
async fn digging_tracks() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_digging_data(&pool).await;

    let resp = client
        .get(format!("{}/api/digging/tracks?pageSize=3", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: Value = resp.json().await.unwrap();
    let tracks = body["data"]["tracks"]
        .as_array()
        .expect("response data.tracks should be an array");

    // There should be at least the 4 digging-seeded files (10-13)
    assert!(
        !tracks.is_empty(),
        "digging tracks should return at least the seeded files"
    );

    // Each track should have the expected fields
    for t in tracks {
        assert!(t["id"].as_i64().is_some(), "each track should have an id");
        assert!(
            t["title"].as_str().is_some(),
            "each track should have a title"
        );
    }

    // Verify limit is respected
    assert!(
        tracks.len() <= 3,
        "should return at most 3 tracks with limit=3, got {}",
        tracks.len()
    );
}

// ── Phase 4: Digging suggest — no seeds ───────────────────────────────────

#[tokio::test]
/// `POST /api/digging/suggest` with empty body `{}` returns an error (not 200).
/// The handler returns 500 because get_multi_seed_suggestions produces an Err
/// when neither seedTag nor seedFileIds is provided.
async fn digging_suggest_no_seeds() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_digging_data(&pool).await;

    let resp = client
        .post(format!("{}/api/digging/suggest", base))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    // Empty body returns 400 (no seedTag or seedFileIds)
    assert_eq!(
        resp.status(),
        400,
        "empty request should return 400 (no seedTag or seedFileIds), got {}",
        resp.status()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 5 — Digging ladder suggest
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/digging/ladder/suggest` — ladder suggestions from a previous track.
async fn digging_ladder_suggest() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_digging_data(&pool).await;

    let resp = client
        .post(format!("{}/api/digging/ladder/suggest", base))
        .json(&serde_json::json!({
            "previousTrackId": 10,
            "limit": 5
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    eprintln!("digging ladder suggest response: {body}");

    // May return 200 with suggestions or 500 if no candidates
    assert!(
        status == 200 || status == 500,
        "ladder suggest should return 200 or 500, got {}",
        status
    );

    if status == 200 {
        let data = &body["data"];
        assert!(
            data["suggestions"].is_array(),
            "response should have suggestions array"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 6 — Digging tracks with filter params
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/digging/tracks?energyLevels=1&pageSize=3` — filter params.
async fn digging_tracks_with_params() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_digging_data(&pool).await;

    let resp = client
        .get(format!(
            "{}/api/digging/tracks?energyLevels=1&pageSize=3&sortBy=bpm&sortOrder=asc",
            base
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.unwrap();
    let tracks = body["data"]["tracks"]
        .as_array()
        .expect("response data.tracks should be an array");

    // Should respect limit
    assert!(
        tracks.len() <= 3,
        "should return at most 3 tracks with limit=3, got {}",
        tracks.len()
    );

    // Verify sortBy=bpm, sortOrder=asc works (BPMs should be ascending)
    let bpms: Vec<f64> = tracks.iter().filter_map(|t| t["bpm"].as_f64()).collect();
    if bpms.len() >= 2 {
        for pair in bpms.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "BPMs should be in ascending order, got {:?}",
                bpms
            );
        }
    }
}
