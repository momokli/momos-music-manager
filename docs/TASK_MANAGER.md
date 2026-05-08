# Task Manager Architecture

## Overview

The Task Manager provides a generic, in-memory task tracking system for background operations. It replaces the old `SyncManager` which was Spotify-specific.

## Core Concepts

### TaskType

```rust
pub enum TaskType {
    ServiceSync { service: String, operation: SyncType },
    WriteComment { file_ids: Vec<i64> },
    RecomputeEmbeddings,
    ScanFolder { folder_id: i64 },
}
```

Each variant defines its own configuration and worker logic.

### SyncType (for ServiceSync)

```rust
pub enum SyncType {
    Full,              // Sync all playlists
    SinglePlaylist { playlist_id: String },  // Sync one playlist
}
```

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
    pub id: String,                    // UUID v4
    pub task_type: TaskType,
    pub status: Arc<RwLock<TaskStatus>>,
    pub progress: Arc<RwLock<TaskProgress>>,
    pub created_at: Instant,
    pub cancel_token: CancellationToken,
    pub completed_at: Arc<RwLock<Option<Instant>>>,
}
```

### TaskProgress

```rust
pub struct TaskProgress {
    pub percent: u8,                          // 0–100
    pub text: String,                         // Human-readable status
    pub sub_items: Vec<ProgressItem>,          // Detail breakdown
}

pub struct ProgressItem {
    pub label: String,
    pub status: String,   // "pending" | "running" | "completed" | "failed" | "skipped"
    pub detail: Option<String>,
}
```

### TaskManager

- Stores `HashMap<String, Task>` in `Arc<RwLock<...>>`
- Methods: `start_task`, `start_task_unique` (rejects duplicates via 409), `get_task`, `cancel_task`, `list_tasks`, `list_tasks_paginated`
- Background pruner: removes completed/failed/cancelled tasks older than 5 minutes, runs every 60 seconds

## API Endpoints

```
GET    /api/tasks               → Paginated list (50/page)
                                 ?limit=50&offset=0&search=&status=active
                                 Returns tasks with `percent` and `subItems`

GET    /api/tasks/{id}          → Single task with full logs
DELETE /api/tasks/{id}          → Cancel a running task
```

### Conflict Prevention (409 Conflict)

- `ServiceSync` → one per service (e.g. one Spotify sync at a time)
- `ScanFolder` → one per folder
- `RecomputeEmbeddings` → only one global
- `WriteComment` → no constraint (can be concurrent)

## WriteComment Task

### Single file

```
POST /api/files/{id}/sync-comment
→ { "data": { "task_id": "uuid" } }
```

### Batch (all files needing update)

```
POST /api/files/bulk-sync
→ { "data": { "task_id": "uuid" } }
```

### Worker algorithm (per file):

1. `compute_target_comment(pool, file_id)` → target string
2. If `file.comment == target` → skip, log "already up to date"
3. If file path doesn't exist → log error, continue
4. `write_comment_to_file(path, target)` → exiftool
5. `update_file_comment(pool, file_id, target)` → DB
6. Log progress: "Writing file 3/12: Talk To Me..."

## ServiceSync Task (Spotify)

Triggered via:

```
POST /api/services/spotify/sync?type=full
POST /api/services/spotify/sync?type=single&playlist_id=xxx
```

### Full sync algorithm:

1. Fetch all user playlists from Spotify API (with pagination)
2. For each playlist:
   - Upsert playlist in `service_playlists` table
   - Fetch all tracks (with pagination)
   - Upsert tracks in `service_tracks` table
   - Link tracks to playlist in `service_playlist_tracks`
3. Update progress per playlist
4. Handle cancellation via `CancellationToken`

## RecomputeEmbeddings Task

Triggered via:

```
POST /api/tags/embeddings/recompute
→ { "data": { "task_id": "uuid" } }
```

### Algorithm:

1. Load embedding model (all-MiniLM-L6-v2 via candle)
2. For each tag (batch of 10):
   - Compute 384-dim f32 embedding vector
   - Store in `tag_embeddings` table
   - Update progress (per-10-tags)
3. After all tags, compute mean category embeddings

## ScanFolder Task

Triggered via:

```
POST /api/folders/{id}/scan
→ { "data": { "task_id": "uuid" } }
```

### Algorithm:

1. Walk directory tree (configurable depth/exclusions)
2. For each audio file:
   - Extract metadata via lofty + exiftool
   - Store/update in `files` table
   - Update progress
3. Remove files from database that no longer exist on disk

## Task Lifecycle & Retention

- Tasks are stored in memory only (no persistence across restarts)
- Background pruner runs every 60 seconds
- Completed/failed/cancelled tasks removed after 5 minutes
- Running/pending tasks survive until completion or server restart

## Migration from SyncManager

- Old `src/sync/mod.rs` → refactored into `src/tasks/mod.rs`
- Old `SyncManager` struct → generic `TaskManager`
- `AppState.sync_manager` → `AppState.task_manager`
- Sync of all services → `ServiceSync` task type with `SyncType`
- Added `ScanFolder`, `RecomputeEmbeddings` task types
- Added progress tracking with `percent` + `sub_items` for UI progress bars

## Related Files

- `src/tasks/mod.rs` — TaskManager, task types, all worker implementations
- `src/api.rs` — API endpoints for task management
- `src/spotify/sync_worker.rs` — SpotifySyncWorker (called by ServiceSync task)
- `frontend/pages/tasks.js` — Frontend task manager UI with polling
