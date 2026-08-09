## Plan: integration-test-harness

**Status**: done ✅
**Branch**: `feat/integration-test-harness`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Build a self-contained Rust integration test harness. Replaces the curl-based
`test.sh` smoke tests with deterministic, fast tests that create a fresh SQLite
DB, run all migrations, seed hand-crafted data, hit every API endpoint with
every filter combination, and assert exact results. Run with `cargo test` — no
server needed, runs in seconds.

### Why

- Agents need fast, deterministic feedback. `cargo test` = single source of truth.
- curl-based tests against real data are fragile (data changes, manual server).
- 16 migrations, 14+ API endpoints, 50+ filter params — all untested.
- Every future plan MUST include tests (enforced by Section 1 Testing rules).

### Architecture: How `cargo test` creates a full app

1. **In-memory SQLite** with `datetime` → `UnixEpoch` conversion for
   `unixepoch()` compatibility (SQLite needs `DATETIME` format, Rust supplies
   Unix timestamps).
2. **Run all migrations** from `migrations/` directory, in order.
3. **Seed hand-crafted data** that exercises every edge case and view chain.
4. **Create a test `Router`** via a new `build_router()` function (extracted
   from `serve()` — see Phase 1).
5. **Hit endpoints with `reqwest`** (already a dependency), parse JSON
   responses, assert exact values.

#### Why in-memory SQLite instead of temp file?

In-memory with `datetime` type affinity works for all our use cases:
`unixepoch()` returns Unix timestamps, `datetime(unixepoch(), ...)` works.
Rust code stores `i64` Unix timestamps. The only caveat: `date('now')` returns
UTC date string, not Unix timestamp. Our queries use `unixepoch('now', ...)`
which works correctly in memory.

### Phase 1: Extract `build_router()` from `serve()`

**File**: `src/main.rs`

Move all `.route()` calls from `serve()` into a standalone function:

```rust
/// Build the Axum router from AppState. Extracted for testability.
/// Does NOT spawn background tasks (pollers, watchers, maintainer).
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(root_handler))
        .route("/api/storage/status", get(api::storage_status_handler))
        .route("/api/storage/prune-preview", post(api::prune_preview_handler))
        // ... all existing routes ...
        .fallback(get(static_handler))
        .layer(CorsLayer::permissive())
}
```

In `serve()`, replace inline router construction with `build_router(state.clone())`.

**This is a pure refactor — zero behavior change.**

### Phase 2: Test helpers

**File**: `tests/common/mod.rs`

```rust
use sqlx::{Pool, Sqlite, SqlitePool};
use axum::Router;
use std::sync::Arc;

/// Create an in-memory SQLite DB, run all migrations, return pool.
pub async fn create_test_db() -> Pool<Sqlite> {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    // Enable WAL + normal sync for in-memory (fast, no durability concern)
    sqlx::query("PRAGMA journal_mode=WAL").execute(&pool).await.unwrap();
    // Run all migration files in order
    run_migrations(&pool).await;
    pool
}

/// Run all .sql files from migrations/ in numeric order.
async fn run_migrations(pool: &Pool<Sqlite>) {
    let mut files: Vec<_> = std::fs::read_dir("migrations")
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |e| e == "sql"))
        .collect();
    files.sort();
    for path in &files {
        let sql = std::fs::read_to_string(path).unwrap();
        sqlx::query(&sql).execute(pool).await.unwrap();
    }
}

/// Build a test AppState with the given DB pool.
pub async fn test_app_state(pool: Pool<Sqlite>) -> Arc<AppState> {
    Arc::new(AppState {
        db: pool,
        config: ServiceCredentials::defaults_for_test(),
        task_manager: TaskManager::new(),
        embeddings: Mutex::new(None),
        category_means: Mutex::new(None),
        public_url: None,
    })
}

/// Create a full test app (DB + migrations + router).
pub async fn test_app() -> (Router, Pool<Sqlite>) {
    let pool = create_test_db().await;
    let state = test_app_state(pool.clone()).await;
    let router = momos_music_manager::build_router(state);
    (router, pool)
}
```

**Note**: `AppState` and `TaskManager` need to be `pub` (or at least
`pub(crate)`). If they aren't already, make them so.

### Phase 3: Domain test files

Each test file follows the same pattern:

```rust
// tests/api_files.rs
mod common;
use axum_test::TestServer;  // or axum::test helpers

#[tokio::test]
async fn files_list_returns_paginated_results() {
    let (app, pool) = common::test_app().await;
    common::seed_basic_files(&pool).await;  // inserts 5 files

    let server = TestServer::new(app).unwrap();
    let resp = server.get("/api/files?limit=3").await;
    resp.assert_status_ok();

    let json: serde_json::Value = resp.json();
    let files = json["data"].as_array().unwrap();
    assert_eq!(files.len(), 3, "limit=3 should return 3 files");
}
```

#### Test files to create

| File                           | Covers                                                                       |
| ------------------------------ | ---------------------------------------------------------------------------- |
| `tests/common/mod.rs`          | DB creation, migration runner, seed helpers, test app factory                |
| `tests/migration_integrity.rs` | All 16 migrations run cleanly, schema has expected tables/views              |
| `tests/api_files.rs`           | All `FilesQuery` params: `isLocal`, `backedUp`, `safeToDelete`, `fileTypes`, |
|                                | `search`, `sort`, `order`, `tags`, `pmvCategories`, `pmvAggregate`,          |
|                                | `commentStatuses`, `linkedOnly`, `nonDefaultOnly`, `untaggedOnly`,           |
|                                | `keys`, plus count endpoint parity                                           |
| `tests/api_tracks.rs`          | All `TracksQuery` params: `services`, `fileTypes`, `fileTypeAgg`,            |
|                                | `hasLocal`, `hasBackup`, `playlists`, `tags`, `search`, `sort`,              |
|                                | plus `/api/tracks/{id}/detail` with `inBackpack` + WAV variants,             |
|                                | plus count endpoint parity                                                   |
| `tests/api_playlists.rs`       | `service` filter, `search`, `archive` filter, count, pagination              |
| `tests/api_tags.rs`            | `search`, `sort`, `categoryId`, `backpack` toggle (`PUT`), count             |
| `tests/api_storage.rs`         | `GET /api/storage/status` (all fields), `POST /api/storage/prune-preview`    |
|                                | (hasStemVariant, reasons), `POST /api/storage/prune` (if safe)               |
| `tests/api_folders.rs`         | Folder list, folder detail, backup config update                             |
| `tests/api_tasks.rs`           | Task list, task detail                                                       |
| `tests/api_digging.rs`         | `POST /api/digging/suggest` (seed tag + seed IDs), `/api/files/{id}/stream`  |
| `tests/api_file_variants.rs`   | `GET /api/files/{id}/variants` (stemType, WAV source grouping)               |

**Each test file is ~100–300 lines.** Total: ~2,000 lines of test code.

### Seed data design principles

- **Minimal, hand-crafted rows.** ~30–50 rows total across all tables.
- **One edge case per row.** A file with ISRC but no stem variant. A file with
  stem variant. A file backed up but not local. A WAV with `source_of`. A tag
  with `backpack=true`. A track in multiple playlists.
- **Deterministic IDs.** No `AUTOINCREMENT` guessing — seed inserts include
  explicit IDs where needed for cross-table references.
- **Reusable seed functions.** `common::seed_basic_files()`,
  `common::seed_track_with_variants()`, etc. Tests compose them.

### How tests stay up-to-date

**Mechanism 1: Hard rule in Section 1.** Every plan that touches an API endpoint
or filter MUST include "add/update integration test" as acceptance criterion.
Agents are instructed to enforce this.

**Mechanism 2: Obvious file placement.** The test file name mirrors the API path:
`tests/api_files.rs` ↔ `/api/files*`. When an agent modifies `src/api.rs`'s file
handlers, the corresponding test file is unambiguous.

**Mechanism 3: Meta-tests.** Each test file has a "count" test that asserts the
number of top-level filter params. If a param is added to the query struct but
no test exercises it, the count changes and the meta-test fails. Example:

```rust
#[test]
fn all_files_query_params_have_coverage() {
    // grep FilesQuery fields and compare to test function count
    // This is a canary — fails when a param is added without a test
}
```

This is a lightweight lint, not a full coverage tool. If it becomes annoying,
remove it — the hard rule (Mechanism 1) is the real enforcement.

**Mechanism 4: Migration integrity test.** `tests/migration_integrity.rs` creates
a fresh DB and runs all migrations. If a migration breaks (wrong order, syntax
error, missing dependency), this test catches it before any other test runs.

### Dependencies

No new dependencies. `reqwest` is already in `Cargo.toml`. `axum::test` is
built-in (behind the `axum/test` feature, enable if needed). Alternatively,
`axum_test` crate for ergonomic `TestServer` — or just use `reqwest` against a
bound port with `tokio::net::TcpListener`.

### Files to create

- `tests/common/mod.rs` — test helpers, migration runner, seed functions
- `tests/migration_integrity.rs` — migration chain test
- `tests/api_files.rs` — files endpoint tests
- `tests/api_tracks.rs` — tracks endpoint tests
- `tests/api_playlists.rs` — playlists endpoint tests
- `tests/api_tags.rs` — tags endpoint tests
- `tests/api_storage.rs` — storage endpoint tests
- `tests/api_folders.rs` — folders endpoint tests
- `tests/api_tasks.rs` — tasks endpoint tests
- `tests/api_digging.rs` — digging suggest + audio stream tests
- `tests/api_file_variants.rs` — file variants endpoint tests

### Files to modify

- `src/main.rs` — extract `build_router()` from `serve()`; make `AppState` fields `pub`
- `src/config.rs` — add `ServiceCredentials::defaults_for_test()` (or `#[cfg(test)]` constructor)
- `Cargo.toml` — enable `axum/test` feature if not already (check)

### Acceptance Criteria

- [ ] `build_router()` extracted; `serve()` delegates to it; `cargo build` passes
- [ ] `cargo test` creates fresh in-memory DB, runs all 16 migrations, no errors
- [ ] `tests/migration_integrity.rs`: asserts all expected tables + views exist
- [ ] `tests/api_files.rs`: ≥15 tests covering every `FilesQuery` param + count parity
- [ ] `tests/api_tracks.rs`: ≥12 tests covering every `TracksQuery` param + detail endpoint + count parity
- [ ] `tests/api_playlists.rs`: ≥5 tests (list, filter, search, archive, pagination)
- [ ] `tests/api_tags.rs`: ≥5 tests (list, search, sort, backpack toggle, count)
- [ ] `tests/api_storage.rs`: ≥4 tests (status fields, prune-preview hasStemVariant, prune-preview reasons)
- [ ] `tests/api_digging.rs`: ≥3 tests (seed by tag, seed by file IDs, audio stream range request)
- [ ] `tests/api_file_variants.rs`: ≥3 tests (stem variants, WAV source grouping, no-variants)
- [ ] All tests pass: `cargo test` exits 0
- [ ] `cargo test` completes in <10 seconds
- [ ] `test.sh` still works as legacy smoke test (no changes needed to test.sh)

---

