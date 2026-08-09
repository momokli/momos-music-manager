## Plan: purge-ghost-records

**Status**: proposed
**Branch**: `feat/purge-ghost-records`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `021_folder_id_on_files.sql`

### Description

Two-part solution for files that don't belong to any tracked folder:

**Part A (long-term)**: Add `folder_id` to the `files` table — an explicit FK
linking each file to the folder that discovered it. The scanner sets it on
insert/update. Replaces the implicit path-prefix relationship used everywhere.

**Part B (immediate utility)**: Add orphan detection + purge endpoint + Storage
page UI. Files with `folder_id IS NULL` (not claimed by any folder) are
"ghost records". The user can list and purge them.

### Why

Currently the file→folder relationship is implicit: `WHERE file_path LIKE
folder_path || '%'`. This breaks when:

1. A DB is imported from one machine to another with different paths
2. A folder's path is changed — all its files become invisible to folder stats
3. A folder is deleted without cleaning up its files

There are 17 places in the codebase doing path-prefix matching
(`src/db/folders.rs` ×12, `src/db/storage.rs` ×5, `src/tasks/mod.rs` ×1).
Every one of these is a latent bug waiting for a path change.

An explicit FK solves this permanently: change the folder path, files stay
linked. Delete a folder, files become orphaned (FK `ON DELETE SET NULL`) —
visible in the UI for purge.

### Investigation Results (2026-06-16)

**Current DB state**:

| Metric              | Value  |
| ------------------- | ------ |
| Tracked files       | 10,258 |
| Local files         | 4,159  |
| Tracked folders     | 2      |
| Orphaned files      | 0      |
| Path-prefix matches | 17     |

**Implicit relationship sites** (all use `file_path LIKE path || '%'`
or `substr(file_path, 1, length(folder_path)) = folder_path`):

| File            | Function                         | Line  |
| --------------- | -------------------------------- | ----- |
| `db/folders.rs` | `scan_folder` stale cleanup      | 338   |
| `db/folders.rs` | `get_folder_file_count`          | 375   |
| `db/folders.rs` | `get_folder_stats` total_files   | 421   |
| `db/folders.rs` | `get_folder_stats` total_size    | 429   |
| `db/folders.rs` | `get_folder_stats` stems count   | 437   |
| `db/folders.rs` | `get_folder_stats` flacs count   | 445   |
| `db/folders.rs` | `get_folder_stats` wavs count    | 453   |
| `db/folders.rs` | `get_folder_stats` mp3s count    | 461   |
| `db/folders.rs` | `get_folder_stats` other count   | 469   |
| `db/folders.rs` | `get_folder_backup_status` count | 480   |
| `db/folders.rs` | `get_folder_backup_status` size  | 491   |
| `db/folders.rs` | WAV source dirs count            | 517   |
| `db/folders.rs` | WAV backed up count              | 527   |
| `db/storage.rs` | `get_prune_candidates`           | ~309  |
| `db/storage.rs` | `get_unbacked_up_files`          | ~466  |
| `db/storage.rs` | `clear_backup_status`            | ~510  |
| `db/storage.rs` | Another prune/storage query      | ~700  |
| `tasks/mod.rs`  | `ScanWavSources` file listing    | ~2966 |

### Part A: Add `folder_id` to Files (schema + scanner)

#### Migration 021 (`migrations/021_folder_id_on_files.sql`)

```sql
-- Add explicit folder_id FK to files table.
-- Replaces implicit path-prefix matching for file→folder relationship.
ALTER TABLE files ADD COLUMN folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_files_folder_id ON files(folder_id);

-- Backfill: set folder_id for all existing files based on path prefix.
-- A file belongs to the longest-matching folder path.
UPDATE files
SET folder_id = (
    SELECT fol.id FROM folders fol
    WHERE files.file_path LIKE (fol.folder_path || '/%')
       OR files.file_path = fol.folder_path
    ORDER BY length(fol.folder_path) DESC
    LIMIT 1
);

SELECT 'Migration 021 applied: folder_id on files with backfill' as status;
```

The `ORDER BY length(fol.folder_path) DESC` handles nested folders:
`/Music/stems/Artist/file.wav` matches both `/Music` and `/Music/stems` —
picks the most specific one. Files matching NO folder get `folder_id = NULL`
(these become the orphaned "ghost records").

#### Rust: `File` struct (`src/db/types.rs`)

Add after `last_verified_local`:

```rust
/// The folder that discovered/tracks this file. NULL = orphaned (not
/// tracked by any active folder — import artifact or deleted folder).
pub folder_id: Option<i64>,
```

Since `folder_id` is `Option<i64>`, SQL `NULL` maps to `None` automatically
via sqlx. No `#[sqlx(default)]` needed — sqlx handles nullable columns natively.

#### Rust: Scanner changes

**`src/db/files.rs` — `scan_and_store_file()`**:

Add `folder_id: Option<i64>` parameter. Include in INSERT and ON CONFLICT
UPDATE:

```rust
pub async fn scan_and_store_file(
    pool: &Pool<Sqlite>,
    path: &Path,
    folder_id: Option<i64>,
) -> Result<File>
```

In INSERT columns: add `folder_id`.
In VALUES: add `.bind(folder_id)`.
In ON CONFLICT UPDATE: `folder_id = COALESCE(excluded.folder_id, files.folder_id)`
(preserve existing folder_id on re-scan from a different folder).

**`src/db/files.rs` — `scan_directory_with_config()`**:

Add `folder_id: Option<i64>` parameter. Pass through to `scan_and_store_file()`.

**`src/db/files.rs` — `scan_directory()` wrapper (line 739)**:

This convenience wrapper calls `scan_directory_with_config()`. Pass `None`:

```rust
pub async fn scan_directory(pool: &Pool<Sqlite>, dir_path: &Path) -> Result<usize> {
    scan_directory_with_config(
        pool, dir_path, true, false, String::new(), 0,
        ScanMode::Full, None,  // ← new: folder_id = None
    ).await
}
```

**All call sites that need updating** (6 sites, verified via grep):

| File:Line                           | Caller                                     | folder_id value                   |
| ----------------------------------- | ------------------------------------------ | --------------------------------- |
| `db/folders.rs:309`                 | `scan_folder()`                            | `Some(folder_id)`                 |
| `tasks/mod.rs:1581`                 | `start_scan_folder_task` worker            | `Some(folder.id)`                 |
| `db/files.rs:865`                   | Inside `scan_directory_with_config()` loop | Pass through from param           |
| `db/files.rs:741`                   | `scan_directory()` wrapper                 | `None`                            |
| `main.rs:504`                       | `scan_file` CLI subcommand                 | `None`                            |
| `main.rs:463` (indirect, delegates) | `scan_directory` CLI subcommand            | (delegates to db::scan_directory) |

**`src/watch.rs` — folder watcher**: No change needed — it calls
`start_scan_folder_task()`, not raw scan functions. The task worker
already handles the `folder_id`.

**`src/maintainer.rs` — maintainer**: Same — calls `start_scan_folder_task()`.

### Part B: Orphan Detection + Purge

#### New DB functions: `src/db/files.rs`

```rust
/// Files not tracked by any folder (folder_id IS NULL).
/// These are import artifacts or remnants of deleted folders.
pub async fn get_orphaned_file_count(pool: &Pool<Sqlite>) -> Result<i64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE folder_id IS NULL"
    )
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// Purge all orphaned files + their dependent records.
/// Deletes from: file_locations, file_resolved_tags, track_resolved_tags, files.
/// Returns count of deleted files.
pub async fn purge_orphaned_files(pool: &Pool<Sqlite>) -> Result<i64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE folder_id IS NULL"
    )
    .fetch_one(pool)
    .await?;

    if count == 0 {
        return Ok(0);
    }

    // Delete in dependency order (respect FK constraints):
    // 1. file_locations (FK to files)
    sqlx::query(
        "DELETE FROM file_locations WHERE file_id IN (SELECT id FROM files WHERE folder_id IS NULL)"
    )
    .execute(pool)
    .await?;

    // 2. file_resolved_tags (FK to files)
    sqlx::query(
        "DELETE FROM file_resolved_tags WHERE file_id IN (SELECT id FROM files WHERE folder_id IS NULL)"
    )
    .execute(pool)
    .await?;

    // 3. track_resolved_tags (FK to service_tracks, may reference orphaned track links)
    // Only needed if orphaned files have linked tracks. Safe no-op otherwise.
    sqlx::query(
        "DELETE FROM track_resolved_tags WHERE track_id IN (
            SELECT vft.track_id FROM v_file_track_link vft
            WHERE vft.file_id IN (SELECT id FROM files WHERE folder_id IS NULL)
        )"
    )
    .execute(pool)
    .await?;

    // 4. files themselves
    sqlx::query("DELETE FROM files WHERE folder_id IS NULL")
        .execute(pool)
        .await?;

    Ok(count)
}
```

#### New API endpoint: `POST /api/storage/purge-orphans`

**File**: `src/api/storage.rs`

```rust
#[derive(Debug, Deserialize)]
struct PurgeOrphansRequest {
    confirm: bool,
}

async fn purge_orphans_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PurgeOrphansRequest>,
) -> impl IntoResponse {
    if !body.confirm {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                data: serde_json::json!({"error": "Must set confirm=true to purge orphaned files"}),
            }),
        )
            .into_response();
    }

    match crate::db::files::purge_orphaned_files(&state.db).await {
        Ok(count) => Json(ApiResponse {
            data: serde_json::json!({"purged": count}),
        })
        .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}
```

Route:

```rust
.route("/api/storage/purge-orphans", post(purge_orphans_handler))
```

#### Extended Storage Status

Add `orphaned_file_count: i64` to `StorageStatus` (in `src/db/storage.rs`).
Populate in `get_storage_status()` from `crate::db::files::get_orphaned_file_count()`.

### Frontend: Storage Page Orphan Section

**File**: `frontend/pages/storage.js`

Add a card to the Storage page, shown ONLY when `orphanedFileCount > 0`:

```html
<div class="card" id="orphan-card">
  <h3><i class="fas fa-ghost"></i> Ghost Records</h3>
  <p class="help-text">
    These files are in the database but not tracked by any active folder. They're
    typically import artifacts from a different machine.
  </p>
  <div class="storage-metric">
    <span class="metric-value">${orphanedFileCount}</span>
    <span class="metric-label">orphaned files</span>
  </div>
  <button class="btn btn-danger" id="purge-orphans-btn">
    <i class="fas fa-eraser"></i> Purge Ghost Records
  </button>
  <p class="help-text" style="margin-top:0.5rem">
    ⚠️ This permanently deletes these records from the database. Backed-up files on the
    NAS are not affected.
  </p>
</div>
```

Click handler:

1. Show confirmation dialog: "Permanently delete N orphaned records?"
2. `POST /api/storage/purge-orphans` with `{ confirm: true }`
3. On success: toast "Purged N ghost records", refresh page, card disappears
4. On error: toast with error message

### Files to create

- `migrations/021_folder_id_on_files.sql` — new migration

### Files to modify

| File                             | Change                                                                                                                                                             |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `src/db/types.rs`                | Add `folder_id: Option<i64>` to `File` struct                                                                                                                      |
| `src/db/files.rs`                | Add `folder_id` param to `scan_and_store_file()` + `scan_directory_with_config()` + `scan_directory()`; add `get_orphaned_file_count()` + `purge_orphaned_files()` |
| `src/db/folders.rs`              | Pass `Some(folder_id)` in `scan_folder()`                                                                                                                          |
| `src/tasks/mod.rs`               | Pass `Some(folder.id)` in `start_scan_folder_task` worker (line ~1581)                                                                                             |
| `src/db/storage.rs`              | Add `orphaned_file_count` to `StorageStatus` + populate in `get_storage_status()`                                                                                  |
| `src/main.rs`                    | Pass `None` in `scan_file` subcommand (line ~504); `scan_directory()` delegates unchanged                                                                          |
| `src/api/storage.rs`             | Add `purge_orphans_handler` + route                                                                                                                                |
| `frontend/pages/storage.js`      | Add orphan card (conditional on `orphanedFileCount > 0`)                                                                                                           |
| `frontend/style.css`             | `.btn-danger` styles (verify existing, add if missing)                                                                                                             |
| `tests/api_storage.rs`           | Integration tests: orphan count, purge with/without confirm, purge empty, orphan count after folder delete (see test list below)                                   |
| `frontend/tests/storage.spec.js` | Playwright: orphan card appears/disappears, purge flow                                                                                                             |

### TDD: Specific Tests

Tests are written FIRST and must fail before implementation.

#### Unit tests (Agent A) — `src/db/files.rs` `#[cfg(test)]`:

| #   | Test name                                         | What it proves                                                         |
| --- | ------------------------------------------------- | ---------------------------------------------------------------------- |
| 1   | `test_migration_021_folder_id_backfill`           | Backfill sets folder_id on all files matching a folder prefix          |
| 2   | `test_migration_021_no_folder_null_for_matched`   | Files under tracked folders get non-NULL folder_id                     |
| 3   | `test_migration_021_orphan_when_no_match`         | Files not under any folder get folder_id=NULL                          |
| 4   | `test_migration_021_nested_folders_longest_match` | File under deepest subfolder gets most specific parent                 |
| 5   | `test_purge_orphaned_files_empty`                 | Returns 0 when no orphans exist (no-op)                                |
| 6   | `test_purge_orphaned_files_with_orphans`          | Purges file_locations → file_resolved_tags → files in correct FK order |
| 7   | `test_purge_orphaned_files_preserves_claimed`     | Files with folder_id set are not deleted                               |
| 8   | `test_scan_and_store_file_preserves_folder_id`    | COALESCE keeps existing folder_id on re-scan                           |

#### Integration tests (Agent B) — `tests/api_storage.rs`:

| #   | Test name                                  | What it proves                                                  |
| --- | ------------------------------------------ | --------------------------------------------------------------- |
| 1   | `storage_orphan_count_when_none`           | `orphanedFileCount` = 0 when all files are claimed              |
| 2   | `storage_orphan_count_after_folder_delete` | Count increases when a tracked folder is deleted                |
| 3   | `storage_purge_orphans_no_confirm`         | 400 with error message when `confirm` is false or missing       |
| 4   | `storage_purge_orphans_confirm`            | 200 + `{"purged": N}` when confirm=true                         |
| 5   | `storage_purge_orphans_idempotent`         | Second purge after first returns `{"purged": 0}`                |
| 6   | `storage_status_includes_orphaned_count`   | `GET /api/storage/status` response includes `orphanedFileCount` |

#### Playwright tests (Agent C) — `frontend/tests/storage.spec.js`:

| #   | Test name                            | What it proves                                       |
| --- | ------------------------------------ | ---------------------------------------------------- |
| 1   | `ghost card hidden when no orphans`  | Orphan card is NOT rendered when orphanedFileCount=0 |
| 2   | `ghost card visible with orphans`    | Orphan card appears when orphaned files exist        |
| 3   | `purge button shows confirmation`    | Clicking purge opens confirmation dialog             |
| 4   | `purge succeeds and card disappears` | After confirm, card is removed and toast appears     |

### Acceptance Criteria

**Part A (folder_id):**

- [ ] Migration 021 runs cleanly on fresh DB (001→021)
- [ ] Migration 021 runs cleanly on existing DB — all files get correct `folder_id`
- [ ] Files not under any folder get `folder_id = NULL`
- [ ] `File` struct has `folder_id: Option<i64>` field
- [ ] `scan_and_store_file()` accepts + stores `folder_id`
- [ ] `COALESCE(excluded.folder_id, files.folder_id)` preserves FK on re-scan
- [ ] All 6 call sites pass correct `folder_id` (Some or None)
- [ ] Existing path-prefix queries still work (backfill guarantees consistency)
- [ ] `cargo build` passes
- [ ] `cargo test` passes (all existing + 8 new unit tests)

**Part B (orphan purge):**

- [ ] `get_orphaned_file_count()` returns count of files with `folder_id IS NULL`
- [ ] `purge_orphaned_files()` deletes dependent records in correct FK order
- [ ] `purge_orphaned_files()` returns 0 when no orphans exist
- [ ] `POST /api/storage/purge-orphans` with `confirm: true` purges and returns count
- [ ] `POST /api/storage/purge-orphans` without `confirm: true` returns 400
- [ ] `GET /api/storage/status` includes `orphanedFileCount`
- [ ] Storage page shows ghost records card when count > 0, hides when 0
- [ ] Purge button → confirmation dialog → success toast → card disappears
- [ ] `cargo build` passes
- [ ] `cargo test` passes (all existing + 6 new integration tests)
- [ ] `cd frontend && npx playwright test` passes (all existing + 4 new)

### Phase 3 (follow-up plan, out of scope): Replace all path-prefix queries

Once `folder_id` is populated and trusted, replace all 17 `file_path LIKE
folder_path || '%'` / `substr(file_path, 1, length(folder_path)) = folder_path`
sites with `WHERE folder_id = ?`. This is a pure refactor — the backfill
guarantees equivalent results. Also provides query performance improvement
(equality on indexed column vs prefix LIKE).

### Agent Decomposition (TDD, 3 agents, zero file conflicts)

| Agent | Files                                                                                                 | Work                                                                              | Tests          |
| ----- | ----------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- | -------------- |
| **A** | `migrations/021_*.sql`, `src/db/types.rs`, `src/db/files.rs`, `src/db/folders.rs`, `src/tasks/mod.rs` | Migration + File struct + scanner changes + purge DB functions + all 6 call sites | ~8 unit        |
| **B** | `src/db/storage.rs`, `src/main.rs`, `src/api/storage.rs`, `tests/api_storage.rs`                      | StorageStatus + main.rs scan-file + purge endpoint + integration tests            | ~6 integration |
| **C** | `frontend/pages/storage.js`, `frontend/style.css`, `frontend/tests/storage.spec.js`                   | Orphan card + purge button + Playwright tests                                     | ~4 Playwright  |

Write scope verification — zero overlap:

- Agent A: `src/db/types.rs`, `src/db/files.rs`, `src/db/folders.rs`, `src/tasks/mod.rs`, `migrations/`
- Agent B: `src/db/storage.rs`, `src/main.rs`, `src/api/storage.rs`, `tests/`
- Agent C: `frontend/` only

All 3 agents can run in parallel.

### Per-Agent Task Briefs

Each agent must:

1. Read the relevant source files to understand existing patterns
2. Write tests FIRST (they will fail) — see named test list above
3. Implement the fix
4. Run `cargo test --lib` (Agent A) or `cargo test --test api_storage` (Agent B)
   or `npx playwright test -- tests/storage.spec.js` (Agent C)
5. Run `cargo build` to verify compilation
6. Report back with test results

**Agent A note**: Update `src/tasks/mod.rs` line ~1581 in the `start_scan_folder_task`
worker to pass `Some(folder.id)` to `scan_directory_with_config()`. The `folder`
variable is already fetched at line 1516.

---

