//! Integration tests for `/api/tag-categories*` endpoints.
//!
//! Tests CRUD for tag categories. Migration 001 seeds 5 default categories
//! (Setlist, Phase, Mood, Vibe, Merkmal).

mod common;

use serde_json::Value;

/// GET /api/tag-categories — returns 5 default categories.
#[tokio::test]
async fn tag_categories_list() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tag-categories", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let categories = json["data"].as_array().unwrap();

    // Should have 5 default categories (Setlist, Phase, Mood, Vibe, Merkmal)
    assert_eq!(
        categories.len(),
        5,
        "should have 5 default categories, got {}: {:#?}",
        categories.len(),
        categories
    );

    // Verify some expected default categories
    let names: Vec<&str> = categories
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(names.contains(&"Setlist"), "should include Setlist");
    assert!(names.contains(&"Phase"), "should include Phase");
    assert!(names.contains(&"Mood"), "should include Mood");
    assert!(names.contains(&"Vibe"), "should include Vibe");
    assert!(names.contains(&"Merkmal"), "should include Merkmal");
}

/// POST /api/tag-categories — creates a new category.
#[tokio::test]
async fn tag_categories_create() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/tag-categories", base))
        .json(&serde_json::json!({
            "name": "Test",
            "prefix": "T",
            "icon": "fa-test",
            "sortOrder": 10
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "create should return 200");

    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    let new_id = data["id"]
        .as_i64()
        .expect("created category should have an id");
    assert_eq!(
        data["name"].as_str(),
        Some("Test"),
        "category name should match"
    );
    eprintln!("created category: {data:#?}");

    // Verify it shows up in list
    let list = client
        .get(format!("{}/api/tag-categories", base))
        .send()
        .await
        .unwrap();
    let list_json: Value = list.json().await.unwrap();
    let categories = list_json["data"].as_array().unwrap();
    assert_eq!(
        categories.len(),
        6,
        "should now have 6 categories (5 default + 1 created)"
    );

    // Return new_id for delete test
    let _ = new_id;
}

/// DELETE /api/tag-categories/{new_id} — deletes a category.
#[tokio::test]
async fn tag_categories_delete() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // First create a category to delete
    let create = client
        .post(format!("{}/api/tag-categories", base))
        .json(&serde_json::json!({
            "name": "DeleteMe",
            "prefix": "D",
            "icon": "fa-delete",
            "sortOrder": 99
        }))
        .send()
        .await
        .unwrap();
    let create_json: Value = create.json().await.unwrap();
    let new_id = create_json["data"]["id"].as_i64().unwrap();

    // Delete it
    let resp = client
        .delete(format!("{}/api/tag-categories/{}", base, new_id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "delete should return 200");

    // Verify it's gone (back to 5 categories)
    let list = client
        .get(format!("{}/api/tag-categories", base))
        .send()
        .await
        .unwrap();
    let list_json: Value = list.json().await.unwrap();
    let categories = list_json["data"].as_array().unwrap();
    assert_eq!(
        categories.len(),
        5,
        "should be back to 5 categories after delete, got {}",
        categories.len()
    );
}
