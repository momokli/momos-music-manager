# Task Manager Architecture

## Overview

The Task Manager provides a generic, in-memory task tracking system for background operations. It replaces the old `SyncManager` which was Spotify-specific.

## Core Concepts

### TaskType

```rust
pub enum TaskType {
    SpotifySync(SyncConfig),
    WriteComment { file_ids: Vec<i64> },
}
```

Each variant defines its own configuration and worker logic.

### TaskStatus

```rust
pub enum TaskStatus {
    Pending,    // Queued but not started
    Running,    // Currently executing
    Completed,  // Finished successfully
    Failed,     // Finished with error
    Cancelled,  // Manually cancelled
}
```

### Task

```rust
pub struct Task {
    pub id: String,                    // UUID
    pub task_type: TaskType,
    pub status: Arc<RwLock<TaskStatus>>,
    pub progress: Arc<RwLock<String>>, // Human-readable progress
    pub logs: Arc<RwLock<VecDeque<String>>>,
    pub created_at: Instant,
    pub cancel_token: CancellationToken,
}
```

### TaskManager

- Stores `HashMap<String, Task>` in `Arc<RwLock<...>>`
- Methods: `start_task`, `get_task`, `cancel_task`, `list_tasks`, `list_tasks_paginated`
- One task per type enforcement (optional, configurable)

## API Endpoints

```
GET    /api/tasks              → List all tasks (paginated, 50 per page)
GET    /api/tasks?status=active → Filter by status
GET    /api/tasks/{id}         → Get single task with full logs
DELETE /api/tasks/{id}         → Cancel a running task
```

### Pagination

- Query params: `limit` (default 50), `offset` (default 0), `search` (optional, filters on type and logs)
- Response: `{ data: { tasks: [...], total: number, limit: number, offset: number } }`

## WriteComment Task

### Single file

```
POST /api/files/{id}/write-comment
→ { "data": { "task_id": "uuid" } }
```

### Batch (all files needing update)

```
POST /api/files/write-comments
→ { "data": { "task_id": "uuid" } }
```

### Worker algorithm for each file:
1. `compute_target_comment(pool, file_id)` → target string
2. If file.comment == target → skip, log "already up to date", success
3. If file path doesn't exist → log error, continue
4. `write_comment_to_file(path, target)` → exiftool
5. `update_file_comment(pool, file_id, target)` → DB
6. If DB update fails → log warning, mark completed with warning
7. Log progress: "Writing file 3/12: Talk To Me..."

## Task Lifecycle & Retention

- Tasks are stored in memory only
- No automatic cleanup — all tasks remain viewable
- Tasks persist until server restart
- Future enhancement: persist to database, auto-cleanup completed tasks

## Migration from SyncManager

- Old `src/sync/mod.rs` content → `src/tasks/sync_worker.rs`
- Old `SyncManager` struct → generic `TaskManager` in `src/tasks/mod.rs`
- `AppState.sync_manager` → `AppState.task_manager`
- Old API routes (e.g., `/api/services/spotify/sync/{task_id}`) → delegate to TaskManager internally
