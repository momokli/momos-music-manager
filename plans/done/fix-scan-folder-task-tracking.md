## Plan: fix-scan-folder-task-tracking

**Status**: done ✅
**Branch**: `fix/scan-folder-task-tracking`
**Ready for review**: no
**Depends on**: `feat/test-coverage-100`
**Migration needed**: no

### Description

The `scan_folder_handler` in `src/api.rs` uses a raw `tokio::spawn` instead of
the TaskManager, making folder scans invisible to `/api/tasks` and the Tasks
page UI. Every other async operation (write comment, backup, prune, sync)
properly uses `start_*_task()` — this is the only outlier.

### Root cause

`src/api.rs` line 6910:

```rust
tokio::spawn(async move {
    match scan_folder(&db, id, scan_mode).await {
        Ok(file_count) => tracing::info!("Scanned {} files", file_count),
        Err(e) => tracing::error!("Failed to scan folder {}: {}", id, e),
    }
});
```

`start_scan_folder_task()` already exists in `src/tasks/mod.rs` (line 1479)
and supports `ScanMode`. The handler just isn't using it.

### Fix

**File**: `src/api.rs` — replace the `tokio::spawn` block in `scan_folder_handler`

```rust
// Use TaskManager so the task appears in /api/tasks and the Tasks UI
let task_id = match crate::tasks::start_scan_folder_task(
    &state.task_manager,
    &state.db,
    id,
    scan_mode,
).await {
    Ok(id) => id,
    Err(e) => return internal_error(e).into_response(),
};

Json(ApiResponse {
    data: serde_json::json!({
        "taskId": task_id,
        "folderId": id,
        "mode": if matches!(scan_mode, crate::db::ScanMode::Full) { "full" } else { "incremental" }
    }),
})
.into_response()
```

Also remove the unused `tokio::spawn` and the manual tracing calls (the task
worker handles those).

**File**: `tests/api_tasks.rs` — update `tasks_list_with_task`

Currently triggers a write-comment task to populate the task list. Now that
scan tasks appear, prefer scanning (it's a more natural fit for this test):

```rust
// Trigger a scan task on folder 1 (path doesn't exist, task will register)
let scan_resp = client
    .post(format!("{base}/api/folders/1/scan?mode=full"))
    .send().await.unwrap();
assert_eq!(scan_resp.status(), 200);
let scan_json: serde_json::Value = scan_resp.json().await.unwrap();
let task_id = scan_json["data"]["taskId"].as_str().unwrap();

// Verify the task appears in the list
let tasks_resp = client.get(format!("{base}/api/tasks")).send().await.unwrap();
let tasks_json: serde_json::Value = tasks_resp.json().await.unwrap();
let tasks = tasks_json["data"].as_array().unwrap();
assert!(!tasks.is_empty(), "scan task should appear in task list");
```

### Acceptance Criteria

- [ ] `scan_folder_handler` uses `start_scan_folder_task()` instead of `tokio::spawn`
- [ ] Response includes `taskId` for frontend progress polling
- [ ] `POST /api/folders/1/scan?mode=full` returns a taskId visible in `GET /api/tasks`
- [ ] All 190 existing tests still pass
- [ ] `cargo build` passes

---

