//! Integration tests for `/api/playlists*` endpoints.
//!
//! Covers: list (paginated, default limit), service filter, search, total count.

mod common;

use serde_json::Value;

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Extract the `data.playlists` array from a playlist API response.
fn extract_playlists(json: &Value) -> &Vec<Value> {
    json["data"]["playlists"].as_array().unwrap()
}

/// Extract the `data.total` count from a playlist API response.
fn extract_total(json: &Value) -> i64 {
    json["data"]["total"].as_i64().unwrap()
}

// ── Pagination ──────────────────────────────────────────────────────────────

#[tokio::test]
/// `GET /api/playlists?limit=1` returns exactly 1 item,
/// and `total` reflects the full (unpaged) count of 2.
async fn playlists_list_paginated() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?limit=1", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert_eq!(
        playlists.len(),
        1,
        "with limit=1, exactly 1 playlist should be returned"
    );
    assert_eq!(
        extract_total(&json),
        2,
        "total should reflect all playlists, not the page"
    );
}

#[tokio::test]
/// `GET /api/playlists` with no limit returns all seeded playlists.
async fn playlists_list_default_limit() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert_eq!(
        playlists.len(),
        2,
        "with no limit, all 2 seeded playlists should be returned"
    );
    assert_eq!(extract_total(&json), 2, "total should be 2");
}

// ── Service filter ──────────────────────────────────────────────────────────

#[tokio::test]
/// `?service=spotify` returns both seeded playlists (both are spotify).
async fn playlists_filter_service() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?service=spotify", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert_eq!(playlists.len(), 2, "2 spotify playlists should be returned");
    assert_eq!(extract_total(&json), 2);

    // Both playlists should have service="spotify"
    for p in playlists {
        assert_eq!(
            p["service"], "spotify",
            "returned playlist should be from spotify"
        );
    }
}

#[tokio::test]
/// `?service=local` returns 0 results (no local playlists seeded).
async fn playlists_filter_service_local() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?service=local", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert_eq!(
        playlists.len(),
        0,
        "no local playlists should exist in seed data"
    );
    assert_eq!(extract_total(&json), 0, "total should be 0");
}

// ── Search ──────────────────────────────────────────────────────────────────

#[tokio::test]
/// `?search=Deep` returns only the "Deep Mix" playlist.
async fn playlists_search() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?search=Deep", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert_eq!(playlists.len(), 1, "search 'Deep' should return 1 playlist");
    assert_eq!(extract_total(&json), 1);

    assert_eq!(
        playlists[0]["name"], "Deep Mix",
        "the matching playlist should be 'Deep Mix'"
    );
}

#[tokio::test]
/// `?search=NoMatch` returns an empty array and total=0.
async fn playlists_search_none() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?search=NoMatch", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert_eq!(playlists.len(), 0, "search with no match should be empty");
    assert_eq!(
        extract_total(&json),
        0,
        "total should be 0 when no match found"
    );
}

// ── Total count (from response, no dedicated count endpoint) ────────────────

#[tokio::test]
/// The `total` field in the response matches the length of the `playlists`
/// array when no pagination is applied.
async fn playlists_total_matches_array_length() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let count = extract_total(&json);
    let array_len = extract_playlists(&json).len() as i64;

    assert_eq!(
        count, array_len,
        "total should match array length when no pagination"
    );
    assert_eq!(count, 2, "there should be exactly 2 seeded playlists");
}

// ── Archive filter ─────────────────────────────────────────────────────────

#[tokio::test]
/// `?archive=archived` returns empty — no playlists have archive_deleted=1.
async fn playlists_filter_archive_archived() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?archive=archived", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert!(
        playlists.is_empty(),
        "no archived playlists should exist, got {}",
        playlists.len()
    );
    assert_eq!(extract_total(&json), 0, "total should be 0 for archived");
}

#[tokio::test]
/// `?archive=active` returns both playlists (both have archive_deleted=0).
async fn playlists_filter_archive_active() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?archive=active", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert_eq!(
        playlists.len(),
        2,
        "both seeded playlists should be active (archive_deleted=0)"
    );
    assert_eq!(extract_total(&json), 2);
}

// ── Subscribed filter ──────────────────────────────────────────────────────

#[tokio::test]
/// `?subscribed=true` returns empty — no subscription rows seed.
async fn playlists_filter_subscribed() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?subscribed=true", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert!(
        playlists.is_empty(),
        "no subscribed playlists should exist, got {}",
        playlists.len()
    );
    assert_eq!(
        extract_total(&json),
        0,
        "total should be 0 for subscribed=true"
    );
}

// ── Single playlist detail ─────────────────────────────────────────────────

#[tokio::test]
/// `GET /api/playlists/{id}` returns a single playlist object.
async fn playlists_single() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists/1", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlist = &json["data"];

    assert_eq!(playlist["id"], 1);
    assert_eq!(playlist["name"], "Groovy");
    assert_eq!(playlist["service"], "spotify");
    assert_eq!(
        playlist["playlistId"], "spotify:playlist:111",
        "playlistId should match seeded data"
    );
}

// ── Mutation: toggle archive ───────────────────────────────────────────────

#[tokio::test]
/// `PUT /api/playlists/{id}/archive` with `{"archiveDeleted": true}`
/// toggles the flag on playlist 1, then verify with GET.
async fn playlists_toggle_archive() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Toggle to true
    let resp = client
        .put(format!("{}/api/playlists/1/archive", base))
        .json(&serde_json::json!({"archiveDeleted": true}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "toggle archive should return 200");

    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    assert_eq!(data["id"], 1);
    assert_eq!(data["archiveDeleted"], true);

    // Toggle back to false
    let resp = client
        .put(format!("{}/api/playlists/1/archive", base))
        .json(&serde_json::json!({"archiveDeleted": false}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    assert_eq!(json["data"]["archiveDeleted"], false);
}

// ── Mutation: create local playlist ────────────────────────────────────────

#[tokio::test]
/// `POST /api/playlists/local` creates a local playlist and returns
/// playlistId + trackCount.
async fn playlists_create_local() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/playlists/local", base))
        .json(&serde_json::json!({"name": "test-playlist", "trackIds": [1]}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "create local playlist should return 200"
    );

    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];

    assert!(
        data.get("playlistId").is_some(),
        "response should have playlistId, got: {:#}",
        data
    );
    assert_eq!(data["trackCount"], 1, "should have 1 track, got {:#}", data);

    let playlist_id = data["playlistId"].as_i64().unwrap();
    assert!(
        playlist_id > 0,
        "playlistId should be > 0, got {}",
        playlist_id
    );
}

// ── Archive + subscribed with data ──────────────────────────────────────────

#[tokio::test]
/// With `seed_tag_hierarchy` + `seed_subscribed_playlist`, playlist 3
/// (collapse-capital) is archived. `?archive=archived` returns it.
async fn playlists_filter_archive_archived_with_data() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;
    common::seed_subscribed_playlist(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?archive=archived", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert!(
        !playlists.is_empty(),
        "at least playlist 3 (archived) should be returned"
    );

    // Find playlist 3
    let pl3 = playlists
        .iter()
        .find(|p| p["id"].as_i64() == Some(3))
        .expect("playlist 3 should be in archived results");

    assert_eq!(pl3["name"], "collapse-capital");
    assert_eq!(
        pl3["archiveDeleted"], true,
        "playlist 3 should have archiveDeleted: true"
    );
}

#[tokio::test]
/// With `seed_subscribed_playlist`, playlist 3 has a subscription row.
/// `?subscribed=true` returns it.
async fn playlists_filter_subscribed_with_data() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;
    common::seed_subscribed_playlist(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?subscribed=true", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert!(
        !playlists.is_empty(),
        "at least playlist 3 (subscribed) should be returned"
    );

    // Find playlist 3
    let pl3 = playlists
        .iter()
        .find(|p| p["id"].as_i64() == Some(3))
        .expect("playlist 3 should be in subscribed results");

    assert_eq!(pl3["name"], "collapse-capital");
    assert_eq!(pl3["service"], "spotify");

    // The subscribed playlists response may include subscription fields.
    // At minimum, the playlist itself should be present.

    // Total should be >= 1
    assert!(
        extract_total(&json) >= 1,
        "total should be >= 1 for subscribed playlists"
    );
}

#[tokio::test]
/// `?categories=1` (Setlist category) returns playlists whose matching tags
/// are in the Setlist category. Playlist 3 "collapse-capital" matches tag 10 (Setlist).
///
/// `?categories=3` (Mood category) returns playlist 1 "Groovy" (tag 7 is Mood).
async fn playlists_filter_categories() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_tag_hierarchy(&pool).await;

    // Refresh materialised table so v_playlist_tag_category can resolve
    momos_music_manager::db::refresh_file_resolved_tags(&pool)
        .await
        .unwrap();

    // ── Setlist category (id=1) ──
    let resp_setlist = client
        .get(format!("{}/api/playlists?categories=1", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_setlist.status(), 200, "categories=1 should return 200");

    let json: Value = resp_setlist.json().await.unwrap();
    let playlists = extract_playlists(&json);

    assert!(
        !playlists.is_empty(),
        "categories=1 should return playlist 3 (collapse-capital in Setlist)"
    );

    let has_pl3 = playlists.iter().any(|p| p["id"].as_i64() == Some(3));
    assert!(
        has_pl3,
        "playlist 3 (collapse-capital) should appear for Setlist category"
    );

    // ── Mood category (id=3) ──
    let resp_mood = client
        .get(format!("{}/api/playlists?categories=3", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_mood.status(), 200, "categories=3 should return 200");

    let json_mood: Value = resp_mood.json().await.unwrap();
    let mood_playlists = extract_playlists(&json_mood);

    assert!(
        !mood_playlists.is_empty(),
        "categories=3 should return playlist 1 (Groovy in Mood)"
    );

    let has_pl1 = mood_playlists.iter().any(|p| p["id"].as_i64() == Some(1));
    assert!(
        has_pl1,
        "playlist 1 (Groovy) should appear for Mood category"
    );

    assert_eq!(
        extract_total(&json_mood),
        1,
        "Mood category should match exactly 1 playlist (Groovy)"
    );
}

// ── Error states ───────────────────────────────────────────────────────────

#[tokio::test]
/// `GET /api/playlists/9999` returns 404.
async fn playlists_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists/9999", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404, "expected 404 for non-existent playlist");
}

#[tokio::test]
/// `POST /api/playlists/local` with empty body `{}` returns 422
/// (Axum/serde rejects missing required fields before reaching handler).
async fn playlists_create_local_no_name() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/playlists/local", base))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        422,
        "empty body should return 422 (serde deserialization failure for missing required fields)"
    );

    // 422 response may be plain text from Axum, not JSON
    let _body = resp.text().await.unwrap();
    // Assertion satisfied by status code — deserialization rejection is correct behavior
}

// --- Filter: untagged ---

/// `?untagged=true` returns only playlists that do NOT match any tag
/// via `v_tag_playlist` (case-insensitive name matching).
///
/// Seeded: playlist 1 "Groovy" matches tag "Groovy", playlist 2 "Deep Mix"
/// does NOT match tag "Deep" (literal names "Deep Mix" != "Deep").
/// So `?untagged=true` should return playlist 2 only.
#[tokio::test]
async fn playlists_filter_untagged() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?untagged=true", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK for untagged=true");

    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    // Playlist 1 "Groovy" matches tag "Groovy" => NOT untagged => excluded
    // Playlist 2 "Deep Mix" does NOT match any tag => untagged => included
    assert!(
        !playlists.is_empty(),
        "untagged=true should return at least 1 playlist (Deep Mix)"
    );
    for p in playlists {
        let name = p["name"].as_str().unwrap();
        assert_ne!(
            name, "Groovy",
            "Groovy playlist has a matching tag, should be excluded"
        );
    }

    // Total should not include Groovy
    assert_eq!(extract_total(&json), playlists.len() as i64);
}

/// `?untagged=false` (or no param) returns all playlists regardless of tag status.
#[tokio::test]
async fn playlists_filter_untagged_false() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists?untagged=false", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);

    // Both playlists should appear when filter is off
    assert_eq!(
        playlists.len(),
        2,
        "untagged=false should return all playlists"
    );
    assert_eq!(extract_total(&json), 2);
}

// ═══════════════════════════════════════════════════════════════════════════
// Mutation: delete playlist
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `DELETE /api/playlists/{id}` deletes a playlist, then GET returns 404.
pub async fn playlists_delete() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Create a local playlist to delete
    let create_resp = client
        .post(format!("{}/api/playlists/local", base))
        .json(&serde_json::json!({"name": "DeleteTest", "trackIds": [1]}))
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), 200);
    let create_json: Value = create_resp.json().await.unwrap();
    let playlist_id = create_json["data"]["playlistId"].as_i64().unwrap();

    // Delete it
    let delete_resp = client
        .delete(format!("{}/api/playlists/{}", base, playlist_id))
        .send()
        .await
        .unwrap();
    assert_eq!(delete_resp.status(), 200, "delete should return 200");
    let delete_json: Value = delete_resp.json().await.unwrap();
    assert_eq!(delete_json["data"]["deleted"].as_bool(), Some(true));

    // Verify 404 on refetch
    let get_resp = client
        .get(format!("{}/api/playlists/{}", base, playlist_id))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), 404, "deleted playlist should return 404");
}

// ═══════════════════════════════════════════════════════════════════════════
// Read: playlist tracks
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/playlists/{id}/tracks` returns tracks for a playlist.
/// Currently returns a "not implemented" message.
pub async fn playlists_tracks() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists/1/tracks", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    // Endpoint returns a string placeholder
    let data = &json["data"];
    assert!(
        data.is_string() || data.is_array(),
        "tracks endpoint should return data"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Mutation: add track to playlist
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/playlists/{id}/tracks` adds a track to a playlist.
pub async fn playlists_add_track() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/playlists/1/tracks", base))
        .json(&serde_json::json!({"trackId": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    assert!(
        data.is_string() || data.is_object(),
        "add track endpoint should return data"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Read: subscriptions list (empty)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/playlists/subscriptions` returns empty array on fresh DB.
pub async fn playlists_subscriptions_list() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists/subscriptions", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    let data = json["data"].as_array().unwrap();
    assert!(data.is_empty(), "no subscriptions on fresh DB");
}

// ═══════════════════════════════════════════════════════════════════════════
// Mutation: subscribe
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/playlists/subscriptions` creates a subscription.
pub async fn playlists_subscribe() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/playlists/subscriptions", base))
        .json(&serde_json::json!({"playlistId": "spotify:playlist:111", "service": "spotify"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    assert!(data.get("id").is_some(), "subscribe should return an id");
    assert_eq!(data["service"], "spotify");
    assert_eq!(data["playlistId"], "spotify:playlist:111");
}

// ═══════════════════════════════════════════════════════════════════════════
// Mutation: subscribe + unsubscribe
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// Create a subscription via subscribe, then `DELETE /api/playlists/subscriptions/{id}` to remove it.
pub async fn playlists_unsubscribe() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Subscribe first
    let sub_resp = client
        .post(format!("{}/api/playlists/subscriptions", base))
        .json(&serde_json::json!({"playlistId": "spotify:playlist:111", "service": "spotify"}))
        .send()
        .await
        .unwrap();
    assert_eq!(sub_resp.status(), 200);
    let sub_json: Value = sub_resp.json().await.unwrap();
    let sub_id = sub_json["data"]["id"].as_i64().unwrap();

    // Unsubscribe
    let unsub_resp = client
        .delete(format!("{}/api/playlists/subscriptions/{}", base, sub_id))
        .send()
        .await
        .unwrap();
    assert_eq!(unsub_resp.status(), 200);
    let unsub_json: Value = unsub_resp.json().await.unwrap();
    assert_eq!(unsub_json["data"]["unsubscribed"].as_bool(), Some(true));

    // Verify list is empty
    let list_resp = client
        .get(format!("{}/api/playlists/subscriptions", base))
        .send()
        .await
        .unwrap();
    assert_eq!(list_resp.status(), 200);
    let list_json: Value = list_resp.json().await.unwrap();
    let subs = list_json["data"].as_array().unwrap();
    assert!(
        !subs.iter().any(|s| s["id"].as_i64() == Some(sub_id)),
        "subscription should be removed"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Read: comment-diff-stats
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `GET /api/playlists/comment-diff-stats` returns stats about comment diffs.
pub async fn playlists_comment_diff_stats() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/playlists/comment-diff-stats", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    // Returns stats object — may be empty object or have fields
    assert!(
        data.is_object(),
        "comment-diff-stats should return an object"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Filter: stale
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `?stale=1` returns playlists where local track count < remote_unique_count.
pub async fn playlists_filter_stale() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // With basic seed, remote_unique_count is 0, so no playlist is "stale"
    // because local count (1) > 0. Filter should work without error.
    let resp = client
        .get(format!("{}/api/playlists?stale=true", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "stale filter should return 200");
    let json: Value = resp.json().await.unwrap();
    let playlists = extract_playlists(&json);
    // No playlists are stale with seed data (local count > remote_unique_count=0)
    assert_eq!(
        playlists.len(),
        0,
        "no playlists should be stale with basic seed data"
    );
    assert_eq!(extract_total(&json), 0);
}
