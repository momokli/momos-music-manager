//! Integration tests for `/api/folders*` endpoints.
//!
//! Seed data includes folder id=1 with path="/test/stems" (from `common::seed_basic_data`).
//! All tests use `common::spawn_test_app()` which returns (reqwest::Client, base_url, Pool<Sqlite>).

mod common;

use serde_json::Value;

/// GET /api/folders — returns array with seeded folder.
#[tokio::test]
async fn folders_list() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/folders", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let folders = json["data"].as_array().unwrap();
    assert!(!folders.is_empty(), "folders list should not be empty");

    // At least folder id=1 from seed
    let folder1 = folders
        .iter()
        .find(|f| f["id"].as_i64() == Some(1))
        .expect("folder with id=1 should exist");
    // FolderInfo returns "path" (not folderPath)
    assert!(folder1["path"].is_string(), "should have path field");
}

/// GET /api/folders/count — returns an integer matching the list length.
#[tokio::test]
async fn folders_count() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Get list length
    let resp = client
        .get(format!("{}/api/folders", base))
        .send()
        .await
        .unwrap();
    let json: Value = resp.json().await.unwrap();
    let list_len = json["data"].as_array().unwrap().len();

    // Get count
    let resp = client
        .get(format!("{}/api/folders/count", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: Value = resp.json().await.unwrap();
    let count = json["data"].as_i64().unwrap_or_else(|| {
        // Could also be nested differently
        json["data"]["count"]
            .as_i64()
            .unwrap_or_else(|| json["data"].as_i64().unwrap_or(-1))
    });
    assert_eq!(count as usize, list_len, "count should match list length");
}

/// GET /api/folders/1/stats — returns object with expected fields.
#[tokio::test]
async fn folders_single() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/folders/1/stats", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];

    eprintln!("folder stats: {data:#}");

    // FolderStats has snake_case keys (no #[serde(rename_all)])
    // but some fields like folder_path, total_files, watch_enabled should be present
    assert!(
        data["folder_path"].is_string()
            || data["folderPath"].is_string()
            || data["path"].is_string(),
        "should have folder path field"
    );
    assert!(
        data["total_files"].is_number() || data["totalFiles"].is_number(),
        "should have total files count"
    );
}

/// POST /api/folders/1/watch — toggles active flag, verify via stats.
#[tokio::test]
async fn folders_toggle_watch() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Get initial watch state from FolderStats (snake_case)
    let resp = client
        .get(format!("{}/api/folders/1/stats", base))
        .send()
        .await
        .unwrap();
    let json: Value = resp.json().await.unwrap();
    let initial_active = json["data"]["watch_enabled"].as_bool().unwrap_or(true);

    // Toggle
    let resp = client
        .post(format!("{}/api/folders/1/watch", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "toggle should return 200");

    // The toggle response returns FolderInfo (camelCase) with watchEnabled
    let json: Value = resp.json().await.unwrap();
    let toggled = json["data"]["watchEnabled"]
        .as_bool()
        .unwrap_or(initial_active);

    // Verify state changed
    let resp = client
        .get(format!("{}/api/folders/1/stats", base))
        .send()
        .await
        .unwrap();
    let json: Value = resp.json().await.unwrap();
    let new_active = json["data"]["watch_enabled"].as_bool().unwrap_or(toggled);

    assert_ne!(
        initial_active, new_active,
        "active/watch state should toggle"
    );
}

/// GET /api/folders/9999/stats — returns 404 for nonexistent folder.
#[tokio::test]
async fn folders_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/folders/9999/stats", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404, "nonexistent folder should return 404");
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 2 — Mutation: Delete
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `DELETE /api/folders/{id}` deletes a folder, then GET stats returns 404.
async fn folders_delete() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Delete seeded folder 1
    let delete_resp = client
        .delete(format!("{}/api/folders/1", base))
        .send()
        .await
        .unwrap();
    assert_eq!(delete_resp.status(), 200, "delete should return 200");
    let delete_body: Value = delete_resp.json().await.unwrap();
    assert!(
        delete_body["data"].is_string(),
        "delete response should contain data message"
    );

    // Verify stats endpoint returns 404
    let stats_resp = client
        .get(format!("{}/api/folders/1/stats", base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        stats_resp.status(),
        404,
        "deleted folder should return 404 from stats"
    );
    let stats_body: Value = stats_resp.json().await.unwrap();
    assert!(
        stats_body["error"].is_string(),
        "404 response should have an error field"
    );

    // Delete non-existent folder returns 404
    let missing_resp = client
        .delete(format!("{}/api/folders/9999", base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        missing_resp.status(),
        404,
        "deleting non-existent folder should return 404"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 3 — Mutation: Update not found
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `PUT /api/folders/{id}` with non-existent id returns 404.
async fn folders_update_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Use a body without a path (path validation runs before 404 check for non-existent folder).
    // Setting watch_enabled (no path) skips path validation and reaches the 404 check.
    let resp = client
        .put(format!("{}/api/folders/9999", base))
        .json(&serde_json::json!({"watch_enabled": false}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        404,
        "updating non-existent folder should return 404"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"].is_string(),
        "404 response should have an error field"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 4 — Mutation: Create
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/folders` creates a new folder, returns FolderInfo with an id.
async fn folders_create() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Create a temp directory that actually exists
    std::fs::create_dir_all("/tmp/test-create").unwrap();

    let resp = client
        .post(format!("{}/api/folders", base))
        .json(&serde_json::json!({
            "path": "/tmp/test-create",
            "watchEnabled": true
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    eprintln!("folders create status: {status}, body: {body_text:?}");
    assert_eq!(status, 200, "create should return 200");

    let json: Value = serde_json::from_str(&body_text).unwrap();
    let data = &json["data"];
    assert!(
        data["id"].as_i64().is_some(),
        "created folder should have an id"
    );
    assert_eq!(
        data["path"].as_str().unwrap_or(""),
        "/tmp/test-create",
        "path should match"
    );
    assert!(
        data["watchEnabled"].as_bool().unwrap_or(false),
        "watch_enabled should be true"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 5 — Mutation: Update
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `PUT /api/folders/1` updates folder fields, verify via GET stats.
async fn folders_update() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // The folder already exists in DB (seed data), and update doesn't validate existing path
    let resp = client
        .put(format!("{}/api/folders/1", base))
        .json(&serde_json::json!({
            "watchEnabled": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "update should return 200");

    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    assert!(
        !data["watchEnabled"].as_bool().unwrap_or(true),
        "watch_enabled should now be false"
    );

    // Verify via stats endpoint (which uses camelCase due to serde rename)
    let stats = client
        .get(format!("{}/api/folders/1/stats", base))
        .send()
        .await
        .unwrap();
    let stats_json: Value = stats.json().await.unwrap();
    assert_eq!(
        stats_json["data"]["watchEnabled"].as_bool(),
        Some(false),
        "stats should reflect update: watchEnabled=false"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 6 — Scan folder task
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/folders/1/scan?mode=full` triggers a scan task (folder 1 path
/// doesn't exist on disk, so the task registers but will fail gracefully).
/// Returns {taskId, folderId, mode}.
async fn folders_scan() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .post(format!("{}/api/folders/1/scan?mode=full", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "scan should return 200");

    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    let task_id = data["taskId"]
        .as_str()
        .expect("scan response should have taskId");
    assert!(!task_id.is_empty(), "taskId should not be empty");
    assert_eq!(data["folderId"].as_i64(), Some(1), "folderId should be 1");
    assert_eq!(data["mode"].as_str(), Some("full"), "mode should be 'full'");
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 7 — Folder backup config
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `PUT /api/folders/1/backup` sets backup_path and scan_sources on a folder.
async fn folders_backup_config() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .put(format!("{}/api/folders/1/backup", base))
        .json(&serde_json::json!({
            "backupPath": "/backups/test",
            "scanSources": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "backup config should return 200");

    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    assert_eq!(
        data["backupPath"].as_str(),
        Some("/backups/test"),
        "backupPath should match"
    );
    assert!(
        data["scanSources"].as_bool().unwrap_or(false),
        "scanSources should be true"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 8 — Folder auto-backup toggle
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `PUT /api/folders/1/auto-backup` toggles the auto_backup flag.
async fn folders_auto_backup() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // The folder already exists in DB (seed data), and auto-backup doesn't validate existing path
    let resp = client
        .put(format!("{}/api/folders/1/auto-backup", base))
        .json(&serde_json::json!({"autoBackup": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "auto-backup toggle should return 200");

    let json: Value = resp.json().await.unwrap();
    assert!(
        !json["data"]["autoBackup"].as_bool().unwrap_or(true),
        "autoBackup should now be false"
    );

    // Verify via GET folder endpoint
    let get_resp = client
        .get(format!("{}/api/folders/1", base))
        .send()
        .await
        .unwrap();
    let get_json: Value = get_resp.json().await.unwrap();
    eprintln!("folder response: {get_json}");
    // The folder endpoint returns autoBackup in the data
    assert_eq!(
        get_json["data"]["autoBackup"].as_bool(),
        Some(false),
        "folder should reflect autoBackup=false"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 9 — Folder scan sources
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
/// `POST /api/folders/1/scan-sources` triggers a WAV source scan task.
/// First sets scan_sources=true on the folder, then scans.
async fn folders_scan_sources() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Enable scan_sources first
    client
        .put(format!("{}/api/folders/1/backup", base))
        .json(&serde_json::json!({
            "backupPath": "/backups/test",
            "scanSources": true
        }))
        .send()
        .await
        .unwrap();

    let resp = client
        .post(format!("{}/api/folders/1/scan-sources", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "scan-sources should return 200");

    let json: Value = resp.json().await.unwrap();
    let data = &json["data"];
    let task_id = data["taskId"]
        .as_str()
        .expect("scan-sources response should have taskId");
    assert!(!task_id.is_empty(), "taskId should not be empty");
}
