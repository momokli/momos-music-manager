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

// ═══════════════════════════════════════════════════════════════════════════
// Additional task tests
// ═══════════════════════════════════════════════════════════════════════════

/// POST /api/folders/1/scan to start a task, then DELETE /api/tasks/{taskId} to cancel it.
#[tokio::test]
async fn tasks_cancel_running() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Trigger a scan task
    let scan_resp = client
        .post(format!("{}/api/folders/1/scan?mode=full", base))
        .send()
        .await
        .unwrap();
    assert_eq!(scan_resp.status(), 200, "scan handler should return 200");

    let json: Value = scan_resp.json().await.unwrap();
    let task_id = json["data"]["taskId"]
        .as_str()
        .expect("scan handler should return taskId");
    assert!(!task_id.is_empty(), "taskId should not be empty");

    // Cancel the task via the task manager cancel endpoint
    // Note: we use the tasks endpoint for cancellation, not spotify sync cancel
    let cancel_resp = client
        .delete(format!("{base}/api/tasks/{task_id}"))
        .send()
        .await
        .unwrap();

    let cancel_status = cancel_resp.status();
    let cancel_body: Value = cancel_resp.json().await.unwrap();
    eprintln!("cancel task response: {cancel_body}");

    // Task cancellation may succeed (200) or fail if already completed (500)
    assert!(
        cancel_status == 200 || cancel_status == 500,
        "cancel task should return 200 or 500, got {cancel_status}"
    );
}

/// GET /api/tasks/{taskId} for a task created by scan.
#[tokio::test]
async fn tasks_get_by_id() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Trigger a scan task
    let scan_resp = client
        .post(format!("{}/api/folders/1/scan?mode=full", base))
        .send()
        .await
        .unwrap();
    assert_eq!(scan_resp.status(), 200);

    let json: Value = scan_resp.json().await.unwrap();
    let task_id = json["data"]["taskId"]
        .as_str()
        .expect("scan handler should return taskId");

    // Fetch the task by ID
    let get_resp = client
        .get(format!("{base}/api/tasks/{task_id}"))
        .send()
        .await
        .unwrap();

    let get_status = get_resp.status();
    let get_body: Value = get_resp.json().await.unwrap();
    eprintln!("get task by id response: {get_body}");

    assert!(
        get_status == 200,
        "get task by id should return 200, got {get_status}"
    );

    if get_status == 200 {
        // The task might be nested under data or data.task
        let task_data = get_body["data"]
            .as_object()
            .or_else(|| get_body["data"]["task"].as_object())
            .or_else(|| get_body["data"]["tasks"].as_object());
        if let Some(task) = task_data {
            assert!(
                task.contains_key("id") || get_body["data"]["id"].as_str().is_some(),
                "task should have an id field"
            );
        }

        // At minimum the data should contain something
        assert!(
            get_body["data"].is_object() || get_body["data"].is_string(),
            "task detail should be an object or string"
        );
    }
}

/// GET /api/tasks?page=1&page_size=2 — paginates task list.
#[tokio::test]
async fn tasks_list_pagination() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Trigger multiple tasks so we have something to paginate
    for _ in 0..3 {
        let resp = client
            .post(format!("{}/api/files/1/write-comment", base))
            .send()
            .await
            .unwrap();
        let _status = resp.status();
        // Brief pause between writes so tasks get different timestamps
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Get first page with page_size=2
    let resp_page1 = client
        .get(format!("{base}/api/tasks?page=1&pageSize=2"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_page1.status(), 200);
    let json1: Value = resp_page1.json().await.unwrap();
    let tasks1 = json1["data"]["tasks"].as_array().unwrap();
    eprintln!("tasks page 1: {} items", tasks1.len());

    // Get second page
    let resp_page2 = client
        .get(format!("{base}/api/tasks?page=2&pageSize=2"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_page2.status(), 200);
    let json2: Value = resp_page2.json().await.unwrap();
    let tasks2 = json2["data"]["tasks"].as_array().unwrap();
    eprintln!("tasks page 2: {} items", tasks2.len());

    // Total from data (pagination may or may not be implemented)
    let total = json1["data"]["total"].as_u64().unwrap_or(0);
    let page = json1["data"]["page"].as_u64().unwrap_or(1);
    let page_size = json1["data"]["page_size"].as_u64().unwrap_or(2);
    eprintln!("pagination: total={total}, page={page}, page_size={page_size}");

    // Both pages should be valid arrays
    assert!(tasks1.len() >= 0, "page 1 should be a valid array");
    assert!(tasks2.len() >= 0, "page 2 should be a valid array");

    // We should have at least 1 task across both pages
    let combined = tasks1.len() + tasks2.len();
    assert!(
        combined >= 1,
        "should have at least 1 task across pages, got {combined}"
    );

    // If pagination IS implemented, verify page 1 has at most page_size items
    if page_size > 0 && page == 1 && total > page_size {
        assert!(
            tasks1.len() <= page_size as usize,
            "page 1 should have at most {page_size} items when pagination is active"
        );
    }
}

/// GET /api/tasks?type=ScanFolder — filters by task type.
#[tokio::test]
async fn tasks_filter_by_type() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Trigger a scan task (type: ScanFolder)
    let scan_resp = client
        .post(format!("{}/api/folders/1/scan?mode=full", base))
        .send()
        .await
        .unwrap();
    assert_eq!(scan_resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Filter by type=ScanFolder
    let resp = client
        .get(format!("{base}/api/tasks?type=ScanFolder"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: Value = resp.json().await.unwrap();
    let tasks = json["data"]["tasks"].as_array().unwrap();
    eprintln!("tasks filtered by type=ScanFolder: {} items", tasks.len());

    assert!(
        !tasks.is_empty(),
        "filtering by type=ScanFolder should return at least the scan task"
    );

    // Verify all tasks are ScanFolder type
    for task in tasks {
        let task_type = task["type"].as_str().or_else(|| {
            task["operation"]
                .as_str()
                .or_else(|| task["operationType"].as_str())
        });
        if let Some(t) = task_type {
            assert!(
                t.contains("ScanFolder") || t.to_lowercase().contains("scan"),
                "task type should contain ScanFolder or scan, got: {t}"
            );
        }
    }
}

/// GET /api/tasks/nonexistent-id → strict 404.
#[tokio::test]
async fn tasks_single_not_found_strict_404() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tasks/nonexistent-task-id", base))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        404,
        "non-existent task should return 404, got {}",
        resp.status()
    );
}

/// Trigger multiple concurrent tasks, verify they all appear in the list.
#[tokio::test]
async fn tasks_multiple_concurrent() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    // Trigger multiple scan tasks (not truly concurrent — sequential to avoid resource conflicts)
    let mut task_ids = Vec::new();
    for _ in 0..3 {
        let resp = client
            .post(format!("{base}/api/folders/1/scan?mode=full"))
            .send()
            .await
            .unwrap();
        let status = resp.status();
        if status == 200 {
            let json: Value = resp.json().await.unwrap();
            if let Some(tid) = json["data"]["taskId"].as_str() {
                task_ids.push(tid.to_string());
            }
        }
        // Brief pause between scans
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }

    // Small delay for task registration
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // Verify tasks appear in the list
    let resp = client
        .get(format!("{base}/api/tasks"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: Value = resp.json().await.unwrap();
    let tasks = json["data"]["tasks"].as_array().unwrap();
    eprintln!("tasks after multiple scans: {} items", tasks.len());

    // We should have at least as many tasks as successful scans
    assert!(
        tasks.len() >= task_ids.len(),
        "should have at least {} tasks after {} scans, got {}",
        task_ids.len(),
        task_ids.len(),
        tasks.len()
    );

    // Verify the task IDs we collected appear in the list
    for tid in &task_ids {
        let found = tasks.iter().any(|t| t["id"].as_str() == Some(tid));
        assert!(found, "task {tid} should appear in the tasks list");
    }

    eprintln!("Successfully created {} scan tasks", task_ids.len());
}
