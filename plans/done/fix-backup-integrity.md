## Plan: fix-backup-integrity

**Status**: done ✅
**Branch**: `fix/backup-integrity`
**Ready for review**: yes
**Depends on**: nothing (branches from `main`)
**Migration needed**: no (data repair via code path, not SQL migration)

### Description

Fix four data integrity issues discovered during a health-check investigation
of the backup/prune system (2026-06-10). The system is **safe** — no data loss
is possible through the prune path — but there are shortcuts and metadata
accuracy problems that could lead to operational issues over time.

### Investigation Findings

**Dataset**: 16,538 tracked files, 2,351 local, 14,896 backed up, 0 prune candidates.

| #   | Issue                                                                           | Severity  | Current Impact                                                 |
| --- | ------------------------------------------------------------------------------- | --------- | -------------------------------------------------------------- |
| 1   | 987 FLACs have `file_size=0` in `files` AND `file_locations.backup`             | 🟡 Medium | Can't verify backup integrity for 41% of FLAC backup records   |
| 2   | Reconcile step matches by **basename only** (not full relative path)            | 🟡 Medium | 2 FLAC collisions exist; WAV sources with common names at risk |
| 3   | Backup records are permanent — never re-verified (`clear_backup_status` unused) | 🟡 Medium | If NAS file is deleted/corrupted, DB still claims backed up    |
| 4   | No post-rsync size verification — records `file.file_size` blindly              | 🟡 Medium | Can't detect partial/corrupt transfers                         |

**Prune safety**: Confirmed robust. All 1,840 local+backed files are
backpack-protected. The prune query correctly requires both
`file_locations.local` AND `file_locations.backup` AND NOT in backpack.
A backup-only file can NEVER be pruned.

### Root Cause Analysis

#### Issue 1: `file_size=0` on 987 FLACs

The 987 FLACs were deleted from local disk (confirmed: Walker & Royce,
DJ Heartstring, Artbat all missing). Their `file_size=0` comes from the
backup task's reconcile/copy steps which blindly copy `file.file_size`
from the `files` table:

```rust
// src/tasks/mod.rs:1945 (reconcile) and :2103 (post-copy)
crate::db::record_backup_result(&db_clone, file.id, true, file.file_size, &remote_path)
```

If the scanner previously recorded `file_size=0` (due to a transient
error or a past code path that didn't set it), the backup record
inherits that 0 forever.

#### Issue 2: Basename-only reconcile

`list_remote_files()` strips paths to basenames (line 163-164 of
`src/backup/mod.rs`). The reconcile then matches local files to remote
files by basename alone:

```rust
// src/tasks/mod.rs:1929-1933
let filename = std::path::Path::new(&file.file_path).file_name()...;
if remote_set.contains(&filename) { /* mark as backed up */ }
```

For the FLACs folder (flat directory, depth=1), this is practically
safe because all FLAC basenames are unique within the folder. But it's
fragile — if a file has the same basename in different subdirectories
(e.g., `vocals.wav` under multiple stem source dirs), the reconcile
would match incorrectly.

#### Issue 3: Permanent backup records

`clear_backup_status()` exists in `src/db/storage.rs:510` but is
**never called outside of unit tests**. Once `file_locations.backup`
is inserted, it persists forever. The reconcile step only operates on
files WITHOUT backup records (via `get_unbacked_up_files()`), so
existing backup records are never re-checked.

#### Issue 4: No post-copy verification

After rsync completes, the backup task calls `record_backup_result`
with `file.file_size` (the LOCAL file size from DB). It never verifies
that the remote file actually has the expected size.

### Fix Plan

#### Fix 1: Use full relative paths for reconcile matching

**File**: `src/backup/mod.rs` — `list_remote_files()` and `list_remote_files_with_depth()`

Add a new method `list_remote_files_relative()` that returns paths relative
to the remote base (reuse `list_remote_files_full` which already does this).
Change the backup task's reconcile step to match by full relative path, not
just basename.

**File**: `src/tasks/mod.rs` — `start_backup_folder_task()` reconcile loop

```rust
// Before: basename match
let filename = std::path::Path::new(&file.file_path).file_name()...;
if remote_set.contains(&filename) { ... }

// After: relative path match
let rel_path = file.file_path.strip_prefix(&local_dir)...;
if remote_set.contains(rel_path) { ... }
```

#### Fix 2: Verify file size after rsync

**File**: `src/backup/mod.rs` — `BackupEngine`

Add a method `verify_remote_file(remote_path, expected_size) -> Result<bool>`
that SSHes to check `stat -c%s` or `ls -l` and compares with expected size.

**File**: `src/tasks/mod.rs` — post-copy recording loop

After rsync, before `record_backup_result`, optionally verify a sample of
files (every 50th file, or first + last) to detect transfer corruption.
Use the verified remote size for the backup record instead of the local
DB size:

```rust
// After rsync, for each file (or sampled):
let remote_size = engine.remote_file_size(&remote_path).await.unwrap_or(None);
let actual_size = remote_size.unwrap_or(file.file_size);
crate::db::record_backup_result(&db_clone, file.id, true, actual_size, &remote_path).await?;
```

#### Fix 3: Add periodic backup re-verification to Maintainer

**File**: `src/maintainer.rs`

Add a 4th check to the maintainer cycle (runs every 24h):

| #   | Check                | Condition                                               | Action                                                                             |
| --- | -------------------- | ------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| 4   | Stale backup records | `file_locations.backup` where `last_verified > 30 days` | Spawn `BackupVerify` task — samples records, `ssh stat` to verify they still exist |

New `TaskType::BackupVerify { folder_id }` — lightweight task that:

1. Queries backup records older than 30 days
2. Samples up to 100 records (stratified: oldest first)
3. For each: `ssh stat` on remote path → if file missing, logs warning + removes backup record
4. Does NOT re-copy anything — just verifies presence

**File**: `src/db/storage.rs` — new function

```rust
/// Verify a sample of backup records for a folder.
/// Returns (verified, missing, errors).
pub async fn verify_backup_records(
    pool: &Pool<Sqlite>,
    folder_id: i64,
    engine: &BackupEngine,
    sample_size: usize,
) -> Result<(usize, usize, usize)>
```

#### Fix 4: Backfill file_size for existing backup records

**File**: `src/db/storage.rs` — new function

```rust
/// For backup records with file_size=0, attempt to get the actual
/// remote file size via SSH and update the record.
/// Returns (checked, fixed, failed).
pub async fn backfill_backup_sizes(
    pool: &Pool<Sqlite>,
    engine: &BackupEngine,
) -> Result<(usize, usize, usize)>
```

**New API endpoint**: `POST /api/storage/backfill-backup-sizes`

Triggers a background task that, for each `file_locations.backup` record
with `file_size=0`, runs `ssh stat -c%s` on the remote path and updates
the record with the actual size. Reports results.

#### Fix 5: Ensure scanner always records file_size

**File**: `src/db/files.rs` — `extract_audio_metadata_from_file()`

Add a debug assertion or fallback: if `metadata.len()` returns 0, log a
warning. The current code looks correct (reads `metadata.len()` before
any cache/lofty logic), but add a test to prove it.

**New unit test**: `test_extract_audio_metadata_file_size_nonzero` —
creates a temp FLAC file, calls `extract_audio_metadata_from_file`,
asserts `file_size > 0`.

### Files to modify

| File                             | Change                                                                                                           |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `src/backup/mod.rs`              | Add `list_remote_files_relative()`, add `verify_remote_file()`                                                   |
| `src/tasks/mod.rs`               | Fix reconcile to use full relative paths; add post-copy size verification; add `BackupVerify` task type + worker |
| `src/db/storage.rs`              | Add `verify_backup_records()`, `backfill_backup_sizes()`; add unit tests for both                                |
| `src/db/files.rs`                | Add debug warning for `file_size=0`; add unit test                                                               |
| `src/maintainer.rs`              | Add periodic backup verification check (check #4)                                                                |
| `src/api/storage.rs`             | Add `POST /api/storage/backfill-backup-sizes` handler + route                                                    |
| `tests/api_storage.rs`           | Integration tests for backfill endpoint                                                                          |
| `frontend/pages/storage.js`      | Add "Backfill Backup Sizes" button + status display                                                              |
| `frontend/style.css`             | Minimal styles for backfill status                                                                               |
| `frontend/tests/storage.spec.js` | Playwright test for backfill button                                                                              |

### Acceptance Criteria

**Fix 1 — Full path reconcile:**

- [ ] `list_remote_files_relative()` returns paths relative to remote base (e.g., `Artist - Title.flac`)
- [ ] Reconcile matches by relative path, not basename
- [ ] Backward compat: flat directory (depth=1) produces same relative paths as before
- [ ] Unit test: `test_reconcile_matches_by_relative_path`

**Fix 2 — Post-copy verification:**

- [ ] `verify_remote_file()` SSHes to check file size, returns `true` when matching
- [ ] After rsync, first + last + every 50th file's remote size is verified
- [ ] Backup record uses verified remote size (not local DB size)
- [ ] Failed verification → logged as warning, record still created (non-fatal)
- [ ] Unit test: `test_verify_remote_file_size_match` and `_mismatch`

**Fix 3 — Periodic re-verification:**

- [ ] `BackupVerify` task type registered in `TaskType` enum + all match arms
- [ ] `verify_backup_records()` samples oldest 100 backup records, SSHes to check existence
- [ ] Missing remote files → backup record removed, warning logged
- [ ] Maintainer includes check #4 (runs every 24h)
- [ ] Unit test: `test_verify_backup_records_finds_missing`

**Fix 4 — Size backfill:**

- [ ] `backfill_backup_sizes()` queries records with `file_size=0`, gets remote size via SSH
- [ ] Updates `file_locations.file_size` and `last_verified`
- [ ] `POST /api/storage/backfill-backup-sizes` triggers background task, returns `{ taskId, zeroSizeRecords, fixed, failed }`
- [ ] Integration test for endpoint
- [ ] Frontend button on Storage page with progress display

**Fix 5 — Scanner guard:**

- [ ] `extract_audio_metadata_from_file` logs warning when `metadata.len() == 0`
- [ ] Unit test: `test_extract_audio_metadata_file_size_nonzero` (creates real temp FLAC)

**Validation:**

- [ ] `cargo build` passes
- [ ] `cargo test` passes (all existing + new tests)
- [ ] `cd frontend && npx playwright test` passes
- [ ] No regressions to backup/reconcile/rsync flow
- [ ] No regressions to prune safety (prune candidates remain 0)

### Agent Decomposition (TDD, 4 agents, zero file conflicts)

All agents write tests FIRST, then implement. Tests fail initially, then
go green as implementation is added.

| Agent | Files                                                                               | Work                                                                                                                                                            | Tests          |
| ----- | ----------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------- |
| **A** | `src/backup/mod.rs`, `src/tasks/mod.rs`                                             | Add `list_remote_files_relative()`, `verify_remote_file()`, fix reconcile path matching, add post-copy size verification, add `BackupVerify` task type + worker | ~8 unit        |
| **B** | `src/db/storage.rs`, `src/db/files.rs`, `src/maintainer.rs`                         | Add `verify_backup_records()`, `backfill_backup_sizes()`, scanner warning for file_size=0, maintainer check #4                                                  | ~8 unit        |
| **C** | `src/api/storage.rs`, `tests/api_storage.rs`                                        | Add `POST /api/storage/backfill-backup-sizes` handler + route + integration tests                                                                               | ~3 integration |
| **D** | `frontend/pages/storage.js`, `frontend/style.css`, `frontend/tests/storage.spec.js` | Add "Backfill Backup Sizes" button + Playwright test                                                                                                            | ~2 Playwright  |

**Write scope verification — zero overlap:**

- Agent A: `src/backup/mod.rs`, `src/tasks/mod.rs`
- Agent B: `src/db/storage.rs`, `src/db/files.rs`, `src/maintainer.rs`
- Agent C: `src/api/storage.rs`, `tests/api_storage.rs`
- Agent D: `frontend/pages/storage.js`, `frontend/style.css`, `frontend/tests/storage.spec.js`

All 4 agents can run in parallel.

### Per-Agent Task Briefs

Each agent must:

1. Read the relevant source files to understand existing patterns
2. Write tests FIRST (they will fail)
3. Implement the fix
4. Run `cargo test --lib` (A, B) or `cargo test --test api_storage` (C) or `npx playwright test -- tests/storage.spec.js` (D)
5. Run `cargo build` to verify compilation
6. Report back with test results

---

