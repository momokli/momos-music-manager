## Plan: harness-completeness-audit

**Status**: proposed
**Branch**: `feat/harness-completeness`
**Ready for review**: no
**Depends on**: `fix/scan-folder-task-tracking`
**Migration needed**: no

### Description

Comprehensive audit of the test harness for coverage gaps. Three parallel
reviews — route coverage, seed data adequacy, and frontend→backend param
completeness — revealed 36 untested routes, 2 placebo tests, 1 silent frontend
bug, and ~50 untested parameter/endpoint combinations.

### Current state

| Metric                             | Value    |
| ---------------------------------- | -------- |
| Total routes (unique URL + method) | 112      |
| Tested                             | 56 (50%) |
| Untested (testable)                | 36 (32%) |
| Partial (error only or needs more) | 4 (4%)   |
| Excluded (OAuth, SSH, ML, WS)      | 16 (14%) |
| Frontend params tested             | ~55      |
| Frontend params untested           | ~50      |

### 🔴 Critical: Placebo tests (assert 200, prove nothing)

**PMV filter tests in `api_files.rs` are dead.** `files_filter_pmv_categories`
and `files_filter_pmv_aggregate_full` both seed data where all files have
`comment = NULL`. The PMV filter operates on the `[PMV]` bracket in the
`comment` column, so every filter variant returns 0 rows — and the tests
assert 0, passing regardless of whether the filter SQL is correct, inverted,
or completely broken.

**Fix**: Add a file with `comment = "[PMV] groovy"` to `seed_files_with_comments`,
then assert:

- `?pmvCategories=p` returns that file
- `?pmvCategories=m,v` returns that file
- `?pmvAggregate=full` returns that file
- `?pmvAggregate=partial` returns that file

### 🔴 Critical: Frontend bug — `untagged` parameter is a silent placebo

**File**: `frontend/pages/playlists.js` sends `params.set("untagged", "true")`
but `PlaylistsQuery` has **no `untagged` field**. The parameter is silently
ignored by serde. The "Untagged" filter button on the Playlists page does
nothing.

**Fix**: Either add `untagged_only: Option<bool>` to `PlaylistsQuery` and
implement the SQL filter, or remove the dead button from the frontend.

### 🟡 Phase 1: Unblock placebo PMV tests (blocks Phase 2 PMV coverage)

**File**: `tests/common/mod.rs` — `seed_files_with_comments()`

Add a file row with `comment = "[PMV] groovy"`, link it to a service track
and the "Groovy" playlist, add backup file_locations, then update the two
PMV filter tests to assert positive results instead of 0.

**Tests to update**: `files_filter_pmv_categories`, `files_filter_pmv_aggregate_full`
(and add `_partial` and `_none` variants).

### 🟡 Phase 2: Missing filter params on existing endpoints

**`tests/api_tracks.rs`** — add ~9 tests:

| Test                                | Param                                                           |
| ----------------------------------- | --------------------------------------------------------------- |
| `tracks_filter_pmv_categories`      | `?pmvCategories=m` (needs seed with PMV comment on linked file) |
| `tracks_filter_pmv_aggregate_full`  | `?pmvAggregate=full`                                            |
| `tracks_filter_file_types`          | `?fileTypes=flac`                                               |
| `tracks_filter_file_type_agg_any`   | `?fileTypeAgg=any`                                              |
| `tracks_filter_file_type_agg_none`  | `?fileTypeAgg=none`                                             |
| `tracks_filter_imported_after_days` | `?importedAfterDays=365`                                        |
| `tracks_filter_added_after_days`    | `?addedAfterDays=365`                                           |
| `tracks_single_playlist_id`         | `?playlistId=1` (single playlist param)                         |

### 🟡 Phase 3: Missing POST mutation endpoints

**`tests/api_tracks.rs`**:

- `tracks_write_comments` — `POST /api/tracks/write-comments` with `{"trackIds": [1]}`, verify returns taskId
- `tracks_backpack_toggle` — `POST /api/tracks/1/backpack` with `{"add": true}`, verify via detail endpoint

**`tests/api_files.rs`**:

- `files_needs_comment_count` — `POST /api/files/needs-comment-count` with `{"fileIds": [1,2]}`
- `files_write_comments_by_ids` — `POST /api/files/write-comments-by-ids` with `{"fileIds": [1]}`

**`tests/api_tags.rs`**:

- `tags_update` — `PUT /api/tags/7` with `{"name": "Groovy-Renamed"}`, verify via GET
- `tags_delete` — `DELETE /api/tags/{newly_created_id}`, verify 404 on re-fetch

**`tests/api_playlists.rs`**:

- `playlists_delete` — `DELETE /api/playlists/{newly_created_id}`, verify 404 on re-fetch

**`tests/api_folders.rs`**:

- `folders_create` — `POST /api/folders` with `{"folderPath": "/test/new", "active": true}`
- `folders_update` — `PUT /api/folders/1` with `{"active": false}`, verify via stats
- `folders_delete` — `DELETE /api/folders/{newly_created_id}`

### 🟡 Phase 4: Missing read endpoints

**`tests/api_tag_categories.rs`** — NEW FILE:

- `tag_categories_list` — `GET /api/tag-categories` returns 5 categories (Setlist, Phase, Mood, Vibe, Merkmal)
- `tag_categories_create` — `POST /api/tag-categories` creates a category

**`tests/api_tag_energy_levels.rs`** — NEW FILE:

- `tag_energy_levels_list` — `GET /api/tag-energy-levels` returns array

**Extend existing files**:

- `tests/api_tasks.rs` — `tasks_single_by_id` (use taskId from scan task)
- `tests/api_tasks.rs` — `tasks_cancel` — `DELETE /api/tasks/{taskId}`
- `tests/api_tags.rs` — `tags_parents_get` — `GET /api/tags/10/parents` returns parents (needs seed_tag_hierarchy)
- `tests/api_tags.rs` — `tags_parents_set` — `PUT /api/tags/10/parents` with body
- `tests/api_tags.rs` — `tags_from_playlists` — `GET /api/tags/from-playlists`
- `tests/api_tags.rs` — `tags_service_coverage` — `GET /api/tags/service-coverage`

### 🟡 Phase 5: Critical filter COMBINATIONS

The frontend sends multiple filters simultaneously. Verify count parity for:

| Combo                                           | Endpoint      |
| ----------------------------------------------- | ------------- |
| `isLocal=true` + `commentStatuses=needs_update` | `/api/files`  |
| `backedUp=true` + `isLocal=false`               | `/api/files`  |
| `hasLocal=true` + `hasBackup=true`              | `/api/tracks` |
| `pmvCategories=m,v` + `hasLocal=true`           | `/api/tracks` |

### 🟡 Phase 6: Digging tracks parameter coverage

**`tests/api_digging.rs`** — add ~5 tests for the `/api/digging/tracks` endpoint
params that the digging page ladder and filter rows send:
`energyLevels`, `keyList`, `keyRange`, `bpmMin`, `bpmMax`, `tags`, `sortBy`,
`sortOrder`, `pmvCategories`, `pmvAggregate`.

Each param gets one smoke test verifying the endpoint accepts it and returns
valid JSON. The digging engine already has complex logic tested via
`/api/digging/suggest`; these just verify the param plumbing.

### 🟢 Low priority (future rounds)

- Folder backup config tests (`PUT /api/folders/{id}/backup`, `PUT /api/folders/{id}/auto-backup`)
- Deemix queue CRUD (requires deemix-pyweb running)
- Traktor status/import (requires `.nml` file on disk — potentially testable)
- Storage backup/discover-backup (requires SSH/NAS)
- Tag similarities (depends on embeddings)
- `POST /api/restore` success path (requires valid dump JSON)
- Playlist subscriptions CRUD
- Dashboard-only endpoints (`/api/tags/from-playlists`, `/api/traktor/status`)

### Seed data changes needed

| Change                                                            | For                                |
| ----------------------------------------------------------------- | ---------------------------------- |
| Add `comment = "[PMV] groovy"` file to `seed_files_with_comments` | Phase 1 PMV tests                  |
| Add Phase-category tag + playlist + file link                     | Positive PMV category filter tests |

### Files to create

- `tests/api_tag_categories.rs` — 2 tests
- `tests/api_tag_energy_levels.rs` — 1 test

### Files to modify

- `tests/common/mod.rs` — add PMV-file to `seed_files_with_comments()`
- `tests/api_files.rs` — fix PMV placebo tests + add mutation tests
- `tests/api_tracks.rs` — add 13 tests (filters + mutations)
- `tests/api_tags.rs` — add 7 tests (mutations + read endpoints)
- `tests/api_playlists.rs` — add 1 test (delete)
- `tests/api_folders.rs` — add 3 tests (create, update, delete)
- `tests/api_digging.rs` — add ~5 tests (tracks params)
- `tests/api_tasks.rs` — add 2 tests (single, cancel)
- `frontend/pages/playlists.js` — fix `untagged` placebo (or add `untagged_only` to PlaylistsQuery)

### Acceptance Criteria

- [ ] PMV filter tests assert actual non-zero results (not placebo)
- [ ] `untagged` bug fixed (param handled or removed from frontend)
- [ ] All TracksQuery filter params tested (21/21)
- [ ] Files POST mutation endpoints tested (write-comments-by-ids, needs-comment-count)
- [ ] Tracks POST mutations tested (write-comments, backlog toggle)
- [ ] Tag update/delete endpoints tested
- [ ] Playlist delete endpoint tested
- [ ] Folder create/update/delete tested
- [ ] Digging tracks params have at least basic smoke coverage
- [ ] Tag-categories and tag-energy-levels endpoints tested
- [ ] `cargo build` passes
- [ ] All existing 190 tests still pass
- [ ] Total test count: ~230+

