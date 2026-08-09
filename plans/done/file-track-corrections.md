## Plan: file-track-corrections

**Status**: done ✅
**Branch**: `feat/file-track-corrections`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — `023_file_track_corrections.sql`

### Description

A `file_track_corrections` table that overrides the automatic file↔track linking
in `v_file_track_link`. When deemix delivers a wrong-artist file whose ISRC
happens to match a Spotify track (same ISRC, different artist), the user can
**exclude** the wrong link and **include** the correct one. The override is
transparent to every consumer — the view handles it.

### Why

`v_file_track_link` matches files to service tracks via ISRC or direct
service_id columns. When deemix downloads the wrong artist's version of a song
(same ISRC, different performer), the file gets linked to the Spotify track
automatically — and the correct file (which may have no ISRC at all) stays
unlinked. There is no way to fix this without editing the DB by hand.

**Real example**: "Red Sun In The Sky" by Gippeul (Spotify track #327244,
ISRC `QZFZ32206098`). Deemix delivered the Fortuna version (file #1529941)
with the same ISRC → auto-linked. The actual Gippeul mp3 (file #1531714) has
no ISRC → never linked. The track-detail page shows the wrong file.

### Audit: Zero leaks — one view controls everything

96 references to `v_file_track_link` across 10 source files. Every single
file↔track relationship in the entire codebase flows through this view:

| Consumer                                                                                                       | Pattern                                                         |
| -------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| `get_track_detail()`                                                                                           | `FROM v_file_track_link v JOIN files f`                         |
| `get_file_detail()`                                                                                            | `FROM v_file_track_link v JOIN service_tracks st`               |
| `get_files()` all filters (hasLocal, hasBackup, linkedOnly, fileTypes, BPM, Key, Rating, PlayCount, PMV, etc.) | `EXISTS (SELECT 1 FROM v_file_track_link ...)`                  |
| `get_tracks()` all file-metric filters                                                                         | `EXISTS (SELECT 1 FROM v_file_track_link vft JOIN files f ...)` |
| `file_resolved_tags` materialization                                                                           | `INSERT FROM v_file_resolved_tags` → `JOIN v_file_track_link`   |
| `v_file_tags` / `v_file_resolved_tags` views                                                                   | `JOIN v_file_track_link v ON v.file_id = f.id`                  |
| `digging.rs` suggestions + track browser                                                                       | `JOIN v_file_track_link vft` (4 places)                         |
| `download_guarantor.rs` gap analysis                                                                           | `SELECT track_id FROM v_file_track_link`                        |
| `dynamic_bundles.rs` track resolution                                                                          | `FROM v_file_track_link vft`                                    |
| Service links counter                                                                                          | `COUNT(DISTINCT v.file_id) FROM v_file_track_link`              |
| `push_to_spotify` resolve track IDs                                                                            | `JOIN v_file_track_link`                                        |
| `v_tag_file_counts` → Tags page file counts                                                                    | `FROM v_file_tags` (depends on v_file_track_link)               |

**Zero direct file↔service_tracks JOINs bypass the view.** Modifying
`v_file_track_link` fixes every consumer with no code changes.

### Schema

#### Migration 023 (`migrations/023_file_track_corrections.sql`)

```sql
-- Migration 023: Manual file↔track link overrides
--
-- Adds a corrections table that takes precedence over automatic
-- ISRC/service_id matching in v_file_track_link.
--
-- link_type = 'include' → explicitly link this file to this track
-- link_type = 'exclude' → explicitly prevent automatic linking

-- Step 1: Create the corrections table
CREATE TABLE IF NOT EXISTS file_track_corrections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES service_tracks(id) ON DELETE CASCADE,
    link_type TEXT NOT NULL CHECK (link_type IN ('include', 'exclude')),
    reason TEXT,
    created_at INTEGER DEFAULT (unixepoch()),
    UNIQUE(file_id, track_id)
);

CREATE INDEX IF NOT EXISTS idx_ftc_file ON file_track_corrections(file_id);
CREATE INDEX IF NOT EXISTS idx_ftc_track ON file_track_corrections(track_id);

-- Step 2: Drop dependent views (must drop leaf views first)
DROP VIEW IF EXISTS v_tag_file_counts;
DROP VIEW IF EXISTS v_file_resolved_tags;
DROP VIEW IF EXISTS v_file_tags;
DROP VIEW IF EXISTS v_file_track_link;

-- Step 3: Recreate v_file_track_link with correction overrides
--
-- Manual includes always win. Automatic matches are excluded
-- when an 'exclude' correction exists for that pair.
CREATE VIEW v_file_track_link AS
-- Manual includes (always win)
SELECT file_id, track_id FROM file_track_corrections WHERE link_type = 'include'
UNION
-- Automatic matches, minus excluded pairs
SELECT f.id AS file_id, st.id AS track_id
FROM files f
JOIN service_tracks st ON (
    st.isrc = f.isrc
    OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
    OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
    OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
    OR (st.service = 'local' AND st.service_id = CAST(f.id AS TEXT))
)
WHERE NOT EXISTS (
    SELECT 1 FROM file_track_corrections ftc
    WHERE ftc.file_id = f.id
      AND ftc.track_id = st.id
      AND ftc.link_type = 'exclude'
);

-- Step 4: Recreate v_file_tags (identical to migration 008)
CREATE VIEW v_file_tags AS
SELECT DISTINCT f.id AS file_id,
       t.id AS tag_id, t.name AS tag_name,
       t.sort_order, t.created_at,
       tc.id AS category_id, tc.name AS category_name,
       tc.is_default, tc.prefix
FROM files f
JOIN v_file_track_link v ON v.file_id = f.id
JOIN service_playlist_tracks spt ON spt.track_id = v.track_id
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN tag_categories tc ON tc.id = t.category_id
WHERE sp.archive_deleted = 1 OR spt.deleted_at IS NULL;

-- Step 5: Recreate v_file_resolved_tags (identical to migration 008)
CREATE VIEW v_file_resolved_tags AS
SELECT DISTINCT
    f.id AS file_id,
    rt.tag_id,
    rt.tag_name,
    rt.sort_order,
    rt.created_at,
    rt.category_id,
    rt.category_name,
    rt.prefix
FROM files f
JOIN v_file_track_link v ON v.file_id = f.id
JOIN service_playlist_tracks spt ON spt.track_id = v.track_id
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN v_resolved_tags rt ON rt.source_tag_id = t.id
WHERE sp.archive_deleted = 1 OR spt.deleted_at IS NULL;

-- Step 6: Recreate v_tag_file_counts (identical to migration 008)
CREATE VIEW v_tag_file_counts AS
SELECT vft.tag_id, COUNT(DISTINCT vft.file_id) AS file_count
FROM v_file_tags vft
GROUP BY vft.tag_id;

SELECT 'Migration 023 applied: file_track_corrections table + updated v_file_track_link' as status;
```

### Backend: API Endpoints

#### New file: `src/api/file_track_corrections.rs`

```rust
pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        // Corrections for a specific file (what tracks is it linked to?)
        .route("/api/files/{id}/track-corrections", get(list_for_file).put(set_for_file))
        // Corrections for a specific track (what files is it linked to?)
        .route("/api/tracks/{id}/file-corrections", get(list_for_track).put(set_for_track))
        // Delete a single correction
        .route("/api/file-track-corrections/{id}", delete(delete_correction))
}
```

##### `GET /api/files/{id}/track-corrections`

Returns the current link state for a file:

```json
{
  "data": {
    "fileId": 1529941,
    "automaticLinks": [
      {
        "trackId": 327244,
        "title": "Red Sun In The Sky",
        "artist": "Gippeul",
        "service": "spotify",
        "reason": "isrc"
      }
    ],
    "manualIncludes": [],
    "manualExcludes": [],
    "effectiveLinks": [
      { "trackId": 327244, "title": "Red Sun In The Sky", "artist": "Gippeul" }
    ]
  }
}
```

Fields:

- `automaticLinks` — what the ISRC/service_id matching would produce (before corrections)
- `manualIncludes` — explicit includes from `file_track_corrections`
- `manualExcludes` — explicit excludes
- `effectiveLinks` — what `v_file_track_link` actually returns (the truth)

##### `PUT /api/files/{id}/track-corrections`

Request body:

```json
{
  "corrections": [
    {
      "trackId": 327244,
      "linkType": "exclude",
      "reason": "wrong artist — Fortuna, not Gippeul"
    },
    { "trackId": 1531714, "linkType": "include", "reason": "correct file" }
  ]
}
```

Wait — `trackId` for include means the file should be linked to that track.
For exclude, it means the file should NOT be linked to that track.
Both are keyed on `(file_id, track_id)`.

Handler logic:

1. Validate: every correction has `trackId` and `linkType` (include or exclude)
2. Verify the track exists (404 if not)
3. DELETE existing corrections for this file (for the tracks being updated)
4. INSERT new corrections
5. Return updated state (same shape as GET)

##### `GET /api/tracks/{id}/file-corrections`

Same shape as the file endpoint but from the track's perspective.
Shows which files are linked (effective) and any manual overrides.

##### `DELETE /api/file-track-corrections/{id}`

Delete a single correction by its primary key. Returns 204.

##### Validation rules

| Case                                               | Response                    |
| -------------------------------------------------- | --------------------------- |
| `linkType` not "include" or "exclude"              | 400                         |
| `trackId` doesn't exist                            | 404                         |
| `fileId` doesn't exist                             | 404                         |
| Empty corrections array                            | 400                         |
| Duplicate (file_id, track_id) with same linkType   | Idempotent — 200, no change |
| Changing linkType for existing (file_id, track_id) | Updates the row             |

#### Router integration

In `src/api/mod.rs`, merge the new router:

```rust
pub mod file_track_corrections;
// ...
.merge(file_track_corrections::router())
```

### Frontend: Track Detail Page

#### `frontend/pages/track-detail.js` — Linked Files section

Currently shows linked files. Add:

- ✕ button on each linked file card: "Disconnect this file" → creates `exclude` correction
- "Link a file…" typeahead at the bottom: searches local files by name, clicking adds an `include` correction
- After any correction: refresh the linked-files list (it now reflects the effective view)

The ✕ button uses `PUT /api/files/{fileId}/track-corrections` with:

```json
{ "corrections": [{ "trackId": 327244, "linkType": "exclude" }] }
```

The typeahead uses `GET /api/files?search=...&isLocal=true&pageSize=10` to find files,
then `PUT /api/files/{fileId}/track-corrections` with:

```json
{ "corrections": [{ "trackId": 327244, "linkType": "include" }] }
```

#### `frontend/pages/file-detail.js` — Linked Tracks section

Currently shows linked tracks. Add:

- ✕ button per linked track: "Disconnect" → `exclude` correction
- The effective links refresh after correction

### Files to create

| File                                        | Description                      |
| ------------------------------------------- | -------------------------------- |
| `migrations/023_file_track_corrections.sql` | New migration                    |
| `src/api/file_track_corrections.rs`         | 3 handlers + router (~120 lines) |
| `tests/api_file_track_corrections.rs`       | Integration tests (~10 tests)    |

### Files to modify

| File                             | Change                                                                     |
| -------------------------------- | -------------------------------------------------------------------------- |
| `src/api/mod.rs`                 | Add `pub mod file_track_corrections;` + merge router                       |
| `frontend/pages/track-detail.js` | ✕ disconnect button + "Link a file…" typeahead + correction-driven refresh |
| `frontend/pages/file-detail.js`  | ✕ disconnect button per linked track                                       |
| `frontend/style.css`             | `.correction-badge` styles                                                 |

### Acceptance Criteria

**Migration:**

- [ ] Migration 023 runs cleanly on fresh DB (001→023)
- [ ] Migration 023 runs cleanly on existing DB with data
- [ ] `file_track_corrections` table created with CHECK constraint + indexes
- [ ] `v_file_track_link` returns manual includes
- [ ] `v_file_track_link` excludes pairs with `link_type = 'exclude'`
- [ ] `v_file_tags` / `v_file_resolved_tags` / `v_tag_file_counts` work identically after migration

**API:**

- [ ] `GET /api/files/{id}/track-corrections` returns automaticLinks, manualIncludes, manualExcludes, effectiveLinks
- [ ] `PUT /api/files/{id}/track-corrections` creates/updates corrections
- [ ] `GET /api/tracks/{id}/file-corrections` returns linked files with correction status
- [ ] `DELETE /api/file-track-corrections/{id}` removes a correction
- [ ] Invalid linkType → 400
- [ ] Non-existent trackId/fileId → 404
- [ ] Idempotent: same correction twice → 200, no duplicate rows

**Correction behavior:**

- [ ] `include(file_id=X, track_id=Y)` makes file X appear in `get_track_detail(Y).files`
- [ ] `exclude(file_id=X, track_id=Y)` removes file X from `get_track_detail(Y).files`
- [ ] Exclude wins over automatic ISRC match
- [ ] Include works even when file has no ISRC / no service_id
- [ ] Include + exclude for same pair → include wins (UNION places it first, exclude only filters the automatic leg)
- [ ] After `exclude(1529941, 327244)` + `include(1531714, 327244)`: track-detail shows only the Gippeul file

**Global propagation (zero leaks):**

- [ ] `get_files()` linkedOnly/unlinked filters respect corrections
- [ ] `get_tracks()` hasLocal/hasBackup/fileTypes filters respect corrections
- [ ] `file_resolved_tags` materialization reflects corrections (after `refresh_file_resolved_tags`)
- [ ] Comment computation uses corrected file→tag chain
- [ ] Backpack system uses corrected file→track→tag chain
- [ ] Digging suggestions use corrected file→track links
- [ ] Download guarantor gap analysis uses corrected links
- [ ] Dynamic bundles track resolution uses corrected links
- [ ] Tags page file counts reflect corrections
- [ ] Playlists page track counts reflect corrections

**Frontend:**

- [ ] Track-detail page: ✕ button on each linked file (creates exclude correction)
- [ ] Track-detail page: "Link a file…" typeahead (creates include correction)
- [ ] Track-detail page: linked files list refreshes after any correction
- [ ] File-detail page: ✕ button on each linked track
- [ ] No page errors (Playwright smoke test)

**Validation:**

- [ ] `cargo build` passes
- [ ] `cargo test` passes (all existing + new tests)
- [ ] `cd frontend && npx playwright test` passes
- [ ] Manual test: correct the "Red Sun In The Sky" mismatch, verify track-detail shows Gippeul only

### Agent Decomposition (TDD, 3 agents, zero file conflicts)

| Agent | Files                                                                                                                | Work                                                              |
| ----- | -------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| **A** | `migrations/023_*.sql`, `src/api/file_track_corrections.rs`, `src/api/mod.rs`, `tests/api_file_track_corrections.rs` | Migration + API handlers + router + integration tests (~10 tests) |
| **B** | `frontend/pages/track-detail.js`, `frontend/pages/file-detail.js`, `frontend/style.css`                              | ✕ disconnect buttons + "Link a file…" typeahead + styles          |
| **C** | `frontend/tests/track-detail.spec.js`, `frontend/tests/file-detail.spec.js`                                          | Playwright tests for correction UI flows                          |

**Write scope verification — zero overlap:**

- Agent A: `migrations/`, `src/api/`, `tests/`
- Agent B: `frontend/pages/`, `frontend/style.css`
- Agent C: `frontend/tests/`

Agents B and C can run in parallel (different files). Agent A is independent.

### Per-Agent Task Briefs

#### Agent A: Backend + Integration Tests

1. Create migration file `migrations/023_file_track_corrections.sql`
2. Write integration tests in `tests/api_file_track_corrections.rs` FIRST (they will fail):
   - `corrections_include_links_file_to_track` — include makes file appear in track-detail
   - `corrections_exclude_unlinks_file_from_track` — exclude removes ISRC-matched file from track-detail
   - `corrections_include_works_without_isrc` — file with no ISRC can be included
   - `corrections_exclude_wins_over_auto_isrc_match` — excluded file not in v_file_track_link
   - `corrections_idempotent_include` — same include twice → 200, one row
   - `corrections_invalid_link_type_400` — "xyz" → 400
   - `corrections_nonexistent_track_404` — trackId 999999 → 404
   - `corrections_delete_removes_correction` — DELETE → correction gone, auto-link restored
   - `corrections_list_for_file_shows_all_fields` — GET returns automaticLinks, manual\*, effectiveLinks
   - `corrections_list_for_track_shows_linked_files` — GET from track perspective
3. Create `src/api/file_track_corrections.rs` with 4 handlers + router
4. Add `pub mod file_track_corrections;` + `.merge(...)` to `src/api/mod.rs`
5. Run `cargo test --test api_file_track_corrections` → all green
6. Run `cargo build` → no errors

#### Agent B: Frontend UI

1. Read `frontend/pages/track-detail.js` and `frontend/pages/file-detail.js`
2. Track-detail: add ✕ button on each linked file card. Click handler calls `PUT /api/files/{fileId}/track-corrections` with exclude, then re-fetches track detail.
3. Track-detail: add "Link a file…" typeahead below the linked files list. Searches `GET /api/files?search=...&isLocal=true`. Clicking a result calls `PUT` with include, then re-fetches.
4. File-detail: add ✕ button on each linked track card. Same pattern.
5. Add `.correction-badge` and `.disconnect-btn` styles to `frontend/style.css`
6. Verify manually: open `#track-detail?id=327244`, disconnect Fortuna, link Gippeul, verify refresh

#### Agent C: Playwright Tests

1. Create `frontend/tests/track-detail.spec.js` if not existing (or extend):
   - `can disconnect a linked file` — click ✕, file disappears
   - `can link a file via typeahead` — search, click result, file appears
2. Create/extend `frontend/tests/file-detail.spec.js`:
   - `can disconnect a linked track` — click ✕, track disappears
3. Run `cd frontend && npx playwright test` → all green

### Execution Order

All 3 agents can run **simultaneously** — zero file conflicts.
After all complete: `cargo build && cargo test && cd frontend && npx playwright test`.
