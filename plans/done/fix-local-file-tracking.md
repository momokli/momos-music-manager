## Plan: fix-local-file-tracking

**Status**: done ✅
**Branch**: `fix/local-file-tracking`
**Depends on**: `feat/backup-as-truth` (already implemented)
**Migration needed**: no

### Description

Fix the disconnect between the data model and the UI. The `file_locations` table correctly tracks local vs backup presence, but every page queries `SELECT * FROM files` which counts ALL tracked files (including backup-only ones deleted from disk). The Storage page shows 10,638 "Local Files" when only ~3,361 are actually on disk. The Files page can't distinguish "on disk" from "backup-only". Fix: add `is_local` to every File API response, fix Storage page counts, add local-presence filter to Files page.

### Current State (from investigation 2026-06-03)

| Data point                      | Value                | Source                                                                             |
| ------------------------------- | -------------------- | ---------------------------------------------------------------------------------- |
| DB records (`files` table)      | 10,638               | `SELECT COUNT(*) FROM files`                                                       |
| Actually on disk                | ~3,361               | `ls` + `find` on filesystem                                                        |
| `file_locations.local` entries  | 8 (should be ~3,361) | Scanner code exists but never populated (incremental scan skipped unchanged files) |
| `file_locations.backup` entries | 10,019               | Correct ✅                                                                         |
| Files without backup            | 619 (all FLACs)      | Need backup before pruning                                                         |
| Prune candidates                | 0                    | Query requires `file_locations.local` → only 8 files have it                       |

**Root cause**: The scanner code to create `file_locations.local` was added (in `backup-as-truth` Part B) but the scanner never ran a full scan since then. The last scan was incremental, skipping all unchanged files. Only 8 files got local entries (presumably new/modified files picked up by the watcher).

**What IS correct**: The data model (`file_locations` with `local`/`backup`) is clean. The UI just queries the wrong data source.

### Backend Changes

#### 1. `src/db.rs` — `get_storage_status()`: count from file_locations.local

Replace `SELECT COUNT(*) FROM files` with counts from `file_locations WHERE location_type = 'local'`:

```rust
// Before (wrong): counts ALL tracked files
let local_file_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files")
    .fetch_one(pool).await.unwrap_or(0);

// After (correct): counts only files currently on disk
let local_file_count: i64 = sqlx::query_scalar(
    "SELECT COUNT(DISTINCT file_id) FROM file_locations WHERE location_type = 'local'"
)
    .fetch_one(pool).await.unwrap_or(0);
```

Same change for `local_size_bytes`, and all per-type counts (`local_stems`, `local_flacs`, `local_wavs`, `local_mp3s`). Join through `file_locations` with `location_type = 'local'`.

Add a new field: `tracked_file_count: i64` for the total archive size (`COUNT(*) FROM files`). Rename existing fields for clarity — keep `local_file_count` meaning "on disk" (data source changes from `files` to `file_locations.local`).

#### 2. `src/db.rs` — `get_files()`: add `is_local` field

The `get_files()` function builds dynamic SQL. Add a LEFT JOIN to `file_locations` for local presence:

```sql
SELECT f.*,
  COALESCE(fl_backup.id IS NOT NULL, 0) as backed_up,
  COALESCE(fl_local.id IS NOT NULL, 0) as is_local
FROM files f
LEFT JOIN file_locations fl_backup ON fl_backup.file_id = f.id AND fl_backup.location_type = 'backup'
LEFT JOIN file_locations fl_local ON fl_local.file_id = f.id AND fl_local.location_type = 'local'
WHERE 1=1 ...
```

Add an `isLocal` filter to `FilesQuery`:

```rust
pub is_local: Option<bool>, // Some(true) = only local, Some(false) = only backup-only, None = all
```

When `is_local = Some(true)`: add `AND fl_local.id IS NOT NULL` — only files on disk.
When `is_local = Some(false)`: add `AND fl_local.id IS NULL` — only backup-only files.

#### 3. `src/api.rs` — `ApiFile` struct: add `is_local` field

```rust
pub struct ApiFile {
    // ... existing fields ...
    pub backed_up: bool,
    pub is_local: bool,       // NEW
    pub has_stem: bool,
    pub safe_to_delete: bool,
}
```

The `backedUp` filter already exists in `FilesQuery`. Keep it. Add the `isLocal` filter alongside it.

### Frontend Changes

#### 4. `frontend/pages/storage.js` — Overhaul Storage cards

Current cards: Local Files | Backed Up | Prune Candidates.

New cards:

```
On Disk         │ On Backup        │ Tracked
3,361 files     │ 10,019 files     │ 10,638 files
70 GB           │                  │ 431.9 GB (archive)

Not Backed Up   │ Prune Candidates
619 FLACs       │ X files · Y GB can be freed
64.3 GB         │
```

All counts come from `StorageStatus` response fields. The frontend already receives the data correctly after the backend fix (step 1).

#### 5. `frontend/pages/files.js` — Add "On Disk" filter button

In the toolbar filter panel (RIGHT column, near the Backup filter), add:

```html
<div class="filter-row">
  <span class="filter-row-label toggleable" data-filter="local">On Disk</span>
  <div class="filter-group">
    <button class="filter-btn" data-local-filter="all">All</button>
    <button class="filter-btn" data-local-filter="yes">
      <i class="fas fa-hdd"></i> Yes
    </button>
    <button class="filter-btn" data-local-filter="no">
      <i class="fas fa-cloud"></i> No
    </button>
  </div>
</div>
```

Add `isLocal: null` to state and hash. Wire `buildParams` to send `isLocal` and `buildFilterParams` to pass it. The comment writer should show a warning/badge when the file is not local ("Backup only — can't write comment").

### Files to modify

- `src/db.rs` — fix `get_storage_status()` counts, add `is_local` to `get_files()`, add `isLocal` to `FilesQuery`
- `src/api.rs` — add `is_local: bool` to `ApiFile`, add `isLocal` param support in `files_handler`
- `frontend/pages/storage.js` — overhaul card layout to 5 cards
- `frontend/pages/files.js` — add "On Disk" filter button, non-local warning in comment writer
- `frontend/style.css` — storage card layout adjustments if needed

### Not in this plan

- Running a full scan to populate `file_locations.local` — **CRITICAL**: without this, Storage page shows 0 "On Disk" files (worse than current). Scan trigger MUST be added to this plan.
- Backing up the 619 unbacked-up FLACs — user triggers manually via Storage page

### Acceptance Criteria

- [ ] `get_storage_status()` counts local files from `file_locations.local`, NOT from `files`
- [ ] Storage page shows "On Disk" count — with warning when local entries are empty
- [ ] Storage page shows "Tracked" as total archive size (`COUNT(*) FROM files`)
- [ ] Storage page shows "Not Backed Up" count (files without backup records)
- [ ] Full scan trigger on Storage page when `file_locations.local` is empty
- [ ] `ApiFile` includes `isLocal: bool` field
- [ ] Files page has "On Disk" filter (All / Yes / No)
- [ ] Files page shows non-local indicator when file is backup-only
- [ ] `isLocal` filter included in `get_files_count()` count query
- [ ] Comment writer deactivated or warns when file is not local
- [ ] `TracksQuery.has_local` fixed — currently queries `v_file_track_link` without joining `file_locations` for local presence
- [ ] `ApiFile.safe_to_delete` logic reviewed: currently `backed_up && has_stem` — should also check `is_local`
- [ ] `cargo build` passes
- [ ] No regressions: backup/reconcile/prune flows unchanged

### Additional concerns (investigated, not yet planned)

#### Track-centric file view

The user thinks in TRACKS, not files. A track can have multiple file versions: FLAC, stem.m4a, and WAV source parts. The track-detail page (`#track-detail?id=1487`) shows ALL file variants grouped by type:

- **Now working**: API returns 7 files (1 FLAC + 1 stem + 5 WAVs), frontend splits them into "Linked Files" + "WAV Sources" sections
- **Gap**: The WAV files show `backedUp: true` but are NOT on local disk. The path `/Users/momo/Music/stems/Boris_Brejcha_Black_Unicorn/...` is stale — the directory was deleted locally after backup. Without `is_local`, the UI can't tell the user "this file exists on backup only."
- Verified: 10+ tracks in the DB have all three types (WAV + FLAC + stem) simultaneously, but many WAVs are backup-only

#### All file versions on backup

619 FLACs are not yet backed up. Every track's file versions should eventually all be on backup. The "Not Backed Up" card on the Storage page shows this count. User should back these up via the Backup button per folder.

#### Conditional local keeping (format preference per-track)

User's rule:

- If a track has a **stem.m4a** locally → the FLAC can be pruned (if backed up)
- If a track has **only FLAC** (no stem) → keep FLAC locally for offline/Traktor use

The `hasStemVariant` badge on the prune preview already supports this per-file. But the user wants the prune preview organized **by track** — showing "for Track X: stem ✓, FLAC (redundant), WAVs (redundant)" grouped together.

#### Backpack feature for offline availability

The current "Follow" mechanism on Tags and "Backpack" toggle on Tracks are **the same thing** — both set `tags.followed = true` which causes `get_prune_candidates()` to exclude files with that tag. The mechanism is consistent but **passive** — it only prevents deletion, it doesn't actively pull files from backup or ensure the right format is local.

**What the user wants**:

- Rename "Follow" → "Backpack" everywhere (more intuitive: "I want these offline")
- A track is "in backpack" if EITHER individually tagged OR in a backpack tag
- Actively pull files from backup when they're not local
- Prefer format: stem > FLAC > MP3 (exactly one version per track)
- Clean up redundant formats when a better one exists

**Current flow:** Tag "Backpack" toggled → Files in that tag won't be pruned. That's it.

**Needed flow:** Tag "Backpack" toggled → Background task checks all tracks → pulls missing files from backup → prefers stem over FLAC → ensures exactly one version per track is local.

**UX flow sketch:**

```
Tags page: toggle "digging-2026-05-26" → Backpack
    ↓
Background "Backpack Sync" task starts:
    For each track in the tag:
      ├─ stem.m4a on disk? → keep, mark FLAC as safe-to-delete
      ├─ FLAC on disk (no stem)? → keep
      ├─ stem on backup only? → pull from backup
      ├─ Nothing anywhere? → skip (can't pull what doesn't exist)
    ↓
Notify: "Backpack sync complete: 5 files pulled, 3 formats cleaned"
```

**Possible new page**: A dedicated "Backpack" page showing all backpack tracks with their file status — which are local, which are backup-only, which need pulling. Could also show per-track format status (stem ✓, FLAC redundant, etc.).

**Dependencies**: Needs file_locations.local (the Maintainer) and is_local on track-detail before this can be built.

---

