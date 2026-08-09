## Plan: harden-test-harness

**Status**: done ✅
**Branch**: `fix/harden-test-harness`
**Ready for review**: no
**Depends on**: `fix/files-pmv-filter`
**Migration needed**: no

### Description

Harden the test harness based on three-audit findings: strengthen 12 weak
assertions, fix 6 handlers missing proper 404 responses, and fix 1 wrong
status code. This is the final polish pass — no new routes, just quality.

### Part A: Harden weak assertions (12 tests, ~50 lines)

| File             | Test                         | Current                                            | Fix                                                                                                                   |
| ---------------- | ---------------------------- | -------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `api_files.rs`   | `files_latest`               | Only checks `id` + `filePath` fields exist         | Assert files are ordered by `created_at` DESC (or at least 2 distinct files returned)                                 |
| `api_files.rs`   | `files_write_comment`        | Accepts `taskId` being null                        | Assert `taskId` is a non-empty string on success                                                                      |
| `api_files.rs`   | `files_key_comparison`       | Silent eprintln on 500, field-presence only on 200 | Assert `summary.matchCount` or `summary.totalCount` is present as a number                                            |
| `api_tracks.rs`  | `tracks_needs_comment_count` | Only checks field names exist                      | Assert `tracksNeedingUpdate` + `filesNeedingUpdate` are numbers                                                       |
| `api_storage.rs` | `storage_status_has_fields`  | 19 `contains_key` checks, no values                | After field-presence check, also verify `localFileCount` matches the value from `storage_status_counts` test seed (2) |
| `api_storage.rs` | `storage_prune_preview`      | Field-presence-only loop                           | Also assert `candidates.len() > 0` and first candidate has `fileSize > 0`                                             |
| `api_digging.rs` | `digging_tracks`             | Smoke test only                                    | Add one filter param (e.g., `?limit=3`) and verify returned count ≤ 3                                                 |
| `api_tasks.rs`   | `tasks_single_not_found`     | Accepts both 404 and 200                           | Assert strictly 404                                                                                                   |
| `api_tasks.rs`   | `tasks_list_status_filter`   | Lax comparison                                     | After scan task runs, verify `?status=running` or `?status=completed` returns non-empty                               |

### Part B: Fix missing 404 handlers (5 handlers, ~30 lines)

These handlers silently succeed or return 500 when the entity doesn't exist.
Each fix follows the same pattern: check existence first, then operate.

| Handler                        | File:Line      | Fix                                                           |
| ------------------------------ | -------------- | ------------------------------------------------------------- |
| `delete_tag_handler`           | `api.rs:3337`  | Query tag by ID first; return 404 if `None`, then `DELETE`    |
| `delete_folder_handler`        | `api.rs:6877`  | Query folder by ID first; return 404 if `None`, then `DELETE` |
| `update_folder_handler`        | `api.rs:6814`  | Query folder by ID first; return 404 if `None`, then `UPDATE` |
| `folder_backup_config_handler` | `api.rs:10401` | Query folder by ID first; return 404 if `None`, then update   |
| `folder_auto_backup_handler`   | `api.rs:10376` | Query folder by ID first; return 404 if `None`, then update   |

### Part C: Fix wrong status code (1 handler, ~2 lines)

| Handler                   | File:Line     | Fix                                                                                                   |
| ------------------------- | ------------- | ----------------------------------------------------------------------------------------------------- |
| `digging_suggest_handler` | `api.rs:2847` | Return 400 (StatusCode::BAD_REQUEST) instead of 500 when neither `seedTag` nor `seedFileIds` provided |

### Part D: Update tests for fixed handlers (4 tests, ~20 lines)

After fixing the 404 handlers, add/update tests:

| File             | Test                       | What                                                                   |
| ---------------- | -------------------------- | ---------------------------------------------------------------------- |
| `api_tags.rs`    | `tags_delete`              | `DELETE /api/tags/{new_id}` → 404 on valid ID, verify tag gone via GET |
| `api_folders.rs` | `folders_delete`           | `DELETE /api/folders/{new_id}` → 404 on valid ID                       |
| `api_folders.rs` | `folders_update_not_found` | `PUT /api/folders/9999` → 404                                          |
| `api_digging.rs` | `digging_suggest_no_seeds` | After fix, assert 400 instead of 500                                   |

### Files to modify

- `src/api.rs` — Part B (5 handlers) + Part C (1 handler)
- `tests/api_files.rs` — 3 weak tests
- `tests/api_tracks.rs` — 1 weak test
- `tests/api_storage.rs` — 2 weak tests
- `tests/api_digging.rs` — 2 tests (harden + fix status code)
- `tests/api_tasks.rs` — 2 weak tests
- `tests/api_tags.rs` — 1 new test (delete)
- `tests/api_folders.rs` — 2 new tests (delete + update 404)

### Acceptance Criteria

- [ ] All 12 weak assertions now verify specific values, not just field presence
- [ ] `delete_tag_handler` returns 404 for non-existent tag
- [ ] `delete_folder_handler` returns 404 for non-existent folder
- [ ] `update_folder_handler` returns 404 for non-existent folder
- [ ] `folder_backup_config_handler` returns 404 for non-existent folder
- [ ] `folder_auto_backup_handler` returns 404 for non-existent folder
- [ ] `digging_suggest_handler` returns 400 (not 500) for empty request
- [ ] All 192 existing tests still pass
- [ ] 4 new tests verify the fixed error paths
- [ ] `cargo build` passes
- [ ] Total test count: ~199 (195 + 4 new)

---

