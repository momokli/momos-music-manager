//! Integration tests for /api/dynamic-bundles* endpoints.
//!
//! Endpoints covered:
//! - GET /api/dynamic-bundles — list all bundles
//! - POST /api/dynamic-bundles — create a bundle with tag
//! - GET /api/dynamic-bundles/{id} — get single bundle
//! - PUT /api/dynamic-bundles/{id} — update bundle filters
//! - DELETE /api/dynamic-bundles/{id} — delete bundle and tag
//! - POST /api/dynamic-bundles/{id}/resolve — force re-resolution
//! - PUT /api/tags/{id}/backpack — backpack toggle on bundle tag
//!
//! Seed data (from common::seed_dynamic_bundles_data):
//! - Tag categories: Setlist=1, Phase=2, Mood=3, Vibe=4, Merkmal=5
//! - Tags: hammahalle (id=50, Mood), spät (id=51, Vibe), bouncy (id=52, Vibe)
//! - Files: id=60 (120 BPM flac), id=61 (140 BPM stem.m4a), id=62 (155 BPM stem.m4a), id=63 (180 BPM flac)
//! - File links via playlists: 61→hammahalle, 62→spät, 63→bouncy

mod common;

use serde_json::Value;

/// Helper: parse the `data` value from a response JSON body.
fn data_value(body: Value) -> Value {
    body["data"].clone()
}

/// Helper: parse an array from `data`.
fn data_array(body: Value) -> Vec<Value> {
    body["data"].as_array().unwrap().clone()
}

// ═══════════════════════════════════════════════════════════════════════════
// Create
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn dynamic_bundles_create_basic() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    let resp = client
        .post(format!("{}/api/dynamic-bundles", base))
        .json(&serde_json::json!({
            "name": "Hard Techno 140-160",
            "baseTags": ["hammahalle", "spät"],
            "bpmMin": 140,
            "bpmMax": 160,
            "excludeWavSources": true,
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 201, "creating a bundle should return 201");
    let body: Value = resp.json().await.unwrap();
    let bundle = data_value(body);

    assert_eq!(bundle["name"], "Hard Techno 140-160");
    assert!(
        bundle["id"].as_i64().unwrap() > 0,
        "bundle should have an id"
    );
    assert!(
        bundle["tagId"].as_i64().unwrap() > 0,
        "bundle should have a tag id"
    );
    assert_eq!(bundle["tagName"], "Hard Techno 140-160");
    assert_eq!(bundle["tagBackpack"], false);
    assert_eq!(
        bundle["matchingFileCount"], 2,
        "should match 2 files (ids 61 and 62 within BPM 140-160 and with base tags)"
    );
}

#[tokio::test]
async fn dynamic_bundles_create_all_tracks() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    let resp = client
        .post(format!("{}/api/dynamic-bundles", base))
        .json(&serde_json::json!({
            "name": "All BPM 140",
            "includeAllTracks": true,
            "bpmMin": 140,
            "bpmMax": 160,
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();
    let bundle = data_value(body);

    assert_eq!(bundle["matchingFileCount"], 3);
}

#[tokio::test]
async fn dynamic_bundles_create_no_name() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    let resp = client
        .post(format!("{}/api/dynamic-bundles", base))
        .json(&serde_json::json!({
            "name": "",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "empty name should return 400 Bad Request"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("name is required"),
        "error should mention missing name"
    );
}

#[tokio::test]
async fn dynamic_bundles_create_bpm_only() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    let resp = client
        .post(format!("{}/api/dynamic-bundles", base))
        .json(&serde_json::json!({
            "name": "Medium BPM",
            "includeAllTracks": true,
            "bpmMin": 130,
            "bpmMax": 170,
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();
    let bundle = data_value(body);

    assert_eq!(
        bundle["matchingFileCount"], 3,
        "should match 3 files (ids 61, 62, 63 within 130-170 BPM)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// List
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn dynamic_bundles_list_empty() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    let resp = client
        .get(format!("{}/api/dynamic-bundles", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let bundles = data_array(body);

    assert_eq!(bundles.len(), 0, "no bundles created yet, should be empty");
}

#[tokio::test]
async fn dynamic_bundles_list_after_create() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    // Create one bundle
    client
        .post(format!("{}/api/dynamic-bundles", base))
        .json(&serde_json::json!({
            "name": "Test Bundle",
            "includeAllTracks": true,
        }))
        .send()
        .await
        .unwrap();

    let resp = client
        .get(format!("{}/api/dynamic-bundles", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let bundles = data_array(body);

    assert_eq!(bundles.len(), 1, "should have exactly 1 bundle");
    assert_eq!(bundles[0]["name"], "Test Bundle");
    assert_eq!(bundles[0]["tagName"], "Test Bundle");
    assert!(bundles[0]["matchingFileCount"].as_i64().is_some());
}

// ═══════════════════════════════════════════════════════════════════════════
// Get by ID
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn dynamic_bundles_get() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    // Create a bundle first
    let create_resp = client
        .post(format!("{}/api/dynamic-bundles", base))
        .json(&serde_json::json!({
            "name": "Get Test Bundle",
            "includeAllTracks": true,
        }))
        .send()
        .await
        .unwrap();

    let created: Value = create_resp.json().await.unwrap();
    let bundle_id = created["data"]["id"].as_i64().unwrap();

    // Get it by ID
    let resp = client
        .get(format!("{}/api/dynamic-bundles/{}", base, bundle_id))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let bundle = data_value(body);

    assert_eq!(bundle["id"], bundle_id);
    assert_eq!(bundle["name"], "Get Test Bundle");
    assert_eq!(bundle["tagName"], "Get Test Bundle");
}

#[tokio::test]
async fn dynamic_bundles_get_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    let resp = client
        .get(format!("{}/api/dynamic-bundles/99999", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
}

// ═══════════════════════════════════════════════════════════════════════════
// Update
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn dynamic_bundles_update() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    // Create a bundle with BPM 140-160
    let create_resp = client
        .post(format!("{}/api/dynamic-bundles", base))
        .json(&serde_json::json!({
            "name": "Update Test",
            "includeAllTracks": true,
            "bpmMin": 140,
            "bpmMax": 160,
        }))
        .send()
        .await
        .unwrap();

    let created: Value = create_resp.json().await.unwrap();
    let bundle_id = created["data"]["id"].as_i64().unwrap();

    // Verify initial count
    assert_eq!(created["data"]["matchingFileCount"], 3);

    // Update to wider BPM range
    let resp = client
        .put(format!("{}/api/dynamic-bundles/{}", base, bundle_id))
        .json(&serde_json::json!({
            "bpmMin": 100,
            "bpmMax": 200,
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let bundle = data_value(body);

    assert_eq!(bundle["id"], bundle_id);
    assert_eq!(
        bundle["matchingFileCount"], 7,
        "wider BPM range should match all files (1,2,3,60,61,62,63)"
    );
    assert_eq!(bundle["bpmMin"], 100.0);
    assert_eq!(bundle["bpmMax"], 200.0);
}

#[tokio::test]
async fn dynamic_bundles_update_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    let resp = client
        .put(format!("{}/api/dynamic-bundles/99999", base))
        .json(&serde_json::json!({"name": "Nope"}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
}

// ═══════════════════════════════════════════════════════════════════════════
// Delete
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn dynamic_bundles_delete_and_gone() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    // Create a bundle
    let create_resp = client
        .post(format!("{}/api/dynamic-bundles", base))
        .json(&serde_json::json!({
            "name": "Delete Test",
            "includeAllTracks": true,
        }))
        .send()
        .await
        .unwrap();

    let created: Value = create_resp.json().await.unwrap();
    let bundle_id = created["data"]["id"].as_i64().unwrap();
    let tag_id = created["data"]["tagId"].as_i64().unwrap();

    // Delete it
    let del_resp = client
        .delete(format!("{}/api/dynamic-bundles/{}", base, bundle_id))
        .send()
        .await
        .unwrap();

    assert_eq!(
        del_resp.status(),
        204,
        "delete should return 204 No Content"
    );

    // Verify bundle is gone
    let get_resp = client
        .get(format!("{}/api/dynamic-bundles/{}", base, bundle_id))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), 404, "bundle should be gone");

    // Tag survives (FK is bundle → tag, not reverse — so deleting the bundle
    // does NOT cascade-delete the tag. The tag remains as a standalone Setlist tag.
    let tag_resp = client
        .get(format!("{}/api/tags/{}", base, tag_id))
        .send()
        .await
        .unwrap();
    assert_eq!(tag_resp.status(), 200, "tag should survive bundle deletion");
}

#[tokio::test]
async fn dynamic_bundles_delete_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    let resp = client
        .delete(format!("{}/api/dynamic-bundles/99999", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
}

// ═══════════════════════════════════════════════════════════════════════════
// Resolve
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn dynamic_bundles_resolve() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    // Create a bundle
    let create_resp = client
        .post(format!("{}/api/dynamic-bundles", base))
        .json(&serde_json::json!({
            "name": "Resolve Test",
            "includeAllTracks": true,
            "bpmMin": 140,
        }))
        .send()
        .await
        .unwrap();

    let created: Value = create_resp.json().await.unwrap();
    let bundle_id = created["data"]["id"].as_i64().unwrap();

    // Call resolve
    let resp = client
        .post(format!(
            "{}/api/dynamic-bundles/{}/resolve",
            base, bundle_id
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let data = data_value(body);

    assert_eq!(data["id"], bundle_id);
    assert!(data["matchingFileCount"].as_i64().is_some());
}

#[tokio::test]
async fn dynamic_bundles_resolve_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    let resp = client
        .post(format!("{}/api/dynamic-bundles/99999/resolve", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
}

// ═══════════════════════════════════════════════════════════════════════════
// Backpack integration (is_in_backpack reflects tag backpack)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn dynamic_bundles_backpack_toggle() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_dynamic_bundles_data(&pool).await;

    // Create a bundle
    let create_resp = client
        .post(format!("{}/api/dynamic-bundles", base))
        .json(&serde_json::json!({
            "name": "Backpack Test",
            "includeAllTracks": true,
            "bpmMin": 140,
        }))
        .send()
        .await
        .unwrap();

    let created: Value = create_resp.json().await.unwrap();
    let tag_id = created["data"]["tagId"].as_i64().unwrap();
    let bundle_id = created["data"]["id"].as_i64().unwrap();

    // Verify backpack is false initially
    assert_eq!(created["data"]["tagBackpack"], false);

    // Toggle backpack on via tags endpoint
    let backpack_resp = client
        .put(format!("{}/api/tags/{}/backpack", base, tag_id))
        .json(&serde_json::json!({"backpack": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(backpack_resp.status(), 200);

    // Verify the bundle GET now reflects backpack=true
    let get_resp = client
        .get(format!("{}/api/dynamic-bundles/{}", base, bundle_id))
        .send()
        .await
        .unwrap();

    assert_eq!(get_resp.status(), 200);
    let body: Value = get_resp.json().await.unwrap();
    let bundle = data_value(body);
    assert_eq!(
        bundle["tagBackpack"], true,
        "bundle should reflect tag backpack status"
    );

    // Toggle backpack off
    client
        .put(format!("{}/api/tags/{}/backpack", base, tag_id))
        .json(&serde_json::json!({"backpack": false}))
        .send()
        .await
        .unwrap();

    let get_resp2 = client
        .get(format!("{}/api/dynamic-bundles/{}", base, bundle_id))
        .send()
        .await
        .unwrap();
    let body2: Value = get_resp2.json().await.unwrap();
    let bundle2 = data_value(body2);
    assert_eq!(
        bundle2["tagBackpack"], false,
        "bundle should reflect changed backpack status"
    );
}
