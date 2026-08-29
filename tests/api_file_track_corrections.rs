//! Integration tests for /api/files/{id}/track-corrections and related endpoints.
//!
//! Endpoints covered:
//! - GET /api/files/{id}/track-corrections — full correction state for a file
//! - PUT /api/files/{id}/track-corrections — upsert corrections for a file
//! - GET /api/tracks/{id}/file-corrections — full correction state for a track
//! - PUT /api/tracks/{id}/file-corrections — upsert corrections for a track
//! - DELETE /api/file-track-corrections/{id} — delete a single correction

mod common;

use serde_json::Value;

/// Helper: parse the `data` value from a response JSON body.
fn data(body: Value) -> Value {
    body["data"].clone()
}

/// Helper: parse the `error` string from a response JSON body.
fn error(body: Value) -> String {
    body["error"].as_str().unwrap_or("").to_string()
}

// ═══════════════════════════════════════════════════════════════════════════
// File-side: GET /api/files/{id}/track-corrections
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn corrections_list_for_file_shows_all_fields() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // File 1 (US001) is linked to service_track 1 (spotify:track:aaa, US001)
    // via the ISRC match in v_file_track_link.
    let resp = client
        .get(format!("{}/api/files/1/track-corrections", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "GET should return 200");

    let d = data(resp.json().await.unwrap());

    // Should have all four key fields
    assert_eq!(d["fileId"], 1);
    assert!(
        d["automaticLinks"].is_array(),
        "automaticLinks should be an array"
    );
    assert!(
        d["manualIncludes"].is_array(),
        "manualIncludes should be an array"
    );
    assert!(
        d["manualExcludes"].is_array(),
        "manualExcludes should be an array"
    );
    assert!(
        d["effectiveLinks"].is_array(),
        "effectiveLinks should be an array"
    );

    // File 1 has ISRC US001 → should auto-link to track 1
    let auto = d["automaticLinks"].as_array().unwrap();
    // File 1 also has spotify_id='spotify:track:aaa' which could add a duplicate
    // via the OR conditions. In practice the same track matches both ISRC and
    // spotify_id, so we should have at least 1 auto link.
    assert!(
        !auto.is_empty(),
        "file 1 should have at least one automatic link"
    );

    // Initially no manual corrections
    assert!(
        d["manualIncludes"].as_array().unwrap().is_empty(),
        "no manual includes initially"
    );
    assert!(
        d["manualExcludes"].as_array().unwrap().is_empty(),
        "no manual excludes initially"
    );

    // Effective links should match auto links
    let eff = d["effectiveLinks"].as_array().unwrap();
    assert!(!eff.is_empty(), "effective links should not be empty");
}

#[tokio::test]
async fn corrections_list_for_file_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/files/9999/track-corrections", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "non-existent file should return 404");
    assert!(
        error(resp.json().await.unwrap()).contains("not found"),
        "error message should mention not found"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// File-side: PUT /api/files/{id}/track-corrections
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn corrections_exclude_unlinks_file_from_track() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // File 1 (US001, spotify:track:aaa) is auto-linked to service_track 1
    // Verify it's linked via track-detail
    let before = client
        .get(format!("{}/api/tracks/1/detail", base))
        .send()
        .await
        .unwrap();
    let before_data = data(before.json().await.unwrap());
    let before_files = before_data["files"].as_array().unwrap();
    assert!(
        before_files.iter().any(|f| f["id"] == 1),
        "file 1 should be in track 1's detail before exclude"
    );

    // Exclude the auto-link
    let put_resp = client
        .put(format!("{}/api/files/1/track-corrections", base))
        .json(&serde_json::json!({
            "corrections": [
                {"trackId": 1, "linkType": "exclude", "reason": "wrong version"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(put_resp.status(), 200, "PUT should return 200");

    let put_data = data(put_resp.json().await.unwrap());
    // Manual excludes should now contain track 1
    let excludes = put_data["manualExcludes"].as_array().unwrap();
    assert_eq!(excludes.len(), 1, "should have 1 manual exclude");
    assert_eq!(excludes[0]["trackId"], 1);

    // Effective links should be empty (the only link was excluded)
    let effective = put_data["effectiveLinks"].as_array().unwrap();
    assert!(
        effective.is_empty(),
        "effective links should be empty after exclude"
    );

    // Verify track-detail no longer shows file 1
    let after = client
        .get(format!("{}/api/tracks/1/detail", base))
        .send()
        .await
        .unwrap();
    let after_data = data(after.json().await.unwrap());
    let after_files = after_data["files"].as_array().unwrap();
    assert!(
        !after_files.iter().any(|f| f["id"] == 1),
        "file 1 should NOT be in track 1's detail after exclude"
    );

    // But file 2 (also US001, stem.m4a) is still linked if not excluded
    // Actually file 2 also has ISRC US001 → still auto-linked
    assert!(
        after_files.iter().any(|f| f["id"] == 2),
        "file 2 should still be in track 1's detail"
    );
}

#[tokio::test]
async fn corrections_include_works_without_isrc() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // File 4 has ISRC US999 and no spotify_id → no auto-links
    let before = client
        .get(format!("{}/api/files/4/track-corrections", base))
        .send()
        .await
        .unwrap();
    let before_data = data(before.json().await.unwrap());
    assert!(
        before_data["automaticLinks"].as_array().unwrap().is_empty(),
        "file 4 has no auto links"
    );

    // Manually include file 4 → track 2 (any existing service_track)
    let put_resp = client
        .put(format!("{}/api/files/4/track-corrections", base))
        .json(&serde_json::json!({
            "corrections": [
                {"trackId": 2, "linkType": "include", "reason": "orphan match"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(put_resp.status(), 200);

    let put_data = data(put_resp.json().await.unwrap());
    let includes = put_data["manualIncludes"].as_array().unwrap();
    assert_eq!(includes.len(), 1, "should have 1 manual include");

    // Effective links should now show track 2
    let effective = put_data["effectiveLinks"].as_array().unwrap();
    assert_eq!(effective.len(), 1, "should have 1 effective link");
    assert_eq!(effective[0]["trackId"], 2);

    // Track-detail for track 2 should show file 4
    let detail = client
        .get(format!("{}/api/tracks/2/detail", base))
        .send()
        .await
        .unwrap();
    let detail_data = data(detail.json().await.unwrap());
    let files = detail_data["files"].as_array().unwrap();
    assert!(
        files.iter().any(|f| f["id"] == 4),
        "file 4 should appear in track 2's detail after include"
    );
}

#[tokio::test]
async fn corrections_include_links_file_to_track() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Include file 4 → track 2
    client
        .put(format!("{}/api/files/4/track-corrections", base))
        .json(&serde_json::json!({
            "corrections": [
                {"trackId": 2, "linkType": "include"}
            ]
        }))
        .send()
        .await
        .unwrap();

    // Track-detail for track 2 should include file 4
    let detail = client
        .get(format!("{}/api/tracks/2/detail", base))
        .send()
        .await
        .unwrap();
    let detail_data = data(detail.json().await.unwrap());
    let files = detail_data["files"].as_array().unwrap();

    // Track 2 already has file 3 via ISRC US002 auto-link
    // Now file 4 should also be there via include
    let file_ids: Vec<i64> = files.iter().map(|f| f["id"].as_i64().unwrap()).collect();
    assert!(
        file_ids.contains(&3),
        "file 3 (auto-linked) should be present"
    );
    assert!(
        file_ids.contains(&4),
        "file 4 (manually included) should be present"
    );
}

#[tokio::test]
async fn corrections_exclude_wins_over_auto_isrc_match() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Exclude file 1 → track 1 auto-link
    client
        .put(format!("{}/api/files/1/track-corrections", base))
        .json(&serde_json::json!({
            "corrections": [
                {"trackId": 1, "linkType": "exclude"}
            ]
        }))
        .send()
        .await
        .unwrap();

    // Check that v_file_track_link no longer has (file_id=1, track_id=1)
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM v_file_track_link WHERE file_id = 1 AND track_id = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "excluded pair should not appear in v_file_track_link"
    );
}

#[tokio::test]
async fn corrections_idempotent_include() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Include file 4 → track 2 twice — should be idempotent
    let body = serde_json::json!({
        "corrections": [
            {"trackId": 2, "linkType": "include"}
        ]
    });

    let resp1 = client
        .put(format!("{}/api/files/4/track-corrections", base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);

    let resp2 = client
        .put(format!("{}/api/files/4/track-corrections", base))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);

    // Should only have one correction row
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM file_track_corrections WHERE file_id = 4 AND track_id = 2",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "duplicate include should not create a second row");

    // Effective links should show track 2 exactly once
    let get_resp = client
        .get(format!("{}/api/files/4/track-corrections", base))
        .send()
        .await
        .unwrap();
    let get_data = data(get_resp.json().await.unwrap());
    let effective = get_data["effectiveLinks"].as_array().unwrap();
    assert_eq!(effective.len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════════
// Validation
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn corrections_invalid_link_type_400() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .put(format!("{}/api/files/1/track-corrections", base))
        .json(&serde_json::json!({
            "corrections": [
                {"trackId": 1, "linkType": "xyz"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "invalid linkType should return 400");
}

#[tokio::test]
async fn corrections_empty_array_400() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .put(format!("{}/api/files/1/track-corrections", base))
        .json(&serde_json::json!({
            "corrections": []
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "empty corrections array should return 400"
    );
}

#[tokio::test]
async fn corrections_nonexistent_track_404() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .put(format!("{}/api/files/1/track-corrections", base))
        .json(&serde_json::json!({
            "corrections": [
                {"trackId": 999999, "linkType": "include"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "non-existent trackId should return 404");
}

#[tokio::test]
async fn corrections_nonexistent_file_404_on_put() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .put(format!("{}/api/files/9999/track-corrections", base))
        .json(&serde_json::json!({
            "corrections": [
                {"trackId": 1, "linkType": "include"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "non-existent fileId should return 404");
}

// ═══════════════════════════════════════════════════════════════════════════
// Track-side: GET /api/tracks/{id}/file-corrections
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn corrections_list_for_track_shows_linked_files() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Track 1 has ISRC US001 → auto-links to files 1 and 2
    let resp = client
        .get(format!("{}/api/tracks/1/file-corrections", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "GET should return 200");

    let d = data(resp.json().await.unwrap());

    assert_eq!(d["trackId"], 1);
    assert!(d["automaticLinks"].is_array());
    assert!(d["manualIncludes"].is_array());
    assert!(d["manualExcludes"].is_array());
    assert!(d["effectiveLinks"].is_array());

    // Auto links should include files 1 and 2 (both US001)
    let auto = d["automaticLinks"].as_array().unwrap();
    assert!(!auto.is_empty(), "track 1 should have automatic file links");

    // Effective should match auto initially
    let eff = d["effectiveLinks"].as_array().unwrap();
    assert!(!eff.is_empty(), "effective links should not be empty");
}

#[tokio::test]
async fn corrections_list_for_track_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tracks/9999/file-corrections", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "non-existent track should return 404");
}

// ═══════════════════════════════════════════════════════════════════════════
// Track-side: PUT /api/tracks/{id}/file-corrections
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn corrections_put_track_side_works() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Put an exclude correction from the track side: exclude file 1 from track 1
    let resp = client
        .put(format!("{}/api/tracks/1/file-corrections", base))
        .json(&serde_json::json!({
            "corrections": [
                {"trackId": 1, "linkType": "exclude", "reason": "wrong file"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let d = data(resp.json().await.unwrap());
    let excludes = d["manualExcludes"].as_array().unwrap();
    assert_eq!(excludes.len(), 1, "should have 1 manual exclude");
}

#[tokio::test]
async fn corrections_put_track_side_nonexistent_file_404() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Pass a file ID that doesn't exist
    let resp = client
        .put(format!("{}/api/tracks/1/file-corrections", base))
        .json(&serde_json::json!({
            "corrections": [
                {"trackId": 9999, "linkType": "include"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        404,
        "non-existent file via track endpoint should return 404"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// DELETE /api/file-track-corrections/{id}
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn corrections_delete_removes_correction() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // First create a correction
    let put_resp = client
        .put(format!("{}/api/files/1/track-corrections", base))
        .json(&serde_json::json!({
            "corrections": [
                {"trackId": 1, "linkType": "exclude", "reason": "test"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(put_resp.status(), 200);

    // Get the correction ID from the database
    let correction_id: i64 = sqlx::query_scalar(
        "SELECT id FROM file_track_corrections WHERE file_id = 1 AND track_id = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // Verify effective links are empty (because excluded)
    let before_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM v_file_track_link WHERE file_id = 1 AND track_id = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before_count, 0, "should be excluded before delete");

    // Delete the correction
    let del_resp = client
        .delete(format!(
            "{}/api/file-track-corrections/{}",
            base, correction_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(del_resp.status(), 204, "DELETE should return 204");

    // Now the auto-link should be restored
    let after_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM v_file_track_link WHERE file_id = 1 AND track_id = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        after_count, 1,
        "auto-link should be restored after deleting exclude"
    );
}

#[tokio::test]
async fn corrections_delete_not_found() {
    let (client, base, _pool) = common::spawn_test_app().await;

    let resp = client
        .delete(format!("{}/api/file-track-corrections/999999", base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        404,
        "non-existent correction should return 404"
    );
}
