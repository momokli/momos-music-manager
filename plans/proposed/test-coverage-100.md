## Plan: test-coverage-100

**Status**: proposed
**Branch**: `feat/test-coverage-100`
**Ready for review**: no
**Depends on**: `feat/integration-test-harness`
**Migration needed**: no

### Description

Achieve ~100% backend route coverage. Currently 17/59 routes tested (29%).
The goal is to test every route that doesn't require external services
(Spotify OAuth, SSH/NAS, WebSocket, ML models). Mutations (POST/PUT)
get basic smoke tests; read endpoints get filter-param coverage.

### Current coverage

| Domain         | Tested         | Untested                                                                                      |
| -------------- | -------------- | --------------------------------------------------------------------------------------------- |
| Files          | 7 of 12 routes | `latest`, `service-links`, `{id}/detail`, `{id}/write-comment`, `key-comparison`              |
| Tracks         | 4 of 7 routes  | `{id}`, `needs-comment-count`, `write-comments`                                               |
| Tags           | 3 of 8 routes  | `POST` (create), `{id}`, `curation-queue`, `unreviewed`, `categorize`                         |
| Playlists      | 1 of 4 routes  | `local` (POST), `{id}/archive`, `{id}`                                                        |
| Storage        | 2 of 5 routes  | `prune`, `backup/{id}`, `discover-backup/{id}`                                                |
| Folders        | 0 of 5 routes  | All 5                                                                                         |
| Tasks          | 0 of 2 routes  | Both                                                                                          |
| Digging        | 1 of 3 routes  | `search`, `tracks`                                                                            |
| Infrastructure | 1 of 8 routes  | `health`, `dump`, `restore`, `tag-energy-levels`×4, `tags/{id}/children`, `tags/{id}/suggest` |

**Untested filter params on already-tested endpoints:**

| Endpoint             | Missing params                                                                                                                 |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `GET /api/files`     | `commentStatuses`, `linkedOnly`, `nonDefaultOnly`, `keys`, `safeToDelete`, `pmvCategories`, `pmvAggregate`, `bpmMin`, `bpmMax` |
| `GET /api/tracks`    | `hasLocal`, `hasBackup`                                                                                                        |
| `GET /api/playlists` | `archive`, `categories`, `subscribed`, `stale`                                                                                 |

### Categorization of untested routes

**Tier A — Fully testable with seed data alone (37 routes):**
All CRUD reads, writes, and infrastructure endpoints that work against
SQLite with seeded data. No external services needed.

**Tier B — Partially testable (2 routes):**
`/api/services/{service}/sync` and `/api/services/{service}/reset` —
can test the "not configured" error response.

**Tier C — Not testable in CI (7 routes):**
OAuth (`/api/services/{service}/auth`, `/callback`), WebSocket (`/ws/spotify`),
SSH (`/api/backup/test`, `/api/backup/explore`), ML (`/api/embeddings/*`),
Traktor (`/api/traktor/import` — needs `.nml` file on disk, actually testable
if we write an inline NML string to a temp file before calling).

### Exclusions (routes we deliberately skip)

| Route                          | Reason                                                       |
| ------------------------------ | ------------------------------------------------------------ |
| `/api/services/{service}/auth` | OAuth redirect — can't test without real Spotify credentials |
| `/callback`                    | OAuth callback — same                                        |
| `/ws/spotify`                  | WebSocket — requires real-time auth token                    |
| `/api/backup/test`             | SSH connection test — needs NAS                              |
| `/api/backup/explore`          | SSH file listing — needs NAS                                 |
| `/api/embeddings/status`       | Requires ML model download (BERT, ~500MB)                    |
| `/api/embeddings/reset-review` | Same                                                         |

### Phase 1: Missing filter params (highest ROI — fills existing test files)

**File**: `tests/api_files.rs` — add ~8 tests

| Test                                         | What it proves                                                  |
| -------------------------------------------- | --------------------------------------------------------------- |
| `files_filter_comment_statuses_needs_update` | `?commentStatuses=needs_update` filters correctly               |
| `files_filter_comment_statuses_up_to_date`   | `?commentStatuses=up_to_date` filters correctly                 |
| `files_filter_linked_only`                   | `?linkedOnly=true` returns only files with service links        |
| `files_filter_unlinked`                      | `?unlinked=true` returns only files without service links       |
| `files_filter_non_default_only`              | `?nonDefaultOnly=true` returns only files with non-Setlist tags |
| `files_filter_key`                           | `?key=4m` returns files matching that Camelot key               |
| `files_filter_safe_to_delete`                | `?safeToDelete=true` filters correctly                          |
| `files_filter_pmv_categories`                | `?pmvCategories=p,m` returns files with Phase/Mood tags         |
| `files_filter_pmv_aggregate`                 | `?pmvAggregate=full` returns files with all 3 PMV tags          |

**File**: `tests/api_tracks.rs` — add ~3 tests

| Test                       | What it proves                                           |
| -------------------------- | -------------------------------------------------------- |
| `tracks_filter_has_local`  | `?hasLocal=true` filters to tracks with local files      |
| `tracks_filter_has_backup` | `?hasBackup=true` filters to tracks with backed-up files |
| `tracks_single_by_id`      | `GET /api/tracks/1` returns single track                 |

**File**: `tests/api_playlists.rs` — add ~4 tests

| Test                                | What it proves                                       |
| ----------------------------------- | ---------------------------------------------------- |
| `playlists_filter_archive_archived` | `?archive=archived` returns only archived playlists  |
| `playlists_filter_archive_active`   | `?archive=active` returns only active playlists      |
| `playlists_filter_subscribed`       | `?subscribed=true` returns only subscribed playlists |
| `playlists_filter_categories`       | `?categories=1,2` filters by tag category IDs        |

### Phase 2: Read-only endpoints (existing domains, new tests)

#### `tests/api_folders.rs` — NEW FILE

| Test                   | What it proves                                        |
| ---------------------- | ----------------------------------------------------- |
| `folders_list`         | `GET /api/folders` returns all seeded folders         |
| `folders_count`        | `GET /api/folders/count` matches list length          |
| `folders_single`       | `GET /api/folders/{id}/stats` returns folder metadata |
| `folders_toggle_watch` | `POST /api/folders/{id}/watch` toggles active flag    |
| `folders_not_found`    | `GET /api/folders/9999/stats` returns 404             |

#### `tests/api_tasks.rs` — NEW FILE

| Test                       | What it proves                                     |
| -------------------------- | -------------------------------------------------- |
| `tasks_list_empty`         | `GET /api/tasks` returns empty array on fresh DB   |
| `tasks_list_with_task`     | After triggering a scan, returns tasks with status |
| `tasks_single_not_found`   | `GET /api/tasks/xxx-xxx` returns 404               |
| `tasks_list_status_filter` | `?status=completed` filters correctly              |

#### Extend existing files

| File                     | Add                                                                                       |
| ------------------------ | ----------------------------------------------------------------------------------------- |
| `tests/api_files.rs`     | `files_latest` — `GET /api/files/latest` returns most recent                              |
| `tests/api_files.rs`     | `files_service_links` — `GET /api/files/service-links` returns Spotify/SC links           |
| `tests/api_files.rs`     | `files_detail` — `GET /api/files/1/detail` returns full metadata                          |
| `tests/api_files.rs`     | `files_key_comparison` — `GET /api/files/key-comparison?tag=Groovy` returns BPM/key table |
| `tests/api_tags.rs`      | `tags_single_by_id` — `GET /api/tags/7` returns tag with category info                    |
| `tests/api_tags.rs`      | `tags_curation_queue` — `GET /api/tags/curation-queue` returns Setlist tags               |
| `tests/api_tags.rs`      | `tags_unreviewed` — `GET /api/tags/unreviewed` returns tags without parents               |
| `tests/api_playlists.rs` | `playlists_single` — `GET /api/playlists/1` returns playlist detail                       |
| `tests/api_digging.rs`   | `digging_search` — `GET /api/digging/search?q=X` returns results                          |
| `tests/api_digging.rs`   | `digging_tracks` — `GET /api/digging/tracks?limit=5` returns paginated                    |

### Phase 3: Mutation endpoints

| File                     | Test                                                                                                                                                                                                                                              |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/api_files.rs`     | `files_write_comment` — `POST /api/files/1/write-comment` queues task, returns taskId                                                                                                                                                             |
| `tests/api_files.rs`     | `files_write_comments_bulk` — `POST /api/files/write-comments-by-ids` with body `{"fileIds": [1,2,3]}` queues bulk task (NOT `/api/files/write-comments` -- that endpoint takes filter params `{linkedOnly, tags, nonDefaultOnly}`, not file IDs) |
| `tests/api_tracks.rs`    | `tracks_needs_comment_count` — `POST /api/tracks/needs-comment-count` with `{"trackIds": [1]}`                                                                                                                                                    |
| `tests/api_tags.rs`      | `tags_create` — `POST /api/tags` with `{"name":"NewTag","categoryId":3}` returns created tag                                                                                                                                                      |
| `tests/api_tags.rs`      | `tags_categorize` — `PUT /api/tags/7/categorize` with `{"categoryId":4}` moves to Vibe                                                                                                                                                            |
| `tests/api_playlists.rs` | `playlists_create_local` — `POST /api/playlists/local` creates local playlist                                                                                                                                                                     |
| `tests/api_playlists.rs` | `playlists_toggle_archive` — `PUT /api/playlists/1/archive` toggles `archiveDeleted`                                                                                                                                                              |
| `tests/api_folders.rs`   | `folders_scan` — `POST /api/folders/1/scan` triggers scan task (note: folder path `/test/stems` doesn't exist on disk, so `GET /api/tasks/{id}` should show failed status)                                                                        |
| `tests/api_storage.rs`   | `storage_prune` -- First calls prune-preview to get candidates, then `POST /api/storage/prune` with body `{"fileIds": [candidate_ids...]}` (NOT `?confirm=true` -- that query param does not exist on this endpoint)                              |

### Phase 4: Error states & infrastructure

| File                     | Test                                                                                 |
| ------------------------ | ------------------------------------------------------------------------------------ |
| `tests/api_files.rs`     | `files_not_found` — `GET /api/files/9999` returns 404                                |
| `tests/api_tracks.rs`    | `tracks_not_found` — `GET /api/tracks/9999` returns 404                              |
| `tests/api_tags.rs`      | `tags_not_found` — `GET /api/tags/9999` returns 404                                  |
| `tests/api_playlists.rs` | `playlists_not_found` — `GET /api/playlists/9999` returns 404                        |
| `tests/api_digging.rs`   | `digging_suggest_no_seeds` — `POST /api/digging/suggest` with empty body returns 400 |
| `tests/api_playlists.rs` | `playlists_create_local_no_name` — `POST /api/playlists/local` with `{}` returns 400 |
| `tests/api_tags.rs`      | `tags_create_no_name` — `POST /api/tags` with `{}` returns 400                       |

#### Infrastructure

| File                        | Test                                                                           |
| --------------------------- | ------------------------------------------------------------------------------ |
| `tests/api_health.rs` — NEW | `health_check` — `GET /api/health` returns `{"status": "ok"}`                  |
| `tests/api_dump.rs` — NEW   | `dump_download` — `GET /api/dump` returns JSON with Content-Disposition header |
| `tests/api_dump.rs` — NEW   | `restore_no_confirm` — `POST /api/restore` without `?confirm=true` returns 400 |
| `tests/api_tags.rs`         | `tags_children` — `GET /api/tags/7/children` returns child tags                |
| `tests/api_tags.rs`         | `tags_suggest` — `GET /api/tags/7/suggest` returns category suggestion         |

#### Service endpoints (error paths only)

| File                          | Test                                                                                                    |
| ----------------------------- | ------------------------------------------------------------------------------------------------------- |
| `tests/api_services.rs` — NEW | `services_sync_not_configured` — `POST /api/services/soundcloud/sync` returns error (SC not configured) |
| `tests/api_services.rs` — NEW | `services_list` — `GET /api/services` returns service status array                                      |

### Seed data requirements

#### Fixes to existing seed (critical -- blocks Phase 1)

1. **Add `spotify_id` to file rows in `seed_basic_data()`** -- files need
   `spotify_id = 'spotify:track:aaa'` (matching service_track 1's `service_id`)
   for `v_file_track_link` to resolve. Without this, `hasLocal`, `hasBackup`,
   `linkedOnly`, and `unlinked` filters silently return empty results.

2. **Add a 4th unlinked file** -- file id=4 with ISRC `US999` (no matching
   service_track) so `?unlinked=true` can be proven to return files.

#### New seed functions (blocks Phase 1-2 tests)

| Function                     | Needed for                                                                                         | What it does                                                                                    |
| ---------------------------- | -------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `seed_files_with_comments()` | `commentStatuses=needs_update`, `up_to_date`                                                       | Files with `comment` set, `file_resolved_tags` populated so computed target differs from stored |
| `seed_tag_hierarchy()`       | `curation-queue`, `unreviewed`, `nonDefaultOnly`, `pmvCategories`, `pmvAggregate`, `tags_children` | Setlist-category tags with parent/child relationships + playlist matching + file links          |
| `seed_subscribed_playlist()` | `playlists_filter_subscribed`, `archive`                                                           | One row in `playlist_subscriptions` + playlist with `archive_deleted=true`                      |

#### Refresh pattern (call after seeding for tag-filter tests)

Every test that filters by tags, PMV, or non-default must call:

```rust
momos_music_manager::db::refresh_file_resolved_tags(&pool).await.unwrap();
```

### Immediate wins -- tests writable NOW (no seed changes)

21 tests can be written against existing `seed_basic_data()`:

| File               | Tests                                                                                                    |
| ------------------ | -------------------------------------------------------------------------------------------------------- |
| `api_files.rs`     | `files_filter_key`, `safe_to_delete`, `latest`, `service_links`, `detail`, `key_comparison`, `not_found` |
| `api_tracks.rs`    | `has_local`, `has_backup` (need spotify_id fix first), `single_by_id`, `not_found`                       |
| `api_playlists.rs` | `archive_active`, `single`, `not_found`                                                                  |
| `api_tags.rs`      | `single_by_id`, `not_found`, `create`, `categorize`, `create_no_name`                                    |
| `api_folders.rs`   | All 5 (list, count, single, toggle_watch, not_found)                                                     |
| `api_tasks.rs`     | All 4 (list_empty, list_with_task, not_found, status_filter)                                             |
| `api_storage.rs`   | `prune`                                                                                                  |
| `api_health.rs`    | `health_check`                                                                                           |
| `api_dump.rs`      | `dump_download`, `restore_no_confirm`                                                                    |
| `api_services.rs`  | `services_list`, `sync_not_configured`                                                                   |

### Filter combinations the frontend uses (test these together)

| Page         | Critical combo                                                 | Why                       |
| ------------ | -------------------------------------------------------------- | ------------------------- |
| files.js     | `isLocal=true` + `commentStatuses=needs_update`                | Tri-state filters combine |
| files.js     | `backedUp=true` + `isLocal=false`                              | Backup-only files         |
| files.js     | `fileTypes=flac,stem.m4a` + `safeToDelete=true`                | Cross-filter              |
| tracks.js    | `hasLocal=true` + `hasBackup=true`                             | AND'd boolean flags       |
| tracks.js    | `pmvCategories=p,m,v` + `hasLocal=true` + `fileTypes=stem.m4a` | Three independent dims    |
| digging.js   | `energyLevels` + `keyList/keyRange` + `tags` + `pmvCategories` | All 4 toggles on          |
| playlists.js | `archive=archived` + `subscribed=true`                         | Subscribed + archived     |

### Seed data ID ranges (documented to prevent collisions)

| Entity            | ID range | Source                                   |
| ----------------- | -------- | ---------------------------------------- |
| Tags              | 1-6      | Migration 001 (phase tags)               |
| Tags              | 7-9      | `seed_basic_data`                        |
| Tags              | 10-19    | `seed_tag_hierarchy` (new)               |
| Tags              | 20+      | `seed_pmv_tags` or inline                |
| Files             | 1-3      | `seed_basic_data`                        |
| Files             | 4        | Unlinked file (add to `seed_basic_data`) |
| Files             | 10-13    | `seed_digging_data`                      |
| Files             | 20-24    | `seed_wav_variant_data`                  |
| Files             | 30+      | `seed_files_with_comments` (new)         |
| Service playlists | 1-2      | `seed_basic_data`                        |
| Service playlists | 3+       | `seed_subscribed_playlist` (new)         |

### What "100%" actually means

52 of 59 routes (88%) for process-unique endpoints. 7 deliberately excluded:

- 2 OAuth (`/api/services/{service}/auth`, `/callback`)
- 1 WebSocket (`/ws/spotify`)
- 2 SSH (`/api/backup/test`, `/api/backup/explore`)
- 2 ML (`/api/embeddings/*`)

This considers only unique URL paths. Some paths have multiple handlers
(GET+POST+PUT). Tests cover all handler methods on covered paths.

### Files to create

- `tests/api_folders.rs` — 5 tests
- `tests/api_tasks.rs` — 4 tests
- `tests/api_health.rs` — 1 test
- `tests/api_dump.rs` — 2 tests
- `tests/api_services.rs` — 2 tests

### Files to modify

- `tests/api_files.rs` — add ~14 tests (filters + read endpoints + mutations + error states)
- `tests/api_tracks.rs` — add ~6 tests (filters + single + mutation + error)
- `tests/api_tags.rs` — add ~10 tests (read + create + categorize + error + children + suggest)
- `tests/api_playlists.rs` — add ~8 tests (archive/subscribed filters + single + create local + toggle archive + error)
- `tests/api_storage.rs` — add ~1 test (prune)
- `tests/api_digging.rs` — add ~3 tests (search, tracks, no-seeds error)
- `tests/common/mod.rs` — add `seed_files_with_comments()`, `seed_subscribed_playlist()`, `seed_tag_hierarchy()` helpers

### Acceptance Criteria

- [ ] `cargo build` passes
- [ ] Existing 129 tests still pass (no regressions)
- [ ] New test files created: `api_folders.rs`, `api_tasks.rs`, `api_health.rs`, `api_dump.rs`, `api_services.rs`
- [ ] All FilesQuery params have at least 1 test (22 params), count parity test covers all
- [ ] All TracksQuery params have at least 1 test (21 params)
- [ ] All PlaylistsQuery params have at least 1 test (11 params)
- [ ] All TagsQuery params tested (already done ✅)
- [ ] All CurationQueueQuery, FoldersQuery, TasksQuery params have at least 1 test
- [ ] All mutation endpoints return valid responses (200 or appropriate error)
- [ ] All 404 error paths tested (files, tracks, tags, playlists)
- [ ] `cargo test` completes in <15 seconds (current: ~5s with 129 tests)
- [ ] Total test count: ~200+ (129 existing + ~75 new)

