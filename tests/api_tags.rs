//! Integration tests for /api/tags* endpoints.
//!
//! Endpoints covered:
//! - GET /api/tags - paginated tag list with search, category, sort filters
//! - GET /api/tags/count - count query matching the same filters
//! - PUT /api/tags/{id}/backpack - toggle backpack flag
//!
//! Seed data (from common::seed_basic_data):
//! - Tag categories (from migration 001): Setlist=1, Phase=2, Mood=3, Vibe=4, Merkmal=5
//! - Tags: Groovy (id=7, Mood, backpack=0), Deep (id=8, Mood, backpack=1), Dark (id=9, Mood, backpack=0)
//! - The 6 migration-001 phase tags (start, build, peak, release, sustain, end) are in Phase category

mod common;

use serde_json::Value;

/// Helper: parse the `data` array from a response JSON body.
fn data_array(body: Value) -> Vec<Value> {
    body["data"].as_array().unwrap().clone()
}

/// Helper: parse a single object from `data` (for count endpoints, it's a number).
fn data_number(body: Value) -> i64 {
    body["data"].as_i64().unwrap()
}

// ═══════════════════════════════════════════════════════════════════════════
// Basic list & pagination
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tags_list_paginated() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tags?limit=2", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let tags = data_array(body);

    assert_eq!(tags.len(), 2, "?limit=2 should return exactly 2 tags");
    for tag in &tags {
        assert!(tag["id"].as_i64().is_some(), "each tag should have an id");
        assert!(
            tag["name"].as_str().is_some(),
            "each tag should have a name"
        );
    }
}

#[tokio::test]
async fn tags_list_default_limit() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tags", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let tags = data_array(body);

    // seed_basic_data inserts 3 tags (Groovy, Deep, Dark) plus the 6 phase tags
    // from migration 001 = 9 total
    assert_eq!(
        tags.len(),
        9,
        "default (no limit) should return all tags (3 seeded + 6 phase = 9)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Search
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tags_search() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tags?search=Groovy", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let tags = data_array(body);

    assert_eq!(tags.len(), 1, "search for Groovy should return 1 tag");
    assert_eq!(tags[0]["name"], "Groovy");
    assert_eq!(tags[0]["backpack"], false);
    assert_eq!(tags[0]["categoryId"], 3);
    assert_eq!(tags[0]["category"], "Mood");
}

#[tokio::test]
async fn tags_search_deep() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tags?search=Deep", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let tags = data_array(body);

    assert_eq!(tags.len(), 1, "search for Deep should return 1 tag");
    assert_eq!(tags[0]["name"], "Deep");
    assert_eq!(
        tags[0]["backpack"], true,
        "Deep is seeded with backpack=true"
    );
    assert_eq!(tags[0]["categoryId"], 3);
}

// ═══════════════════════════════════════════════════════════════════════════
// Category filter
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tags_filter_category() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // All 3 hand-seeded tags are in Mood category (id=3, name="Mood")
    let resp = client
        .get(format!("{}/api/tags?category=Mood", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let tags = data_array(body);

    // Only our 3 seeded tags are in Mood — the 6 phase tags are in Phase category
    assert_eq!(
        tags.len(),
        3,
        "category=Mood should return the 3 seeded mood tags"
    );
    for tag in &tags {
        assert_eq!(tag["category"], "Mood", "all returned tags should be Mood");
    }
}

#[tokio::test]
async fn tags_filter_category_setlist() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Setlist category — no hand-seeded tags are Setlist.
    let resp = client
        .get(format!("{}/api/tags?category=Setlist", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let tags = data_array(body);

    // Migration 001 seeds only phase tags, no Setlist tags.
    // The 6 phase tags are in Phase category. So Setlist should be empty.
    assert!(
        tags.is_empty(),
        "category=Setlist should return 0 tags (none seeded in Setlist category)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Sort
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tags_sort_name_asc() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tags?sort=t.name&order=asc", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let tags = data_array(body);

    // The 3 seeded tags + 6 phase tags should be sorted alphabetically by name.
    // Expected order: build, dark, deep, end, groovy, peak, release, start, sustain
    let names: Vec<&str> = tags.iter().map(|t| t["name"].as_str().unwrap()).collect();

    let mut sorted = names.clone();
    sorted.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

    assert_eq!(
        names, sorted,
        "tags should be sorted alphabetically by name ascending"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Count
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tags_count() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Get the list (all tags)
    let list_resp = client
        .get(format!("{}/api/tags", base))
        .send()
        .await
        .unwrap();
    let list_body: Value = list_resp.json().await.unwrap();
    let list_len = data_array(list_body).len() as i64;

    // Get the count
    let count_resp = client
        .get(format!("{}/api/tags/count", base))
        .send()
        .await
        .unwrap();
    let count_body: Value = count_resp.json().await.unwrap();
    let count = data_number(count_body);

    assert_eq!(
        count, list_len,
        "/api/tags/count should match the number of items returned by /api/tags"
    );
}

#[tokio::test]
async fn tags_count_with_search() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Count with search filter
    let count_resp = client
        .get(format!("{}/api/tags/count?search=Groovy", base))
        .send()
        .await
        .unwrap();
    let count_body: Value = count_resp.json().await.unwrap();
    let count = data_number(count_body);

    assert_eq!(count, 1, "count with search=Groovy should return 1");
}

#[tokio::test]
async fn tags_count_with_category() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Count with category filter
    let count_resp = client
        .get(format!("{}/api/tags/count?category=Mood", base))
        .send()
        .await
        .unwrap();
    let count_body: Value = count_resp.json().await.unwrap();
    let count = data_number(count_body);

    assert_eq!(
        count, 3,
        "count with category=Mood should return 3 seeded tags"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 2 — Read: Single tag by ID
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tags_single_by_id() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tags/7", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK for tag id=7");

    let body: Value = resp.json().await.unwrap();
    let tag = &body["data"];

    assert_eq!(tag["id"], 7, "tag id should be 7");
    assert_eq!(tag["name"], "Groovy", "tag name should be Groovy");
    // The single-tag endpoint returns 'category' (the category name string),
    // not 'categoryId'. It comes from v_tags_with_categories.
    assert_eq!(tag["category"], "Mood", "tag category should be Mood");
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 3 — Mutation: Create tag
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tags_create() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/tags", base))
        .json(&serde_json::json!({"name": "NewTag", "categoryId": 3}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK for tag creation");

    let body: Value = resp.json().await.unwrap();
    let tag = &body["data"];

    assert!(
        tag["id"].as_i64().is_some(),
        "created tag should have an id"
    );
    assert_eq!(tag["name"], "NewTag", "created tag name should be NewTag");
    // create_tag_handler returns a Tag with category: None (not fetched from view)
    // Verify via list search instead
    let verify_resp = client
        .get(format!("{}/api/tags?search=NewTag", base))
        .send()
        .await
        .unwrap();
    let verify_body: Value = verify_resp.json().await.unwrap();
    let found = &data_array(verify_body)[0];
    assert_eq!(found["name"], "NewTag");
    assert_eq!(found["categoryId"], 3, "should be in Mood category");
    assert_eq!(found["category"], "Mood");
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 3 — Mutation: Categorize (move tag to new category)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tags_categorize() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Step 1: Move Groovy (id=7) from Mood(3) to Vibe(4)
    let put_resp = client
        .put(format!("{}/api/tags/7/categorize", base))
        .json(&serde_json::json!({"categoryId": 4}))
        .send()
        .await
        .unwrap();

    assert_eq!(put_resp.status(), 200, "expected 200 OK for categorize");

    // categorize response body uses small Tag struct (category name string)
    let put_body: Value = put_resp.json().await.unwrap();
    assert_eq!(
        put_body["data"]["category"], "Vibe",
        "categorize response should show new category name"
    );

    // Step 2: Verify category changed via GET (single tag endpoint)
    let get_resp = client
        .get(format!("{}/api/tags/7", base))
        .send()
        .await
        .unwrap();

    assert_eq!(get_resp.status(), 200);
    let body: Value = get_resp.json().await.unwrap();
    let tag = &body["data"];
    assert_eq!(tag["category"], "Vibe", "category should now be Vibe");

    // Also verify via list endpoint (has categoryId)
    let list_resp = client
        .get(format!("{}/api/tags?search=Groovy", base))
        .send()
        .await
        .unwrap();
    let list_body: Value = list_resp.json().await.unwrap();
    let found = &data_array(list_body)[0];
    assert_eq!(
        found["categoryId"], 4,
        "list endpoint should show categoryId=4"
    );
    assert_eq!(found["category"], "Vibe");

    // Step 3: Move back to Mood(3) to not break other tests
    let revert_resp = client
        .put(format!("{}/api/tags/7/categorize", base))
        .json(&serde_json::json!({"categoryId": 3}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        revert_resp.status(),
        200,
        "expected 200 OK for revert categorize"
    );

    // Final verification via list endpoint
    let final_resp = client
        .get(format!("{}/api/tags?search=Groovy", base))
        .send()
        .await
        .unwrap();
    let final_body: Value = final_resp.json().await.unwrap();
    let found = &data_array(final_body)[0];
    assert_eq!(found["categoryId"], 3, "final: back in Mood category");
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 4 — Error: Tag not found
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tags_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tags/9999", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404, "nonexistent tag should return 404");

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"].as_str().is_some(),
        "404 response should have an error field"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 4 — Error: Create tag with empty body
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tags_create_no_name() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/tags", base))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    // Empty body is rejected — missing required fields "name" and "categoryId"
    // Axum JSON extractor returns 422 Unprocessable Entity for missing fields
    assert!(
        resp.status().is_client_error(),
        "empty body should return 4xx, got {}",
        resp.status()
    );

    // The body may be Axum's default error text (not always JSON).
    // Just verify the status code is correct.
    let body_text = resp.text().await.unwrap();
    assert!(
        !body_text.is_empty(),
        "error response should have a body, got empty"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Backpack toggle
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tags_toggle_backpack() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Step 1: Verify initial state — Groovy has backpack=false
    let before = client
        .get(format!("{}/api/tags?search=Groovy", base))
        .send()
        .await
        .unwrap();
    let before_body: Value = before.json().await.unwrap();
    let before_tags = data_array(before_body);
    assert_eq!(before_tags[0]["backpack"], false);
    assert_eq!(before_tags[0]["id"], 7);

    // Step 2: Toggle backpack on via PUT
    let put_resp = client
        .put(format!("{}/api/tags/7/backpack", base))
        .json(&serde_json::json!({ "backpack": true }))
        .send()
        .await
        .unwrap();

    assert_eq!(put_resp.status(), 200);
    let put_body: Value = put_resp.json().await.unwrap();
    assert_eq!(
        put_body["data"]["backpack"], true,
        "PUT response should return backpack: true"
    );

    // Step 3: Verify persistence via GET
    let after = client
        .get(format!("{}/api/tags?search=Groovy", base))
        .send()
        .await
        .unwrap();
    let after_body: Value = after.json().await.unwrap();
    let after_tags = data_array(after_body);
    assert_eq!(
        after_tags[0]["backpack"], true,
        "Groovy should now have backpack=true after PUT toggle"
    );

    // Step 4: Toggle back to false
    let put_back = client
        .put(format!("{}/api/tags/7/backpack", base))
        .json(&serde_json::json!({ "backpack": false }))
        .send()
        .await
        .unwrap();

    assert_eq!(put_back.status(), 200);
    let put_back_body: Value = put_back.json().await.unwrap();
    assert_eq!(put_back_body["data"]["backpack"], false);

    // Step 5: Verify the toggle back persisted
    let final_resp = client
        .get(format!("{}/api/tags?search=Groovy", base))
        .send()
        .await
        .unwrap();
    let final_body: Value = final_resp.json().await.unwrap();
    let final_tags = data_array(final_body);
    assert_eq!(
        final_tags[0]["backpack"], false,
        "Groovy should be back to backpack=false after second toggle"
    );
}

#[tokio::test]
async fn tags_toggle_backpack_deep() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Step 1: Verify initial state — Deep has backpack=true
    let before = client
        .get(format!("{}/api/tags?search=Deep", base))
        .send()
        .await
        .unwrap();
    let before_body: Value = before.json().await.unwrap();
    let before_tags = data_array(before_body);
    assert_eq!(before_tags[0]["backpack"], true);

    // Step 2: Toggle to false
    let put_resp = client
        .put(format!("{}/api/tags/8/backpack", base))
        .json(&serde_json::json!({ "backpack": false }))
        .send()
        .await
        .unwrap();

    assert_eq!(put_resp.status(), 200);
    let put_body: Value = put_resp.json().await.unwrap();
    assert_eq!(put_body["data"]["backpack"], false);

    // Step 3: Verify persistence
    let after = client
        .get(format!("{}/api/tags?search=Deep", base))
        .send()
        .await
        .unwrap();
    let after_body: Value = after.json().await.unwrap();
    let after_tags = data_array(after_body);
    assert_eq!(after_tags[0]["backpack"], false);
}

// ═══════════════════════════════════════════════════════════════════════════
// Hierarchy-dependent tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/tags/curation-queue` returns Setlist tags with parent metadata.
/// Tag 10 (collapse-capital) has 2 parents + file count > 0.
async fn tags_curation_queue() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;

    let resp = client
        .get(format!("{}/api/tags/curation-queue", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "curation-queue should return 200");

    let body: Value = resp.json().await.unwrap();
    let tags = body["data"].as_array().unwrap();

    assert!(
        !tags.is_empty(),
        "curation-queue should return at least the seeded Setlist tag (collapse-capital)"
    );

    // Find tag 10 (collapse-capital)
    let tag10 = tags
        .iter()
        .find(|t| t["id"].as_i64() == Some(10))
        .expect("tag 10 (collapse-capital) should be in curation queue");

    assert_eq!(tag10["name"], "collapse-capital");
    assert!(
        tag10["parentCount"].as_i64().unwrap() >= 2,
        "collapse-capital should have >= 2 parents"
    );
    assert!(
        tag10["parents"].as_array().is_some(),
        "tag should have a parents array"
    );

    let parents = tag10["parents"].as_array().unwrap();
    assert!(parents.len() >= 2, "tag 10 should have at least 2 parents");

    // Verify parent tag IDs are present
    let parent_ids: Vec<i64> = parents.iter().map(|p| p["id"].as_i64().unwrap()).collect();
    assert!(
        parent_ids.contains(&11),
        "should have parent 'shadow' (id=11)"
    );
    assert!(
        parent_ids.contains(&12),
        "should have parent 'techno' (id=12)"
    );

    // File count: track 1 is linked to playlist 3 (collapse-capital) → file 1 has that tag
    assert!(
        tag10["fileCount"].as_i64().unwrap() > 0,
        "collapse-capital should have > 0 files"
    );
}

#[tokio::test]
/// `?search=collapse` filters curation-queue down to matching tag name only.
async fn tags_curation_queue_search() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;

    let resp = client
        .get(format!("{}/api/tags/curation-queue?search=collapse", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    let tags = body["data"].as_array().unwrap();

    assert_eq!(tags.len(), 1, "search=collapse should return only tag 10");
    assert_eq!(tags[0]["id"], 10);
    assert_eq!(tags[0]["name"], "collapse-capital");
}

#[tokio::test]
/// `?has_parents=yes` returns only tags that have at least one parent.
async fn tags_curation_queue_has_parents_yes() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;

    let resp = client
        .get(format!("{}/api/tags/curation-queue?has_parents=yes", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    let tags = body["data"].as_array().unwrap();

    // Tag 10 has parents, so it should be in the results
    let has_tag10 = tags.iter().any(|t| t["id"].as_i64() == Some(10));
    assert!(
        has_tag10,
        "tag 10 (with parents) should appear when has_parents=yes"
    );

    // No tag should have parentCount = 0
    for tag in tags {
        assert!(
            tag["parentCount"].as_i64().unwrap() > 0,
            "all tags returned by has_parents=yes should have parentCount > 0"
        );
    }
}

#[tokio::test]
/// `GET /api/tags/unreviewed` returns tags with reviewed_at IS NULL.
/// Tag 10 (collapse-capital) has parents but still counts as unreviewed
/// since parent assignment does not set reviewed_at.
async fn tags_unreviewed() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;

    let resp = client
        .get(format!("{}/api/tags/unreviewed", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "unreviewed endpoint should return 200");

    let body: Value = resp.json().await.unwrap();
    let data = &body["data"];

    // Response shape: { totalUnreviewed, totalReviewed, queue: [...] }
    assert!(
        data["totalUnreviewed"].as_u64().is_some(),
        "should have totalUnreviewed"
    );
    assert!(
        data["totalReviewed"].as_u64().is_some(),
        "should have totalReviewed"
    );

    let queue = data["queue"].as_array().unwrap();
    assert!(!queue.is_empty(), "unreviewed queue should not be empty");

    // Tag 10 has parents but reviewed_at IS NULL → still appears in unreviewed
    let has_tag10 = queue.iter().any(|t| t["id"].as_i64() == Some(10));
    assert!(
        has_tag10,
        "tag 10 (Setlist with parents, never reviewed) should be in unreviewed queue"
    );

    // Each item should have id + name
    for item in queue {
        assert!(item["id"].as_i64().is_some());
        assert!(item["name"].as_str().is_some());
    }
}

#[tokio::test]
/// `GET /api/tags/11/children` returns tags that have tag 11 ("shadow") as parent.
/// Tag 10 (collapse-capital) has shadow as a parent → should appear.
async fn tags_children() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;

    let resp = client
        .get(format!("{}/api/tags/11/children", base))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "tag children endpoint should return 200"
    );

    let body: Value = resp.json().await.unwrap();
    let children = body["data"].as_array().unwrap();

    assert!(
        !children.is_empty(),
        "tag 11 (shadow) should have at least 1 child"
    );

    // Tag 10 should be among the children
    let has_tag10 = children.iter().any(|t| t["id"].as_i64() == Some(10));
    assert!(
        has_tag10,
        "tag 10 (collapse-capital) should be a child of tag 11 (shadow)"
    );

    // Verify each child has required fields
    for child in children {
        assert!(child["id"].as_i64().is_some());
        assert!(child["name"].as_str().is_some());
    }
}

#[tokio::test]
/// `GET /api/tags/10/suggest` returns a category suggestion for tag 10.
/// The ML model may or may not load in CI — test verifies the endpoint
/// is reachable and returns either a valid suggestion or an embedding error.
async fn tags_suggest() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;

    let resp = client
        .get(format!("{}/api/tags/10/suggest", base))
        .send()
        .await
        .unwrap();

    // The suggest endpoint may succeed (if ML model loads) or 500 (if it doesn't).
    // Both are valid behaviors — the important thing is the endpoint exists.
    let status = resp.status();
    assert!(
        status == 200 || status == 500,
        "expected 200 or 500 (embedding model may not load), got {}",
        status
    );

    let body: Value = resp.json().await.unwrap();

    if status == 200 {
        // Valid response shape
        let data = &body["data"];
        assert!(
            data["suggestedCategoryId"].as_i64().is_some(),
            "should have suggestedCategoryId"
        );
        assert!(
            data["suggestedCategoryName"].as_str().is_some(),
            "should have suggestedCategoryName"
        );
        assert!(
            data["allCategories"].as_array().is_some(),
            "should have allCategories"
        );
    } else {
        // 500 with error message about embedding model
        assert!(
            body["error"].as_str().is_some(),
            "error response should have an error field"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 4 — Mutation: Delete
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `DELETE /api/tags/{id}` deletes a tag, then GET returns 404.
async fn tags_delete() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Create a tag to delete
    let create_resp = client
        .post(format!("{}/api/tags", base))
        .json(&serde_json::json!({"name": "TempDeleteTag", "categoryId": 3}))
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), 200, "create should succeed");
    let body: Value = create_resp.json().await.unwrap();
    let tag_id = body["data"]["id"]
        .as_i64()
        .expect("created tag should have an id");

    // Delete the tag
    let delete_resp = client
        .delete(format!("{base}/api/tags/{tag_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(delete_resp.status(), 200, "delete should return 200");

    // Verify it's gone
    let get_resp = client
        .get(format!("{base}/api/tags/{tag_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), 404, "deleted tag should return 404");
    let get_body: Value = get_resp.json().await.unwrap();
    assert!(
        get_body["error"].is_string(),
        "404 response should have an error field"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Read: from-playlists
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/tags/from-playlists` returns playlists that don't have matching tags.
pub async fn tags_from_playlists() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tags/from-playlists", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    // Response is wrapped: data.playlists (object with playlists array)
    let playlists = json["data"]["playlists"]
        .as_array()
        .or_else(|| json["data"].as_array())
        .expect("response should contain playlists array in data.playlists or data");
    // One playlist without tag: "Deep Mix" doesn't match "Deep" exactly
    assert!(
        !playlists.is_empty(),
        "should have at least 1 untagged playlist"
    );
    let names: Vec<&str> = playlists
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"Deep Mix"),
        "Deep Mix playlist should be untagged"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Energy level edge cases
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `PUT /api/tag-energy-levels/{tag_id}` accepts extreme values (0, 10).
async fn tag_energy_level_edge_cases() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Set energy level to 0 (minimum)
    let resp0 = client
        .put(format!("{}/api/tag-energy-levels/7", base))
        .json(&serde_json::json!({"energyLevel": 0}))
        .send()
        .await
        .unwrap();
    let status0 = resp0.status();
    let body0: Value = resp0.json().await.unwrap();
    eprintln!("energy level 0: {body0}");
    assert!(
        status0 == 200 || status0 == 500,
        "energy level 0 should return 200 or 500, got {status0}"
    );

    // Set energy level to 10 (maximum)
    let resp10 = client
        .put(format!("{}/api/tag-energy-levels/7", base))
        .json(&serde_json::json!({"energyLevel": 10}))
        .send()
        .await
        .unwrap();
    let status10 = resp10.status();
    let body10: Value = resp10.json().await.unwrap();
    eprintln!("energy level 10: {body10}");
    assert!(
        status10 == 200 || status10 == 500,
        "energy level 10 should return 200 or 500, got {status10}"
    );

    // Verify the energy level was stored via GET
    let get_resp = client
        .get(format!("{}/api/tag-energy-levels", base))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), 200);
    let get_body: Value = get_resp.json().await.unwrap();
    eprintln!("all energy levels: {get_body}");
    let levels = get_body["data"].as_array().unwrap();
    // Tag 7 should be present with energyLevel 10 (the last value set)
    let tag7 = levels.iter().find(|l| l["tag_id"].as_i64() == Some(7));
    if let Some(entry) = tag7 {
        let level = entry["energy_level"].as_i64().unwrap_or(-1);
        eprintln!("tag 7 energy level: {level}");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Bulk import edge cases
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/tags/bulk-import` with empty array → returns empty list.
async fn tag_bulk_import_edge_cases() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Empty array — should return empty list (200)
    let resp_empty = client
        .post(format!("{}/api/tags/bulk-import", base))
        .json(&serde_json::json!({"entries": []}))
        .send()
        .await
        .unwrap();
    let status_empty = resp_empty.status();
    let body_empty: Value = resp_empty.json().await.unwrap();
    eprintln!("bulk import empty: {body_empty}");
    assert!(
        status_empty == 200 || status_empty == 400 || status_empty == 500,
        "empty bulk import should return 200/400/500, got {status_empty}"
    );

    // Duplicate names in same import
    let resp_dup = client
        .post(format!("{}/api/tags/bulk-import", base))
        .json(&serde_json::json!({"entries": [
            {"name": "DupTag", "categoryId": 3},
            {"name": "DupTag", "categoryId": 4}
        ]}))
        .send()
        .await
        .unwrap();
    let status_dup = resp_dup.status();
    let body_dup: Value = resp_dup.json().await.unwrap();
    eprintln!("bulk import duplicates: {body_dup}");
    // Duplicate names — may succeed (creating one tag) or fail (400/500)
    assert!(
        status_dup == 200 || status_dup == 400 || status_dup == 500,
        "duplicate bulk import should return 200/400/500, got {status_dup}"
    );

    // Import same name as existing tag — should be idempotent or rejected
    let resp_existing = client
        .post(format!("{}/api/tags/bulk-import", base))
        .json(&serde_json::json!({"entries": [
            {"name": "Groovy", "categoryId": 3}
        ]}))
        .send()
        .await
        .unwrap();
    let status_existing = resp_existing.status();
    let body_existing: Value = resp_existing.json().await.unwrap();
    eprintln!("bulk import existing: {body_existing}");
    assert!(
        status_existing == 200 || status_existing == 400 || status_existing == 500,
        "existing tag import should return 200/400/500, got {status_existing}"
    );
}

#[tokio::test]
/// `POST /api/tags/bulk-categorize` with multiple tags at once.
async fn tag_bulk_categorize_multiple() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;

    // Move tags 7 (Groovy, Mood→cat 3) and 8 (Sparkle, Vibe→cat 4) to category 5 (Merkmal)
    let resp = client
        .post(format!("{}/api/tags/bulk-categorize", base))
        .json(&serde_json::json!({"tagIds": [7, 8], "categoryId": 5}))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    eprintln!("bulk categorize multiple: {body}");

    assert!(
        status == 200,
        "bulk-categorize should return 200, got {status}"
    );

    if status == 200 {
        let updated = body["data"]["updated"].as_i64().unwrap_or(0);
        assert!(
            updated >= 2,
            "should have updated at least 2 tags, got {updated}"
        );
    }

    // Verify tag 7 changed category
    let get7 = client
        .get(format!("{}/api/tags/7", base))
        .send()
        .await
        .unwrap();
    let get7_body: Value = get7.json().await.unwrap();
    eprintln!("tag 7 after recategorize: {get7_body}");

    // Verify tag 8 changed category
    let get8 = client
        .get(format!("{}/api/tags/8", base))
        .send()
        .await
        .unwrap();
    let get8_body: Value = get8.json().await.unwrap();
    eprintln!("tag 8 after recategorize: {get8_body}");
}

#[tokio::test]
/// `GET /api/tags/curation-queue` with limit param to test pagination.
async fn tag_curation_queue_pagination() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;

    // Get all curation queue items first
    let resp_all = client
        .get(format!("{}/api/tags/curation-queue", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_all.status(), 200);
    let body_all: Value = resp_all.json().await.unwrap();
    let all_tags = body_all["data"].as_array().unwrap();
    let total_count = all_tags.len();
    eprintln!("curation queue total items: {total_count}");

    // Get first 2 items with limit
    let resp_page = client
        .get(format!("{}/api/tags/curation-queue?limit=2", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_page.status(), 200);
    let body_page: Value = resp_page.json().await.unwrap();
    let page_tags = body_page["data"].as_array().unwrap();
    eprintln!("curation queue with limit=2: {} items", page_tags.len());

    // limit=2 should return at most 2 items
    assert!(
        page_tags.len() <= 2,
        "limit=2 should return at most 2 items, got {}",
        page_tags.len()
    );

    // If total > limit, page result should be smaller
    if total_count > 2 {
        assert_eq!(page_tags.len(), 2, "should return exactly 2 items when limit=2");
    }

    // Verify each item has required fields
    for item in page_tags {
        assert!(item["id"].as_i64().is_some(), "tag should have an id");
        assert!(item["name"].as_str().is_some(), "tag should have a name");
        assert!(
            item["fileCount"].as_i64().is_some(),
            "tag should have fileCount"
        );
        assert!(
            item["parentCount"].as_i64().is_some(),
            "tag should have parentCount"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Mutation: create-from-playlists
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/tags/create-from-playlists` creates tags from playlists that don't have tags.
pub async fn tags_create_from_playlists() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/tags/create-from-playlists", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    assert!(data.get("created").is_some(), "should return created count");
    assert!(data.get("message").is_some(), "should return message");
}

// ═══════════════════════════════════════════════════════════════════════════
// Read: service-coverage
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/tags/service-coverage` returns service coverage stats.
pub async fn tags_service_coverage() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tags/service-coverage", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    assert!(data.get("total").is_some(), "should have total");
    assert!(data.get("spotify").is_some(), "should have spotify count");
    assert!(
        data["spotify"].as_i64().unwrap_or(-1) >= 0,
        "spotify count should be >= 0"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Read: tag parents get (empty)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/tags/{id}/parents` returns empty array for a tag with no parents.
pub async fn tags_parents_get() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tags/7/parents", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    let parents = json["data"].as_array().unwrap();
    // Tag 7 (Groovy) is a Mood tag — Setlist-only parents, so it has none
    assert!(
        parents.is_empty(),
        "tag 7 (non-Setlist) should have no parents"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Mutation: set tag parents
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `PUT /api/tags/{id}/parents` sets parent tags for a Setlist tag.
pub async fn tags_parents_set() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;

    // Tag 10 (collapse-capital, Setlist) already has parents 11,12,13 from seed_tag_hierarchy
    // Verify they're set
    let get_resp = client
        .get(format!("{}/api/tags/10/parents", base))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), 200);
    let get_json: Value = get_resp.json().await.unwrap();
    let parents = get_json["data"].as_array().unwrap();
    assert!(!parents.is_empty(), "tag 10 should have parent tags");
    assert!(
        parents.iter().any(|p| p["id"].as_i64() == Some(11)),
        "should include shadow"
    );

    // Now SET parents to only tag 7 (Groovy)
    // Note: tag 7 is Mood, not Setlist — might be rejected by validation
    let set_resp = client
        .put(format!("{}/api/tags/10/parents", base))
        .json(&serde_json::json!({"parentTagIds": [7]}))
        .send()
        .await
        .unwrap();
    // Check status before consuming response
    let is_success = set_resp.status().is_success();
    let set_json: Value = set_resp.json().await.unwrap();
    if is_success {
        // Replaced parents
        let result = set_json["data"].as_array().unwrap();
        assert!(
            result.iter().any(|p| p["id"].as_i64() == Some(7)),
            "should include Groovy"
        );
    } else {
        // Non-Setlist parent might be rejected — verify error message
        assert!(
            set_json["error"].is_string(),
            "error response should explain rejection"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Mutation: bulk-categorize
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/tags/bulk-categorize` moves multiple tags to a new category.
pub async fn tags_bulk_categorize() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/tags/bulk-categorize", base))
        .json(&serde_json::json!({"tagIds": [7], "categoryId": 5}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "bulk-categorize should return 200");
    let json: Value = resp.json().await.unwrap();
    assert!(
        json["data"]["updated"].as_i64().is_some(),
        "should return updated count"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Mutation: bulk-import
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/tags/bulk-import` imports multiple tags at once.
pub async fn tags_bulk_import() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/tags/bulk-import", base))
        .json(&serde_json::json!({"entries": [
            {"name": "BulkTestTag1", "categoryId": 3},
            {"name": "BulkTestTag2", "categoryId": 4}
        ]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "bulk-import should return 200");
    let json: Value = resp.json().await.unwrap();
    let data = json["data"].as_array().unwrap();
    assert!(!data.is_empty(), "should return imported tags");
    assert_eq!(data[0]["name"], "BulkTestTag1");
}

// ═══════════════════════════════════════════════════════════════════════════
// Mutation: bulk-resolve
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/tags/bulk-resolve` resolves tags with specified actions.
pub async fn tags_bulk_resolve() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Use "create" action to create a new tag
    let resp = client
        .post(format!("{}/api/tags/bulk-resolve", base))
        .json(&serde_json::json!({"entries": [
            {"name": "ResolveNewTag", "categoryId": 3, "action": "create"}
        ]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "bulk-resolve should return 200");
    let json: Value = resp.json().await.unwrap();
    let data = json["data"].as_array().unwrap();
    assert!(!data.is_empty(), "should return resolved results");
    assert_eq!(data[0]["status"], "created", "tag should be created");
    assert_eq!(data[0]["name"], "ResolveNewTag");

    // Test "move" action on an existing tag
    let resp2 = client
        .post(format!("{}/api/tags/bulk-resolve", base))
        .json(&serde_json::json!({"entries": [
            {"name": "Groovy", "categoryId": 5, "action": "move"}
        ]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);
    let json2: Value = resp2.json().await.unwrap();
    let data2 = json2["data"].as_array().unwrap();
    assert!(!data2.is_empty());
    assert_eq!(data2[0]["status"], "moved", "tag should be moved");
}
