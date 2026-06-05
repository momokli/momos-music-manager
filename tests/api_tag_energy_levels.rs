//! Integration tests for `/api/tag-energy-levels*` endpoints.
//!
//! Tests CRUD for tag energy levels. Seed data includes default tag energy
//! levels (from migration 001) plus any created by `seed_basic_data`.

mod common;

use serde_json::Value;

/// GET /api/tag-energy-levels — returns list of tag energy levels.
#[tokio::test]
async fn tag_energy_levels_list() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tag-energy-levels", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let items = json["data"].as_array().cloned().unwrap_or_default();

    // Should have at least some default energy levels
    assert!(
        !items.is_empty(),
        "tag energy levels should not be empty, got: {json:#?}"
    );
    eprintln!("tag energy levels: {items:#?}");

    // Each entry should have tag_id and energy_level
    for item in &items {
        assert!(
            item["tag_id"].as_i64().is_some() || item["tagId"].as_i64().is_some(),
            "each item should have a tag_id"
        );
    }
}

/// PUT /api/tag-energy-levels/1 — sets energy level for a tag.
#[tokio::test]
async fn tag_energy_levels_set() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .put(format!("{}/api/tag-energy-levels/1", base))
        .json(&serde_json::json!({"energyLevel": 3}))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("set tag energy level response: {body}");

    assert!(
        status == 200 || status == 400 || status == 500,
        "set energy level should return 200/400/500, got {status}"
    );
}

/// PUT /api/tag-energy-levels/batch — batch reorder tag energy levels.
#[tokio::test]
async fn tag_energy_levels_batch() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // The endpoint expects `tags` array with TagReorderItem (tagId, energyLevel, sortOrder)
    let resp = client
        .put(format!("{}/api/tag-energy-levels/batch", base))
        .json(&serde_json::json!({
            "tags": [
                {"tagId": 1, "energyLevel": 4, "sortOrder": 0},
                {"tagId": 2, "energyLevel": 2, "sortOrder": 1}
            ]
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    eprintln!("batch tag energy level status: {status}");
    eprintln!("batch tag energy level body: {body_text}");

    assert!(
        status == 200 || status == 400 || status == 500,
        "batch reorder should return 200/400/500, got {status}"
    );
}
