//! Integration tests for `/api/tasks*` endpoints.
//!
//! The TaskManager is in-memory and starts empty. Tasks are created via other
//! endpoints (e.g. POST /api/folders/{id}/scan, POST /api/files/{id}/write-comment).

mod common;

use serde_json::Value;

/// GET /api/tasks — returns empty tasks array on a fresh DB with no prior API calls.
#[tokio::test]
async fn tasks_list_empty() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tasks", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let tasks = json["data"]["tasks"].as_array().unwrap();
    assert!(tasks.is_empty(), "tasks should be empty on fresh start");
}

/// Trigger a scan task via POST /api/folders/1/scan, then verify it appears in /api/tasks.
#[tokio::test]
async fn tasks_list_with_scan_task() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Trigger a scan task via TaskManager (folder 1 path doesn't exist, task will register)
    let resp = client
        .post(format!("{}/api/folders/1/scan?mode=full", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "scan handler should return 200");

    let json: Value = resp.json().await.unwrap();
    let task_id = json["data"]["taskId"]
        .as_str()
        .expect("scan handler should return taskId");
    assert!(!task_id.is_empty(), "taskId should not be empty");

    // Now check tasks list
    let resp = client
        .get(format!("{}/api/tasks", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: Value = resp.json().await.unwrap();
    let tasks = json["data"]["tasks"].as_array().unwrap();

    eprintln!("tasks after write-comment: {tasks:?}");

    assert!(
        !tasks.is_empty(),
        "tasks list should not be empty after triggering a task"
    );

    // The task we just created should be present
    let found = tasks.iter().any(|t| t["id"].as_str() == Some(task_id));
    assert!(found, "created task should appear in tasks list");
}

/// GET /api/tasks/nonexistent-id — returns 404.
#[tokio::test]
async fn tasks_single_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tasks/nonexistent-task-id", base))
        .send()
        .await
        .unwrap();

    // TaskManager returns NOT_FOUND for non-existent task IDs
    assert_eq!(
        resp.status(),
        404,
        "non-existent task should return 404, got {}",
        resp.status()
    );
}

/// GET /api/tasks?status=pending — filters tasks by status.
#[tokio::test]
async fn tasks_list_status_filter() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Trigger a task so we have something to filter
    let resp = client
        .post(format!("{}/api/files/1/write-comment", base))
        .send()
        .await
        .unwrap();
    let json: Value = resp.json().await.unwrap();
    let _task_id = json["data"]["taskId"].as_str().unwrap();

    // Give the task a moment to register in the TaskManager
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Get all tasks first
    let resp = client
        .get(format!("{}/api/tasks", base))
        .send()
        .await
        .unwrap();
    let json: Value = resp.json().await.unwrap();
    let all_tasks = json["data"]["tasks"].as_array().unwrap();
    let all_len = all_tasks.len();

    // Assert at least one task was created
    assert!(
        all_len > 0,
        "should have at least 1 task after triggering write-comment, got {all_len}"
    );

    // Filter by status=failed (the write-comment task fails because the seed
    // file doesn't actually exist on disk)
    let resp = client
        .get(format!("{}/api/tasks?status=failed", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: Value = resp.json().await.unwrap();
    let failed_tasks = json["data"]["tasks"].as_array().unwrap();

    // Failed count should be > 0 and <= all count
    assert!(
        failed_tasks.len() > 0,
        "filtering by status=failed should return at least the write-comment task"
    );
    assert!(
        failed_tasks.len() <= all_len,
        "failed filter should return a subset of all tasks, got {} > {}",
        failed_tasks.len(),
        all_len
    );
}
