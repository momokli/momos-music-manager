## Plan: coverage-post-modularization

**Status**: done ✅
**Branch**: `feat/coverage-round2`
**Ready for review**: no
**Depends on**: `main` (post-modularization at d59f477)
**Migration needed**: no

### Description

After the modularization of `api.rs` → `api/` (16 files) and `db.rs` → `db/` (8 files),
re-compute coverage and add targeted tests. The modularization made the coverage data
much more actionable — instead of two mega-files, we now see exactly which domain has gaps.

### Current State (post-modularization)

| Metric                             | Value                                                                                                               |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| Tests                              | 415 (190 lib + 10 bin + 215 integration), all passing                                                               |
| Line coverage                      | 45.86% (22,983 lines, 10,541 covered)                                                                               |
| External (untestable) lines        | ~5,000 (spotify/\*, deemix/client, backup, global_poller, poller, watch, launch_agent, api/websocket, api/explorer) |
| Reachable (testable) line coverage | ~55.6% (9,991 / 17,983)                                                                                             |

### Coverage by module (key gaps)

**api/ modules:**

| Module                  | Lines | Cover% | Priority                                        |
| ----------------------- | ----- | ------ | ----------------------------------------------- |
| `api/files.rs`          | 2,660 | 60.58% | 🟡 +300 more lines reachable via edge cases     |
| `api/tracks.rs`         | 2,067 | 83.44% | ✅ Near max                                     |
| `api/tags.rs`           | 1,476 | 66.98% | 🟡 +150 more                                    |
| `api/playlists.rs`      | 882   | 73.76% | ✅ Good                                         |
| `api/folders.rs`        | 639   | 73.44% | ✅ Good                                         |
| `api/storage.rs`        | 371   | 44.70% | 🟡 +80 more                                     |
| `api/services.rs`       | 676   | 25.30% | 🔴 +200 more (config endpoints deeply untested) |
| `api/infrastructure.rs` | 364   | 59.30% | 🟡 +50 more                                     |
| `api/deemix_api.rs`     | 566   | 31.32% | 🔴 External dep (error paths max out ~40%)      |
| `api/explorer.rs`       | 218   | 10.94% | ⬛ External dep (SSH)                           |
| `api/websocket.rs`      | 181   | 2.92%  | ⬛ WebSocket (excluded)                         |
| `api/spotify_sync.rs`   | 405   | 22.22% | 🔴 External dep                                 |

**db/ modules:**

| Module            | Lines | Cover% | Priority                                                            |
| ----------------- | ----- | ------ | ------------------------------------------------------------------- |
| `db/files.rs`     | 1,750 | 31.00% | 🔴 **Biggest win** — mostly pure SQL builders, could add ~800 lines |
| `db/playlists.rs` | 567   | 25.21% | 🔴 +250 more via unit tests                                         |
| `db/tags.rs`      | 885   | 68.87% | ✅ Good                                                             |
| `db/folders.rs`   | 486   | 67.97% | ✅ Good                                                             |
| `db/tracks.rs`    | 245   | 56.52% | 🟡 +60 more                                                         |
| `db/storage.rs`   | 737   | 47.98% | 🔴 +200 more                                                        |
| `db/schema.rs`    | 192   | 53.00% | 🟡 +40 more                                                         |

**Other source modules:**

| Module          | Lines | Cover% | Priority                                              |
| --------------- | ----- | ------ | ----------------------------------------------------- |
| `tasks/mod.rs`  | 4,037 | 22.77% | 🔴 Big file, mostly I/O. Can add ~400 via integration |
| `digging.rs`    | 2,643 | 68.11% | 🟡 +200 via more unit tests                           |
| `dump.rs`       | 1,595 | 45.77% | 🔴 +300 via more roundtrip tests                      |
| `config.rs`     | 477   | 44.03% | 🔴 +150 via env-loading edge cases                    |
| `main.rs`       | 409   | 16.14% | 🔴 Hard to unit test (serve/startup). Can add ~80     |
| `maintainer.rs` | 163   | 34.97% | 🟡 +60 via more schedule tests                        |
| `scan_cache.rs` | 260   | 58.46% | 🟡 +40 via edge cases                                 |

**External (0-5% — can't improve without mocks):**
`spotify/*` (4 files), `global_poller.rs`, `poller.rs`, `watch.rs`,
`backup/mod.rs`, `deemix/client.rs`, `launch_agent.rs` — combined ~3,500 lines

### Target: 60% line coverage

Rationale: 75% is blocked by external services (~15% of codebase at near-0%).
60% is achievable by covering the reachable code (55.6% currently) up to ~72%.

72% of 17,983 reachable = 12,948 covered (currently 9,991 → need +2,957).
Plus external path error coverage: +200 lines → total 13,148 / 22,983 = 57.2%.

Wait — let's redo the math more carefully:

- Total lines: 22,983
- External (untestable): ~3,500 lines (0-2% coverage = ~100 covered)
- Reachable: ~19,483 lines (9,891 covered = 50.8%)

Target: 60% overall = 13,790 covered.
Currently: 10,541 covered. Need: +3,249.

To get +3,249 from reachable (19,483 lines): need 13,790 - 100 (external covered) = 13,690 from reachable.
That means reachable needs to go from 9,891 to 13,690 = +3,799 more covered reachable lines.
Reachable coverage target: 13,690 / 19,483 = 70.3%.

Hmm, that math double-counts. Let me restart:

- `reachable_covered` = total_covered - external_covered = 10,541 - ~100 = ~10,441
- `reachable_total` = total_lines - external_lines = 22,983 - 3,500 = 19,483
- `reachable_coverage` = 10,441 / 19,483 = 53.6%

To reach 60% overall: 60% \* 22,983 = 13,790 covered.
External can improve from ~100 to ~300 (more error path tests).
So reachable needs: 13,790 - 300 = 13,490.
Reachable needs +3,049 more covered lines.
Reachable target: 13,490 / 19,483 = 69.2%.

**Where the +3,049 comes from:**

| Source                                  | New tests | Est. lines gained |
| --------------------------------------- | --------- | ----------------- |
| `db/files.rs` unit tests                | ~30       | +800              |
| `db/playlists.rs` unit tests            | ~15       | +250              |
| `db/storage.rs` unit tests              | ~10       | +200              |
| `db/tracks.rs` unit tests               | ~5        | +60               |
| `db/schema.rs` unit tests               | ~3        | +40               |
| `api/services.rs` integration           | ~6        | +200              |
| `api/storage.rs` integration            | ~4        | +80               |
| `api/files.rs` integration (edge cases) | ~8        | +300              |
| `api/infrastructure.rs` integration     | ~3        | +50               |
| `api/tags.rs` integration               | ~4        | +150              |
| `digging.rs` unit tests                 | ~6        | +200              |
| `dump.rs` unit tests                    | ~8        | +300              |
| `config.rs` unit tests (env loading)    | ~6        | +150              |
| `tasks/mod.rs` integration (scan/write) | ~6        | +400              |
| `main.rs` unit tests                    | ~3        | +80               |
| `maintainer.rs` unit tests              | ~3        | +60               |
| `scan_cache.rs` unit tests              | ~3        | +40               |
| External error paths                    | ~8        | +200              |
| **Total**                               | **~125**  | **~3,560**        |

Phase 2 (external mocks) would push to 65-70%, but that's a separate plan.

### Phase 1: db/ unit tests — ~65 tests, +1,350 lines

#### 1a: `db/files.rs` — ~30 tests (~800 new covered lines)

This is the biggest gap (1,750 lines, 31%). Read the file first, then test pure functions:

- SQL builder functions that construct WHERE clauses for FilesQuery
- `build_files_filter_sql()` and variants
- File type detection helpers
- ISRC/comment/BPM processing functions
- `link_wav_to_stem()` logic
- File lifecycle queries

Each test creates an in-memory SQLite, creates the relevant tables, inserts seed rows,
calls the function, and asserts results.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        // Create minimal schema needed
        sqlx::query("CREATE TABLE IF NOT EXISTS files (id INTEGER PRIMARY KEY, ...)")
            .execute(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_some_db_function() {
        let pool = test_db().await;
        // seed, call, assert
    }
}
```

#### 1b: `db/playlists.rs` — ~15 tests (~250 lines)

Test subscription CRUD, playlist query builders, stale detection logic.

#### 1c: `db/storage.rs` — ~10 tests (~200 lines)

Test prune candidate queries, storage status computation, file location CRUD.

#### 1d: Other db modules — ~10 tests (~100 lines)

`db/tracks.rs` (~5), `db/schema.rs` (~3), remaining gaps (~2).

### Phase 2: Integration test additions — ~35 tests, +830 lines

#### 2a: `api/services.rs` — ~6 tests (+200 lines)

The services config endpoints are deeply untested (25%). Add:

- `services_config_get_spotify` — get config when not configured
- `services_config_put_spotify` — update with valid JSON
- `services_config_put_invalid` — 422 on invalid body
- `services_fetch_counts_spotify` — fetch counts when not configured (500 expected)
- `services_sync_status_spotify` — sync status when not configured
- `services_reset_spotify` — reset (may error or succeed)

#### 2b: `api/files.rs` — ~8 tests (+300 lines)

Test edge cases not yet covered:

- `files_filter_bpm_exact` — exact BPM match
- `files_filter_multiple_keys` — OR list of keys
- `files_sort_play_count` — sort by play count
- `files_filter_energy` — energy level filter
- `files_filter_safe_to_delete_false` — negative case
- `files_write_comment_task_succeeds` — exercise full write flow
- `files_bulk_sync_by_filter` — filter-based bulk sync
- `files_filter_comment_missing` — files with null comment

#### 2c: Other integration gaps — ~21 tests (+330 lines)

- `api/storage.rs`: +4 tests (settings edge cases, prune execute dry run) (~80 lines)
- `api/infrastructure.rs`: +3 tests (embeddings/reset, similarities/recompute) (~50 lines)
- `api/tags.rs`: +4 tests (energy level edge cases, bulk import edge cases) (~150 lines)
- `api/services.rs`: external error path tests for spotify/deemix edge cases (+6 tests, ~200 lines)

### Phase 3: Other source module unit tests — ~30 tests, +1,130 lines

#### 3a: `digging.rs` — ~6 tests (+200 lines)

Add edge cases for the multi-seed suggestion engine:

- `suggest_with_no_compatible_tracks` — empty suggestions
- `suggest_bpm_range_clamped_to_min` — min range handling
- `suggest_bpm_range_clamped_to_max` — max range handling
- `suggest_camelot_jumps_all_off` — all jumps disabled returns empty
- `suggest_score_breakdown_exact_weights` — verify score weights
- `suggest_ranked_by_scoring_criteria` — full ranking pipeline

#### 3b: `dump.rs` — ~8 tests (+300 lines)

More roundtrip edge cases:

- `dump_with_all_table_types` — verify every table present
- `dump_large_dataset_roundtrip` — 100+ records
- `dump_unicode_strings` — special characters in paths/names
- `restore_from_corrupt_json` — halfway through valid JSON
- `restore_partial_tables` — some tables missing
- `dump_restore_preserves_foreign_keys` — FK integrity
- `dump_compares_identical` — two dumps produce identical output
- `restore_idempotent` — restoring twice produces same state

#### 3c: `config.rs` — ~6 tests (+150 lines)

Env var loading edge cases (currently 44% coverage — the env loading paths are complex):

- `config_env_or_toml_port_invalid_number` — port that's not numeric
- `config_env_or_toml_port_out_of_range` — port >65535
- `config_mixed_env_and_toml_priority` — some env, some TOML
- `config_secrets_masked_in_log` — debug doesn't leak tokens
- `config_bool_env_var_false` — "false" env var correctly parsed
- `config_bool_env_var_true` — "true" env var correctly parsed

#### 3d: Other source modules — ~10 tests (+480 lines)

- `tasks/mod.rs`: +6 integration tests for task lifecycle (create, poll, complete, cancel, timeout, error) (+400 lines)
- `main.rs`: +3 more CLI tests (+80 lines)
- `maintainer.rs`: +3 schedule edge cases (+60 lines)
- `scan_cache.rs`: +3 cache edge cases (+40 lines)

### Phase 4: External service error path coverage — ~8 tests, +200 lines

Add more error path tests for external service endpoint handlers:

- `api/deemix_api.rs`: +2 tests (deemix retry validation, delete validation) (~50 lines)
- `api/spotify_sync.rs`: +3 tests (refresh error, full sync error, task cancel error) (~80 lines)
- `api/services.rs`: +3 tests already covered in Phase 2c (~70 lines)

### Files to modify

- `src/db/files.rs` — add `#[cfg(test)]` module (~30 tests, ~600 lines)
- `src/db/playlists.rs` — add `#[cfg(test)]` module (~15 tests, ~250 lines)
- `src/db/storage.rs` — add `#[cfg(test)]` module (~10 tests, ~150 lines)
- `src/db/tracks.rs` — add `#[cfg(test)]` module (~5 tests, ~80 lines)
- `src/db/schema.rs` — add `#[cfg(test)]` module (~3 tests, ~40 lines)
- `src/digging.rs` — extend `#[cfg(test)]` module (~6 tests, ~150 lines)
- `src/dump.rs` — extend `#[cfg(test)]` module (~8 tests, ~200 lines)
- `src/config.rs` — extend `#[cfg(test)]` module (~6 tests, ~120 lines)
- `src/main.rs` — extend `#[cfg(test)]` module (~3 tests, ~50 lines)
- `src/maintainer.rs` — extend `#[cfg(test)]` module (~3 tests, ~40 lines)
- `src/scan_cache.rs` — extend `#[cfg(test)]` module (~3 tests, ~40 lines)
- `tests/api_services.rs` — add ~8 tests (~250 lines)
- `tests/api_files.rs` — add ~8 tests (~250 lines)
- `tests/api_storage.rs` — add ~4 tests (~80 lines)
- `tests/api_infrastructure.rs` — add ~3 tests (~50 lines)
- `tests/api_tags.rs` — add ~4 tests (~120 lines)
- `tests/api_tasks.rs` — add ~6 tests (~150 lines)
- `tests/api_deemix.rs` — add ~2 tests (~50 lines)
- `tests/api_spotify_sync.rs` — add ~3 tests (~80 lines)
- `tests/common/mod.rs` — add seed helpers (~100 lines)
- `tests/README.md` — update coverage numbers
- `AGENT.md` — update "Last Updated"

### Acceptance Criteria

- [ ] `db/files.rs` coverage: 31% → 60%+ (+30 tests)
- [ ] `db/playlists.rs` coverage: 25% → 55%+ (+15 tests)
- [ ] `db/storage.rs` coverage: 48% → 65%+ (+10 tests)
- [ ] `api/services.rs` coverage: 25% → 45%+ (+8 tests)
- [ ] `digging.rs` coverage: 68% → 75%+ (+6 tests)
- [ ] `dump.rs` coverage: 46% → 60%+ (+8 tests)
- [ ] `config.rs` coverage: 44% → 60%+ (+6 tests)
- [ ] All other targeted modules gain ≥5pp coverage
- [ ] Total tests: ~540 (415 existing + ~125 new)
- [ ] Overall line coverage: 45.86% → ≥60%
- [ ] Reachable line coverage: 53.6% → ≥69%
- [ ] `cargo build` passes
- [ ] `cargo test` passes (all ~540 tests, <30s)
- [ ] No regressions to existing functionality

### Out of scope (requires external service mocking)

- `spotify/*` (4 files, ~2,300 lines) — needs trait-based test doubles
- `deemix/client.rs` (242 lines) — needs HTTP mock
- `backup/mod.rs` (388 lines) — needs SSH command mock
- `global_poller.rs` (314 lines) — needs Spotify client mock
- `poller.rs` (320 lines) — needs Spotify client mock
- `watch.rs` (107 lines) — needs filesystem fixture
- `launch_agent.rs` (203 lines) — macOS-specific, excluded

These 7 modules account for ~3,500 lines (15% of codebase). Mocking them would unlock
another 10-15pp coverage in a future plan.

---

### Agent Decomposition (all parallel, zero file conflicts)

The plan decomposes into 6 agents with **completely disjoint write scopes** — no
file is touched by more than one agent. All can run in parallel immediately.

| Agent | Files touched                                                                                                                | Work                                                                                         | Tests | Est. coverage gain |
| ----- | ---------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- | ----- | ------------------ |
| **A** | `src/db/files.rs`                                                                                                            | Unit tests for SQL builders, file helpers, WAV linking                                       | ~30   | +800 lines         |
| **B** | `src/db/playlists.rs`, `src/db/storage.rs`, `src/db/tracks.rs`, `src/db/schema.rs`                                           | Unit tests for subscription CRUD, prune queries, storage status, track queries               | ~35   | +550 lines         |
| **C** | `src/digging.rs`, `src/dump.rs`, `src/config.rs`                                                                             | Unit tests for scoring edge cases, dump roundtrip edge cases, env loading edge cases         | ~20   | +650 lines         |
| **D** | `src/main.rs`, `src/maintainer.rs`, `src/scan_cache.rs`                                                                      | Unit tests for CLI parsing, scheduling logic, cache edge cases                               | ~9    | +180 lines         |
| **E** | `tests/api_services.rs`, `tests/api_files.rs`, `tests/api_storage.rs`, `tests/common/mod.rs`                                 | Integration tests for service config, file edge cases, storage edge cases + ALL seed helpers | ~18   | +580 lines         |
| **F** | `tests/api_infrastructure.rs`, `tests/api_tags.rs`, `tests/api_tasks.rs`, `tests/api_deemix.rs`, `tests/api_spotify_sync.rs` | Integration tests for infra, tags, tasks, deemix, spotify sync                               | ~17   | +530 lines         |

**Write scope verification:**

- Agents A, B, C, D: all touch different `src/` files — zero overlap
- Agent E: touches `tests/api_services.rs`, `tests/api_files.rs`, `tests/api_storage.rs`, `tests/common/mod.rs` — none overlap with F
- Agent F: touches `tests/api_infrastructure.rs`, `tests/api_tags.rs`, `tests/api_tasks.rs`, `tests/api_deemix.rs`, `tests/api_spotify_sync.rs` — none overlap with E
- `tests/common/mod.rs` is only touched by Agent E, which adds ALL needed seed helpers

### Per-Agent Task Briefs

Each agent should:

1. Read the source files it's responsible for
2. Add tests following existing patterns in that file or sibling test files
3. Run `cargo test --lib` (for unit tests) or `cargo test --test FILENAME` (for integration) to verify
4. Run `cargo build` to check compilation
5. Report back with test counts, any failures, and coverage improvement estimates

Agent E additionally handles ALL seed helpers in `tests/common/mod.rs`.
Other integration agents (F) should use existing seed functions or inline seeding.

---

### Agent A: `db/files.rs` — ~30 unit tests, +800 lines

**File**: `src/db/files.rs` (1,750 lines, currently 31% coverage)

Read the file first. This module handles file queries, metadata extraction,
WAV source linking, and file lifecycle tracking. Focus on testable functions that don't require external state.

**Test targets (check what exists first, only add what's missing):**

1. SQL builder functions — construct WHERE clauses for FilesQuery, test various filter combinations
2. `build_files_filter_sql()` and variant filter builder functions
3. File type detection and classification helpers
4. BPM/comment/ISRC processing functions
5. `link_wav_to_stem()` — parse WAV filenames, extract stem_type, find parent stem
6. File lifecycle queries (create, update, delete, locations)

Use in-memory SQLite for DB-dependent tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;
    async fn test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        // Run migrations or create minimal schema
        pool
    }
}
```

**Verify**: `cargo test --lib -- db::files` must pass.

### Agent B: `db/playlists + storage + tracks + schema` — ~35 unit tests, +550 lines

**Files**:

- `src/db/playlists.rs` (567 lines, 25%)
- `src/db/storage.rs` (737 lines, 48%)
- `src/db/tracks.rs` (245 lines, 56%)
- `src/db/schema.rs` (192 lines, 53%)

Read all four files. Focus on testable functions:

**`db/playlists.rs`** (~15 tests):

- Subscription CRUD (subscribe, unsubscribe, list)
- Playlist query builders
- Stale detection logic (comparing local count vs remote_unique_count)
- Archive toggle queries

**`db/storage.rs`** (~10 tests):

- Prune candidate queries with filter combinations
- Storage status computation (local/backup/file counts)
- File location CRUD (insert local, insert backup, query, remove)
- Safe-to-delete logic

**`db/tracks.rs`** (~5 tests):

- Track query builders -`hasLocal`/`hasBackup` filter logic
- Playlist filtering logic

**`db/schema.rs`** (~3 tests):

- Schema introspection functions
- Migration verification helpers

**Verify**: `cargo test --lib -- db::playlists db::storage db::tracks db::schema` must pass.

### Agent C: `digging + dump + config` — ~20 unit tests, +650 lines

**Files**:

- `src/digging.rs` (2,643 lines, 68%)
- `src/dump.rs` (1,595 lines, 46%)
- `src/config.rs` (477 lines, 44%)

**`digging.rs`** (~6 tests):

- `suggest_with_no_compatible_tracks` — empty track set
- `suggest_bpm_range_clamped_to_min` — min range edge case
- `suggest_bpm_range_clamped_to_max` — max range edge case
- `suggest_camelot_jumps_all_off` — all jumps off → empty
- `suggest_score_breakdown_exact_weights` — verify score math
- `suggest_ranked_by_scoring_criteria` — verify ranking order

**`dump.rs`** (~8 tests):

- `dump_with_all_table_types` — every table present
- `dump_large_dataset_roundtrip` — 100+ records
- `dump_unicode_strings` — special chars in paths
- `restore_from_corrupt_json` — halfway-broken JSON
- `restore_partial_tables` — some tables missing
- `dump_restore_preserves_foreign_keys` — FK integrity
- `dump_compares_identical` — two dumps produce identical output
- `restore_idempotent` — restoring twice yields same state

**`config.rs`** (~6 tests):

- `config_env_or_toml_port_invalid_number` — non-numeric port
- `config_env_or_toml_port_out_of_range` — port >65535
- `config_mixed_env_and_toml_priority` — env + TOML mix
- `config_secrets_masked_in_log` — debug doesn't leak
- `config_bool_env_var_false` — "false" env var
- `config_bool_env_var_true` — "true" env var

**Verify**: `cargo test --lib -- digging dump config` must pass.

### Agent D: `main + maintainer + scan_cache` — ~9 unit tests, +180 lines

**Files**:

- `src/main.rs` (409 lines, 16%)
- `src/maintainer.rs` (163 lines, 35%)
- `src/scan_cache.rs` (260 lines, 58%)

**`main.rs`** (~3 tests):

- More CLI subcommand tests for edge cases
- `build_router()` structure test

**`maintainer.rs`** (~3 tests):

- Schedule edge cases (zero interval, very long interval)
- Condition check edge cases

**`scan_cache.rs`** (~3 tests):

- Cache edge cases (very large entries, concurrent access patterns)
- Serialization edge cases

**Verify**: `cargo test --lib -- main maintainer scan_cache` must pass.

### Agent E: Integration tests — services, files, storage + ALL seed helpers (~18 tests, +580 lines)

**Files**:

- `tests/api_services.rs`
- `tests/api_files.rs`
- `tests/api_storage.rs`
- `tests/common/mod.rs` (YOU handle ALL seed helpers needed by any integration test)

Read `tests/common/mod.rs` FIRST to understand seed patterns. Then:

**`tests/api_services.rs`** (~6 tests):

1. `services_config_get_spotify` — get config for unconfigured Spotify
2. `services_config_put_spotify` — update config with valid JSON body
3. `services_config_put_invalid` — malformed body → 422
4. `services_fetch_counts_spotify` — fetch counts for unconfigured
5. `services_sync_status_spotify` — sync status for unconfigured
6. `services_reset_spotify` — reset endpoint

**`tests/api_files.rs`** (~8 tests):

1. `files_filter_bpm_exact` — query with exact BPM value
2. `files_filter_multiple_keys` — OR list of Camelot keys
3. `files_sort_play_count` — sort by play_count field
4. `files_filter_energy` — energy level filter
5. `files_filter_safe_to_delete_false` — negative case
6. `files_write_comment_task_succeeds` — exercise full write comment flow
7. `files_bulk_sync_by_filter` — filter-based bulk sync (linked_only=true)
8. `files_filter_comment_missing` — filter files with null comment

**`tests/api_storage.rs`** (~4 tests):

1. `storage_settings_edge_cases` — test setting unusual values
2. `storage_prune_execute_dry_run` — test prune with empty file IDs
3. `storage_prune_execute_no_permission` — test error handling

**`tests/common/mod.rs`** — ADD ALL needed seed helpers:
Any new seed data needed by agents E or F. Check what `seed_basic_data()` provides, then add helpers for:

- Files with specific play counts, energy levels, null comments
- Service configs in various states

Existing test pattern:

```rust
let (client, base, pool) = common::spawn_test_app().await;
common::seed_basic_data(&pool).await;
// Add inline seeding as needed
```

Use inline seeding where possible to minimize seed helper surface area.

**Verify**: `cargo test --test api_services --test api_files --test api_storage` must pass.

### Agent F: Integration tests — infra, tags, tasks, deemix, spotify-sync (~17 tests, +530 lines)

**Files**:

- `tests/api_infrastructure.rs`
- `tests/api_tags.rs`
- `tests/api_tasks.rs`
- `tests/api_deemix.rs`
- `tests/api_spotify_sync.rs`

You do NOT modify `tests/common/mod.rs` — use existing seed functions or inline seeding.

**`tests/api_infrastructure.rs`** (~3 tests):

1. `embeddings_reset_review` — POST reset-review endpoint
2. `tag_similarities_recompute_again` — recompute with existing state
3. `version_endpoint_format` — verify version string format

**`tests/api_tags.rs`** (~4 tests):

1. `tag_energy_level_edge_cases` — set extreme energy levels (0, 10)
2. `tag_bulk_import_edge_cases` — empty import, duplicate names
3. `tag_bulk_categorize_multiple` — move multiple tags at once
4. `tag_curation_queue_pagination` — verify pagination on curation queue

**`tests/api_tasks.rs`** (~6 tests):

1. `tasks_cancel_running` — cancel a running task (scan)
2. `tasks_get_by_id` — fetch specific task by ID
3. `tasks_list_pagination` — paginated task list
4. `tasks_filter_by_type` — filter by task type (ScanFolder, WriteComment)
5. `tasks_single_not_found_strict_404` — verify 404 format
6. `tasks_multiple_concurrent` — trigger multiple tasks simultaneously

**`tests/api_deemix.rs`** (~2 tests):

1. `deemix_queue_retry_validation` — retry with invalid UUID format
2. `deemix_queue_delete_validation` — delete with no ID

**`tests/api_spotify_sync.rs`** (~3 tests):

1. `spotify_sync_task_cancel` — cancel a sync task
2. `spotify_refresh_playlist_not_found` — refresh non-existent playlist ID
3. `spotify_sync_full_error` — full sync without config

**Verify**:

```bash
cargo test --test api_infrastructure --test api_tags --test api_tasks --test api_deemix --test api_spotify_sync
```

---

### Agent Execution Order

All 6 agents can run **simultaneously** — no file overlaps. After all complete:

1. Run `cargo build` to verify compilation
2. Run `cargo test` to verify all tests pass
3. Run `cargo llvm-cov --html --ignore-filename-regex 'tests/'` to measure new coverage
4. Update `tests/README.md` with final numbers
5. Update AGENT.md plan status to done ✅

---

