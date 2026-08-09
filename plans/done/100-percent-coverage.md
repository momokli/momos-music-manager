## Plan: 100-percent-coverage

**Status**: done ✅
**Branch**: `feat/100-percent-coverage`
**Ready for review**: no
**Depends on**: `feat/query-performance-optimization` (current branch), `fix/scan-folder-task-tracking` (done ✅)
**Migration needed**: no

### Description

Achieve effective 100% code coverage: every API endpoint tested (happy + sad paths),
every query param covered, all pure business logic unit-tested, external-service
modules tested with error paths. The goal is **behavioral coverage** — every code
path exercised by a test — not necessarily 100% line coverage (external services
can't run in CI).

### Current State

| Metric                               | Value                                       |
| ------------------------------------ | ------------------------------------------- |
| Source lines                         | 31,505                                      |
| Routes (unique `.route()` calls)     | 118                                         |
| Total tests                          | 195 (59 unit + 134 integration + 2 doctest) |
| Modules with 0 unit tests            | 17 of 22                                    |
| Route handler methods                | ~130 (GET+POST+PUT+DELETE across 118 paths) |
| Routes completely untested           | ~38                                         |
| Routes partially tested (error-only) | ~4                                          |

**Source modules by line count and test status**:

| Module                   | Lines  | Unit Tests | Status                               |
| ------------------------ | ------ | ---------- | ------------------------------------ |
| `api.rs`                 | 10,688 | 0          | Integration tests cover handlers     |
| `db.rs`                  | 5,265  | 1          | Vastly undertested                   |
| `tasks/mod.rs`           | 3,208  | 0          | Integration (via task endpoints)     |
| `digging.rs`             | 2,150  | 0          | Integration (via digging endpoints)  |
| `spotify/sync_worker.rs` | 1,395  | 0          | External service (error paths only)  |
| `dump.rs`                | 1,288  | 0          | Integration (via dump endpoints)     |
| `comment.rs`             | 819    | 37         | ✅ Well covered                      |
| `traktor.rs`             | 605    | 8          | ✅ Covered                           |
| `poller.rs`              | 575    | 0          | External service (error paths only)  |
| `config.rs`              | 568    | 0          | Untested (pure Rust, fully testable) |
| `global_poller.rs`       | 532    | 0          | External service (error paths only)  |
| `main.rs`                | 457    | 0          | CLI parsing testable                 |
| `embeddings.rs`          | 429    | 6          | ✅ Covered                           |
| `backup/mod.rs`          | 410    | 0          | SSH-dependent (parse logic testable) |
| `deemix/client.rs`       | 373    | 0          | External service (error paths only)  |
| `spotify/client.rs`      | 344    | 0          | External service (error paths only)  |
| `audio_extensions.rs`    | 343    | 6          | ✅ Well covered                      |
| `launch_agent.rs`        | 311    | 0          | macOS-specific (excluded)            |
| `scan_cache.rs`          | 277    | 0          | Pure Rust (fully testable)           |
| `spotify/replay.rs`      | 265    | 0          | Pure Rust (fully testable)           |
| `deemix/cli.rs`          | 227    | 0          | CLI parsing testable                 |
| `maintainer.rs`          | 221    | 0          | Scheduling logic testable            |
| `watch.rs`               | 207    | 0          | File system (smoke only)             |
| `deemix/models.rs`       | 166    | 0          | Pure Rust (fully testable)           |
| `spotify/models.rs`      | ~100   | 0          | Pure Rust (fully testable)           |

### What "100%" means (realistic target)

| Category                 | Lines  | Target         | Strategy                                                     |
| ------------------------ | ------ | -------------- | ------------------------------------------------------------ |
| `api.rs` handlers        | 10,688 | 90% behavioral | Integration (happy+sad paths, all params, all filter combos) |
| `db.rs` logic            | 5,265  | 80% line       | Unit (in-memory SQLite) + integration                        |
| `tasks/mod.rs`           | 3,208  | 40% line       | Integration (via task endpoints + scan/write triggers)       |
| `digging.rs`             | 2,150  | 80% line       | Integration (endpoints) + unit (scoring, keys, dedup)        |
| `spotify/sync_worker.rs` | 1,395  | 20% line       | Integration (error paths only — no real Spotify in CI)       |
| `dump.rs`                | 1,288  | 60% line       | Integration (endpoints) + unit (serialization)               |
| `comment.rs`             | 819    | 100% ✅        | Already covered (37 tests)                                   |
| `traktor.rs`             | 605    | 90% ✅         | Already covered (8 tests), add error paths                   |
| `poller.rs`              | 575    | 15% line       | Integration (error paths only)                               |
| `config.rs`              | 568    | 90% line       | Unit (TOML parsing, env override, priority)                  |
| `global_poller.rs`       | 532    | 15% line       | Integration (error paths only)                               |
| `main.rs`                | 457    | 50% line       | Unit (CLI parsing, build_router structure)                   |
| `embeddings.rs`          | 429    | 90% ✅         | Already covered (6 tests), add edge cases                    |
| `backup/mod.rs`          | 410    | 30% line       | Unit (path construction, output parsing)                     |
| `deemix/client.rs`       | 373    | 15% line       | Integration (error paths only)                               |
| `spotify/client.rs`      | 344    | 15% line       | Integration (error paths only)                               |
| `audio_extensions.rs`    | 343    | 100% ✅        | Already covered (6 tests)                                    |
| `launch_agent.rs`        | 311    | 0%             | Excluded (macOS launchd — can't test in CI)                  |
| `scan_cache.rs`          | 277    | 70% line       | Unit (cache hits, invalidation, expiry)                      |
| `spotify/replay.rs`      | 265    | 80% line       | Unit (replay mode, cache save/load)                          |
| `deemix/cli.rs`          | 227    | 50% line       | Unit (CLI arg parsing)                                       |
| `maintainer.rs`          | 221    | 30% line       | Unit (scheduling logic, age checks)                          |
| `watch.rs`               | 207    | 15% line       | Smoke (start/stop, no real FS in CI)                         |
| `deemix/models.rs`       | 166    | 85% line       | Unit (serialization, status variants)                        |
| `spotify/models.rs`      | ~100   | 85% line       | Unit (key conversion, From impls)                            |

**Overall behavioral target**: ≥90% of reachable code paths exercised.
**Overall line coverage target**: ≥75% (measured via `cargo-llvm-cov`).

Rationale for <100% line coverage:

- External service code (Spotify API, deemix server, SSH/NAS) can't run in CI without real credentials
- System-level code (launchd, file watchers) is inherently integration-level
- `tasks/mod.rs` and `api.rs` are mostly I/O orchestration — covered by integration tests
- The goal is **effective** coverage: every behavior path exercised, not every line

---

### Phase 1: Prerequisite fixes — ~115 lines changed

Merge the two existing proposed fix plans from this document. These must be
done first because they fix bugs in the code under test and strengthen weak
assertions that would mask regressions.

#### 1a: Fix PMV filter data source (`fix/files-pmv-filter` plan)

The Files PMV filter reads `SUBSTR(files.comment, 2, 1)` — a comment-string
artifact — instead of `file_resolved_tags.prefix` (the actual tag category
data). Fix in 3 places: `get_files()`, `get_files_count()`,
`build_files_filter_sql()`. Replace with `EXISTS (SELECT 1 FROM
file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) IN (...))`.

**Files**: `src/api.rs` (~30 lines, 3 locations)

#### 1b: Strengthen assertions + fix 404 handlers (`harden-test-harness` plan)

- **12 weak assertions**: Replace `contains_key()` / field-presence checks with
  specific value assertions (e.g., `localFileCount` must equal seed value 2,
  `taskId` must be non-empty string)
- **5 handlers missing proper 404**: `delete_tag_handler`, `delete_folder_handler`,
  `update_folder_handler`, `folder_backup_config_handler`,
  `folder_auto_backup_handler` — query entity first, return 404 if `None`
- **1 wrong status code**: `digging_suggest_handler` returns 500 instead of 400
  for empty request body — change to `StatusCode::BAD_REQUEST`

**Files**: `src/api.rs` (~35 lines), `tests/api_*.rs` (~50 lines)

---

### Phase 2: Missing route coverage — ~80 integration tests, ~2,000 lines

Every unique endpoint gets at least one test. Read endpoints get full filter-param
coverage. Mutations get smoke tests. Error paths (400, 404) tested.

#### 2a: Files endpoints (add ~8 tests to `tests/api_files.rs`)

| Test                               | Endpoint                                | Coverage                      |
| ---------------------------------- | --------------------------------------- | ----------------------------- |
| `files_sync_comment`               | `POST /api/files/{id}/sync-comment`     | Write comment for single file |
| `files_similar_tracks`             | `GET /api/files/{id}/similar-tracks`    | Similar tracks by tag         |
| `files_debug_comment`              | `GET /api/files/{id}/debug-comment`     | Debug comment computation     |
| `files_needs_comment_count_by_ids` | `POST /api/files/needs-comment-count`   | By file IDs                   |
| `files_write_comments_by_ids`      | `POST /api/files/write-comments-by-ids` | By file IDs                   |
| `files_backup_status`              | `GET /api/files/{id}/backup-status`     | Backup status for file        |
| `files_pull_from_backup_error`     | `POST /api/files/{id}/pull-from-backup` | Error: no SSH config          |
| `files_needs_update_count`         | `GET /api/files/needs-update-count`     | Filter-based count            |

Note: `/api/files/bulk-sync` and `/api/files/write-comments` share a handler.
`/api/files/needs-comment-count-all` and `/api/files/write-comments-all` are
higher-risk (operate on all files) — smoke-test via the existing filter-based
endpoints instead.

#### 2b: Tracks endpoints (add ~9 tests to `tests/api_tracks.rs`)

| Test                           | Endpoint                               | Coverage                              |
| ------------------------------ | -------------------------------------- | ------------------------------------- |
| `tracks_write_comments`        | `POST /api/tracks/write-comments`      | Bulk write by track IDs               |
| `tracks_needs_refresh_count`   | `POST /api/tracks/needs-refresh-count` | Refresh count                         |
| `tracks_refresh_comments`      | `POST /api/tracks/refresh-comments`    | Refresh comments                      |
| `tracks_backpack_toggle`       | `POST /api/tracks/{id}/backpack`       | Add/remove from backpack              |
| `tracks_filter_pmv_categories` | `?pmvCategories=m,v`                   | PMV filter (uses track_resolved_tags) |
| `tracks_filter_pmv_aggregate`  | `?pmvAggregate=full`                   | PMV aggregate                         |
| `tracks_filter_file_types`     | `?fileTypes=flac`                      | File type filter                      |
| `tracks_filter_file_type_agg`  | `?fileTypeAgg=any`                     | File type aggregate                   |
| `tracks_filter_date_imported`  | `?importedAfterDays=365`               | Import date filter                    |
| `tracks_filter_date_added`     | `?addedAfterDays=365`                  | Added date filter                     |
| `tracks_filter_playlist_id`    | `?playlistId=1`                        | Single playlist param                 |

#### 2c: Tags endpoints (add ~8 tests to `tests/api_tags.rs`)

| Test                         | Endpoint                               | Coverage                   |
| ---------------------------- | -------------------------------------- | -------------------------- |
| `tags_from_playlists`        | `GET /api/tags/from-playlists`         | Playlists without tags     |
| `tags_create_from_playlists` | `POST /api/tags/create-from-playlists` | Create tags from playlists |
| `tags_service_coverage`      | `GET /api/tags/service-coverage`       | Service coverage stats     |
| `tags_parents_get`           | `GET /api/tags/{id}/parents`           | Get parent tags            |
| `tags_parents_set`           | `PUT /api/tags/{id}/parents`           | Set parent tags            |
| `tags_bulk_categorize`       | `POST /api/tags/bulk-categorize`       | Bulk category assignment   |
| `tags_bulk_import`           | `POST /api/tags/bulk-import`           | Bulk import                |
| `tags_bulk_resolve`          | `POST /api/tags/bulk-resolve`          | Bulk resolve               |

#### 2d: Playlists endpoints (add ~8 tests to `tests/api_playlists.rs`)

| Test                           | Endpoint                                   | Coverage                                |
| ------------------------------ | ------------------------------------------ | --------------------------------------- |
| `playlists_delete`             | `DELETE /api/playlists/{id}`               | Delete playlist + verify 404 on refetch |
| `playlists_tracks`             | `GET /api/playlists/{id}/tracks`           | List tracks in playlist                 |
| `playlists_add_track`          | `POST /api/playlists/{id}/tracks`          | Add track to playlist                   |
| `playlists_subscriptions_list` | `GET /api/playlists/subscriptions`         | List subscriptions                      |
| `playlists_subscribe`          | `POST /api/playlists/subscriptions`        | Subscribe to playlist                   |
| `playlists_unsubscribe`        | `DELETE /api/playlists/subscriptions/{id}` | Unsubscribe                             |
| `playlists_comment_diff_stats` | `GET /api/playlists/comment-diff-stats`    | Comment diff stats                      |
| `playlists_filter_stale`       | `?stale=1`                                 | Stale playlists filter                  |

#### 2e: Folders endpoints (add ~6 tests to `tests/api_folders.rs`)

| Test                    | Endpoint                              | Coverage                                           |
| ----------------------- | ------------------------------------- | -------------------------------------------------- |
| `folders_create`        | `POST /api/folders`                   | Create folder                                      |
| `folders_update`        | `PUT /api/folders/{id}`               | Update folder                                      |
| `folders_scan`          | `POST /api/folders/{id}/scan`         | Trigger scan (path doesn't exist → task registers) |
| `folders_backup_config` | `PUT /api/folders/{id}/backup`        | Set backup config                                  |
| `folders_auto_backup`   | `PUT /api/folders/{id}/auto-backup`   | Toggle auto-backup                                 |
| `folders_scan_sources`  | `POST /api/folders/{id}/scan-sources` | Scan WAV sources                                   |

#### 2f: Storage endpoints (add ~5 tests to `tests/api_storage.rs`)

| Test                             | Endpoint                                 | Coverage                  |
| -------------------------------- | ---------------------------------------- | ------------------------- |
| `storage_settings_get`           | `GET /api/storage/settings`              | Get settings              |
| `storage_settings_put`           | `PUT /api/storage/settings`              | Update settings           |
| `storage_backup_no_ssh`          | `POST /api/storage/backup/{id}`          | Error: SSH not configured |
| `storage_backup_wavs_no_ssh`     | `POST /api/storage/backup-wavs/{id}`     | Error: SSH not configured |
| `storage_discover_backup_no_ssh` | `POST /api/storage/discover-backup/{id}` | Error: SSH not configured |

#### 2g: Service endpoints (add ~6 tests, extend `tests/api_services.rs`)

| Test                    | Endpoint                                   | Coverage                              |
| ----------------------- | ------------------------------------------ | ------------------------------------- |
| `services_config_get`   | `GET /api/services/{service}/config`       | Get service config                    |
| `services_config_put`   | `PUT /api/services/{service}/config`       | Update service config                 |
| `services_fetch_counts` | `GET /api/services/{service}/fetch-counts` | Fetch counts                          |
| `services_sync_status`  | `GET /api/services/{service}/sync-status`  | Sync status                           |
| `services_reset`        | `POST /api/services/{service}/reset`       | Reset service (error: not configured) |
| `services_deemix_auth`  | `POST /api/services/deemix/auth`           | Deemix auth (error: not configured)   |

#### 2h: Deemix endpoints (new file `tests/api_deemix.rs`, ~4 tests)

| Test                      | Endpoint                                     | Coverage                        |
| ------------------------- | -------------------------------------------- | ------------------------------- |
| `deemix_queue_list`       | `GET /api/services/deemix/queue`             | Queue list (empty — no server)  |
| `deemix_queue_add_error`  | `POST /api/services/deemix/queue`            | Add to queue (error: no server) |
| `deemix_queue_retry_404`  | `POST /api/services/deemix/queue/{id}/retry` | Retry non-existent → 404        |
| `deemix_queue_delete_404` | `DELETE /api/services/deemix/queue/{id}`     | Delete non-existent → 404       |

#### 2i: Tag energy levels (new file `tests/api_tag_energy_levels.rs`, ~3 tests)

| Test                      | Endpoint                              | Coverage         |
| ------------------------- | ------------------------------------- | ---------------- |
| `tag_energy_levels_list`  | `GET /api/tag-energy-levels`          | List all         |
| `tag_energy_levels_set`   | `PUT /api/tag-energy-levels/{tag_id}` | Set energy level |
| `tag_energy_levels_batch` | `PUT /api/tag-energy-levels/batch`    | Batch reorder    |

#### 2j: Tag categories (new file `tests/api_tag_categories.rs`, ~3 tests)

| Test                    | Endpoint                          | Coverage                            |
| ----------------------- | --------------------------------- | ----------------------------------- |
| `tag_categories_list`   | `GET /api/tag-categories`         | List all (5 defaults + any created) |
| `tag_categories_create` | `POST /api/tag-categories`        | Create category                     |
| `tag_categories_delete` | `DELETE /api/tag-categories/{id}` | Delete created category             |

#### 2k: Spotify sync endpoints (new file `tests/api_spotify_sync.rs`, ~5 tests)

All return errors when Spotify isn't configured — test the error paths:

| Test                                 | Endpoint                                           | Coverage              |
| ------------------------------------ | -------------------------------------------------- | --------------------- |
| `spotify_sync_playlists_error`       | `POST /api/services/spotify/sync/playlists`        | Error: not configured |
| `spotify_sync_new_playlists_error`   | `POST /api/services/spotify/sync/new-playlists`    | Error                 |
| `spotify_sync_playlists_batch_error` | `POST /api/services/spotify/sync/playlists/batch`  | Error                 |
| `spotify_sync_tracks_error`          | `POST /api/services/spotify/sync/tracks`           | Error                 |
| `spotify_refresh_playlist_error`     | `POST /api/services/spotify/refresh-playlist/{id}` | Error                 |

#### 2l: Infrastructure endpoints (add ~4 tests to existing files or new)

| Test                      | Endpoint                           | Coverage               |
| ------------------------- | ---------------------------------- | ---------------------- |
| `version_check`           | `GET /api/version`                 | Returns version string |
| `tag_similarities_status` | `GET /api/tag-similarities/status` | Similarities status    |
| `traktor_status`          | `GET /api/traktor/status`          | Traktor import status  |
| `traktor_import_no_file`  | `POST /api/traktor/import`         | Error: no file         |

#### 2m: Embeddings endpoints (add ~3 tests to existing or new)

| Test                         | Endpoint                               | Coverage                 |
| ---------------------------- | -------------------------------------- | ------------------------ |
| `embeddings_status`          | `GET /api/embeddings/status`           | Status (no model loaded) |
| `embeddings_recompute`       | `POST /api/embeddings/recompute`       | Triggers recompute task  |
| `tag_similarities_recompute` | `POST /api/tag-similarities/recompute` | Triggers task            |

#### 2n: Digging endpoints (add ~2 tests to `tests/api_digging.rs`)

| Test                         | Endpoint                                         | Coverage           |
| ---------------------------- | ------------------------------------------------ | ------------------ |
| `digging_ladder_suggest`     | `POST /api/digging/ladder/suggest`               | Ladder suggestions |
| `digging_tracks_with_params` | `GET /api/digging/tracks?energyLevels=1&limit=3` | Filter params      |

#### 2o: Filter combination tests (~4 tests across files)

The frontend sends multiple filters simultaneously — test that critical
combinations work and count parity holds:

| Combo                                           | Endpoint      | Test file       |
| ----------------------------------------------- | ------------- | --------------- |
| `isLocal=true` + `commentStatuses=needs_update` | `/api/files`  | `api_files.rs`  |
| `backedUp=true` + `isLocal=false`               | `/api/files`  | `api_files.rs`  |
| `hasLocal=true` + `hasBackup=true`              | `/api/tracks` | `api_tracks.rs` |
| `pmvCategories=m,v` + `hasLocal=true`           | `/api/tracks` | `api_tracks.rs` |

---

### Phase 3: Unit tests for untested modules — ~130 unit tests, ~2,000 lines

Every untested pure-Rust module gets a `#[cfg(test)]` module. External-service
modules get tests for their pure logic (parsing, conversion, error handling).

#### 3a: `src/config.rs` (~15 tests)

Test config file parsing, env var override, priority ordering, defaults:

- `config_loads_from_toml` — Parse a valid temp config.toml
- `config_env_override` — Env var `SPOTIFY_CLIENT_ID` overrides TOML
- `config_defaults` — Missing optional values get defaults
- `config_priority_order` — Env > TOML > hardcoded default
- `config_spotify_configured` — `is_spotify_configured()` returns bool
- `config_soundcloud_configured` — Same for SoundCloud
- `config_youtube_configured` — Same for YouTube
- `config_invalid_toml` — Graceful error on malformed TOML
- `config_missing_file` — Graceful when config.toml doesn't exist
- `config_empty_sections` — Empty `[spotify]` doesn't crash
- `config_polling_section` — Parse `[polling]` section
- `config_maintainer_section` — Parse `[maintainer]` section
- `config_database_url_env` — `DATABASE_URL` env var
- `config_public_url` — `PUBLIC_URL` / `MOMOS_PUBLIC_URL` env var
- `config_secrets_not_in_debug` — Secrets excluded from Debug output

#### 3b: `src/db.rs` (~40 tests)

Test pure functions and in-memory SQLite operations:

- 5 tests: Camelot key parsing (`parse_camelot_key`, display, edge cases)
- 5 tests: Comment computation (`compute_target_comment`, with/without parents, empty)
- 4 tests: Tag queries (`get_tag_by_name` nocase, `tag_exists`, by category, by backpack)
- 4 tests: File tag resolution (`get_file_resolved_tags`, `refresh_file_resolved_tags`)
- 4 tests: Prune candidates (`get_prune_candidates` with various filters)
- 3 tests: Storage status (`get_storage_status` field accuracy)
- 4 tests: File variants (`get_file_variants` ISRC grouping, WAV source grouping)
- 3 tests: WAV→stem linking (`link_wav_to_stem` parsing, matching, edge cases)
- 3 tests: File locations CRUD (local/backup tracking)
- 3 tests: Folder CRUD (create, update, delete)
- 2 tests: Playlist subscription CRUD

#### 3c: `src/digging.rs` (~20 tests)

- 5 tests: Camelot key compatibility (`are_keys_compatible` — perfect, good, ok, incompatible, edge cases)
- 3 tests: Scoring (`score_breakdown` math, edge cases, ranking order)
- 3 tests: BPM outlier detection (median-based, edge cases, single seed)
- 3 tests: ISRC dedup with format preference (stem.m4a > flac > wav)
- 2 tests: Audio format preference ranking
- 4 tests: Full `get_multi_seed_suggestions` flow (by tag, by file IDs, empty seeds, no candidates)

#### 3d: `src/dump.rs` (~10 tests)

- 3 tests: Dump to JSON (empty DB, populated DB, all tables present)
- 3 tests: Restore from JSON (valid, invalid, preserves IDs)
- 2 tests: Roundtrip (dump → restore → dump produces identical output)
- 2 tests: Edge cases (large dataset, special characters in strings)

#### 3e: `src/spotify/models.rs` (~5 tests)

- 3 tests: `spotify_key_to_camelot` — all 24 key mappings (12 minor + 12 major)
- 2 tests: Conversion from rspotify types (PlaylistInfo, TrackInfo)

#### 3f: `src/spotify/replay.rs` (~8 tests)

- 3 tests: Replay mode (enabled check, cache hit, cache miss)
- 3 tests: Cache operations (save, load, invalidation/clear)
- 2 tests: File I/O (save to temp file, load back, corrupt file error)

#### 3g: `src/scan_cache.rs` (~8 tests)

- 2 tests: Cache hit/miss (same path+size+mtime → hit, changed → miss)
- 3 tests: Cache lifecycle (expiry/TTL, clear all, LRU eviction at max entries)
- 2 tests: Serialization (save to file, load from file)
- 1 test: Empty cache behavior

#### 3h: `src/main.rs` (~5 tests)

- 3 tests: CLI parsing (`serve`, `scan-file`, `dump`, `restore` subcommands)
- 1 test: `build_router()` returns router with expected top-level routes
- 1 test: Help text includes all subcommands

#### 3i: `src/maintainer.rs` (~5 tests)

- 2 tests: Scheduling logic (interval calculation, next run time)
- 2 tests: Condition checks (full_scan_needed when last_scanned old, not needed when recent)
- 1 test: Auto-backup eligibility check

#### 3j: `src/backup/mod.rs` (~8 tests)

- 3 tests: Remote path construction (local→remote mapping)
- 2 tests: Output parsing (`ls -l` size extraction, `find` listing)
- 2 tests: Backup engine creation (with/without SSH config)
- 1 test: Dry-run output parsing

#### 3k: `src/deemix/models.rs` + `src/deemix/cli.rs` (~5 tests)

- 2 tests: Model deserialization (queue status JSON → struct)
- 2 tests: CLI argument parsing (subcommands)
- 1 test: Download status enum variants

---

### Phase 4: External service error paths — covered by Phase 2

For modules that require real external services (Spotify API, deemix server,
SSH/NAS), test only the "service not available" error paths. These are covered
by Phase 2 integration tests:

- **Spotify sync**: 5 error-path tests (Phase 2k) — `POST` returns error when not configured
- **Deemix queue**: 4 error-path tests (Phase 2h) — `POST`/`DELETE` returns error when no server
- **Backup/SSH**: 3 error-path tests (Phase 2f) — `POST` returns error when no SSH config
- **Service auth/config**: 6 tests (Phase 2g) — `GET`/`PUT`/`POST` on config endpoints

No additional work beyond Phase 2.

---

### Phase 5: Coverage measurement + iterative gap filling

#### 5a: Set up coverage tooling

```bash
# Install cargo-llvm-cov (requires nightly or Rust 1.74+)
cargo install cargo-llvm-cov

# Generate HTML coverage report
cargo llvm-cov --html --ignore-filename-regex 'tests/'

# Or with tarpaulin (works on stable)
cargo install cargo-tarpaulin
cargo tarpaulin --out Html --output-dir coverage --ignore-tests
```

#### 5b: Iterative gap filling process

1. Run coverage → generate HTML report
2. Sort modules by uncovered-line count descending
3. For each module with >20 uncovered lines, add targeted tests
4. Re-run coverage → verify improvement
5. Repeat until ≥75% line coverage

#### 5c: Coverage report as CI artifact

Add to release checklist (AGENT.md Section 1 Release Process):

- Step 7.5: Run `cargo llvm-cov --fail-under-lines 75` to verify coverage threshold

---

### Phase 6: Documentation

#### 6a: Add `tests/README.md`

Document:

- How to run tests: `cargo test`, `cargo test --test api_files`
- Test structure: unit (in `src/` via `#[cfg(test)]`), integration (in `tests/`)
- Seed data conventions: all in `tests/common/mod.rs`, explicit IDs, `refresh_file_resolved_tags()` after seeding
- How to add a new endpoint test: template + pattern
- Coverage measurement commands
- Coverage target: ≥75% line

#### 6b: Update AGENT.md Section 1 Testing rules

Replace the existing Testing section (or add to it):

```markdown
### Testing

- **`cargo test` is the single source of truth.** Every API endpoint, every filter
  parameter, every query variation must have a corresponding integration test.
  Agents must never merge code that doesn't pass `cargo test`.
- **Every plan that adds or modifies an API endpoint or filter parameter MUST
  include "add/update integration test" as an acceptance criterion.** Tests are
  not optional — they are part of the feature contract.
- **Coverage threshold**: ≥75% line coverage (via `cargo llvm-cov`). Run
  `cargo llvm-cov --fail-under-lines 75` before release.
- **Unit tests** go in `#[cfg(test)] mod tests` within the source file for
  pure functions. Integration tests go in `tests/api_*.rs` files.
- **Integration tests use a self-contained SQLite DB.** No external server, no
  real data. Each test creates a fresh in-memory DB, runs all migrations, seeds
  hand-crafted data that exercises edge cases, then hits the API and asserts
  exact results (row counts, field values, response shapes).
- **Test files mirror API structure.** `tests/api_files.rs` tests `/api/files*`,
  `tests/api_tracks.rs` tests `/api/tracks*`, etc.
- **Migration integrity is tested.** A dedicated test creates a fresh DB and
  runs all migrations end-to-end.
```

---

### Files to create

- `tests/api_deemix.rs` — ~4 tests (deemix queue endpoints)
- `tests/api_spotify_sync.rs` — ~5 tests (Spotify sync endpoints, error paths)
- `tests/api_tag_categories.rs` — ~3 tests
- `tests/api_tag_energy_levels.rs` — ~3 tests
- `tests/README.md` — documentation

### Files to modify

- `src/api.rs` — Phase 1 fixes (~65 lines)
- `src/config.rs` — add `#[cfg(test)]` module (~15 tests, ~200 lines)
- `src/db.rs` — add `#[cfg(test)]` module (~40 tests, ~600 lines)
- `src/digging.rs` — add `#[cfg(test)]` module (~20 tests, ~300 lines)
- `src/dump.rs` — add `#[cfg(test)]` module (~10 tests, ~150 lines)
- `src/spotify/models.rs` — add `#[cfg(test)]` module (~5 tests, ~80 lines)
- `src/spotify/replay.rs` — add `#[cfg(test)]` module (~8 tests, ~120 lines)
- `src/scan_cache.rs` — add `#[cfg(test)]` module (~8 tests, ~120 lines)
- `src/main.rs` — add `#[cfg(test)]` module (~5 tests, ~80 lines)
- `src/maintainer.rs` — add `#[cfg(test)]` module (~5 tests, ~80 lines)
- `src/backup/mod.rs` — add `#[cfg(test)]` module (~8 tests, ~120 lines)
- `src/deemix/models.rs` — add `#[cfg(test)]` module (~3 tests, ~50 lines)
- `src/deemix/cli.rs` — add `#[cfg(test)]` module (~2 tests, ~30 lines)
- `tests/common/mod.rs` — add seed helpers for new scenarios (~200 lines)
- `tests/api_files.rs` — add ~8 tests (~250 lines)
- `tests/api_tracks.rs` — add ~11 tests (~300 lines)
- `tests/api_tags.rs` — add ~8 tests (~200 lines)
- `tests/api_playlists.rs` — add ~8 tests (~200 lines)
- `tests/api_folders.rs` — add ~6 tests (~150 lines)
- `tests/api_storage.rs` — add ~5 tests (~120 lines)
- `tests/api_services.rs` — add ~6 tests (~150 lines)
- `tests/api_digging.rs` — add ~2 tests (~60 lines)
- `tests/api_tasks.rs` — add ~2 tests (~50 lines)
- `AGENT.md` — update Section 1 Testing rules, update "Last Updated"

### Acceptance Criteria

**Phase 1 (prerequisites):**

- [ ] Files PMV filter uses `file_resolved_tags.prefix`, not `SUBSTR(comment)`
- [ ] 5 handlers return proper 404 for non-existent entities (tag, folder, config)
- [ ] `digging_suggest_handler` returns 400 for empty request (not 500)
- [ ] 12 weak assertions strengthened to verify specific values
- [ ] All 195 existing tests still pass
- [ ] `cargo build` passes

**Phase 2 (route coverage):**

- [ ] Every unique API route has at least one integration test (happy or sad path)
- [ ] Every query param on FilesQuery, TracksQuery, PlaylistsQuery, TagsQuery has a test
- [ ] 400/404 error paths tested for all CRUD endpoints (create, read, update, delete)
- [ ] 4 critical filter combinations tested with count parity
- [ ] 5 new test files created: `api_deemix.rs`, `api_spotify_sync.rs`, `api_tag_categories.rs`, `api_tag_energy_levels.rs`, `tests/README.md`
- [ ] All 13 existing test files extended with missing endpoint/param coverage
- [ ] Total integration tests: ~215 (134 existing + ~80 new)

**Phase 3 (unit tests):**

- [ ] `config.rs`: 15 unit tests (TOML parsing, env override, priority, defaults)
- [ ] `db.rs`: 40 unit tests (camelot keys, comment computation, tag queries, file resolution, prune candidates, storage status, file variants, WAV linking, CRUD)
- [ ] `digging.rs`: 20 unit tests (camelot compatibility, scoring, BPM outliers, ISRC dedup, full flow)
- [ ] `dump.rs`: 10 unit tests (serialization, deserialization, roundtrip, edge cases)
- [ ] `spotify/models.rs`: 5 unit tests (key conversion, type conversions)
- [ ] `spotify/replay.rs`: 8 unit tests (replay mode, cache operations, file I/O)
- [ ] `scan_cache.rs`: 8 unit tests (hit/miss, TTL, LRU, serialization)
- [ ] `main.rs`: 5 unit tests (CLI parsing, build_router structure)
- [ ] `maintainer.rs`: 5 unit tests (scheduling, condition checks)
- [ ] `backup/mod.rs`: 8 unit tests (path construction, output parsing)
- [ ] `deemix/models.rs` + `deemix/cli.rs`: 5 unit tests (deserialization, CLI parsing)
- [ ] Total unit tests: ~190 (59 existing + ~130 new)

---

