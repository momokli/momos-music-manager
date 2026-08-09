## Plan: backup-as-truth

**Status**: proposed
**Branch**: `feat/backup-as-truth`
**Ready for review**: no
**Depends on**: `feat/wav-source-linking` (Phase 1-2 done, Phase 3 partial)
**Migration needed**: yes — `013_backup_discovery.sql`

### Description

Rethink the file lifecycle model: treat the NAS backup as the **source of truth** and local disk as a **working cache**. Files flow: NAS → load to local → Traktor scan for BPM/key → write comment → confirm backup → delete local. This requires: tracking local file presence explicitly, discovering backup-only files, enriching track-detail with WAV source variants, and changing the prune criteria to require metadata completeness.

### Investigation Results (2026-05-28)

**Current data model gap:**

| Concept               |       Currently Tracked        | Issue                                                                                                  |
| --------------------- | :----------------------------: | ------------------------------------------------------------------------------------------------------ |
| File metadata         |         `files` table          | ✅ OK                                                                                                  |
| Backup location       | `file_locations` (type=backup) | ✅ OK                                                                                                  |
| Local presence        |        **Not tracked**         | ❌ Implicit: "being in `files` means local" — but files can be deleted locally and still be in `files` |
| Backup-only files     |       **Not discovered**       | ❌ Files on NAS with no local DB record are invisible to the app                                       |
| Metadata completeness |           Partially            | ❌ 10,622 files total, only 3,726 have BPM+Key, 3,849 have comments                                    |

**Key findings:**

- 0 `file_locations` entries with `location_type='local'` — local presence is implicit
- 10,019 files backed up, 603 not backed up (all FLACs)
- **track-detail page has NO variant/source integration** — `get_track_detail()` doesn't traverse `source_of`
- Track #1487 (Boris Brejcha - Black Unicorn) has 2 linked files (FLAC + stem) but 5 WAV sources linked to the stem are invisible
- WAV linking works in isolation: `GET /api/files/{id}/variants` shows WAV sources when querying the stem file

**The conceptual gap:**

1. `track-detail` → linked files → but stops there, doesn't traverse `source_of` to find WAV sources
2. `file-detail` → variants → works for the specific file, but the track-detail user never sees this connection
3. Backup reconciliation only matches REMOTE files against LOCAL files in DB — if a file was deleted locally, it stays in DB but there's no way to know it's no longer on disk
4. Files on NAS that were NEVER scanned locally are completely invisible

### Part A: Enrich Track Detail with WAV Source Variants

Extend `get_track_detail()` and the track-detail page to show all file variants, not just directly-linked files.

#### Backend: `src/db.rs` — extend `get_track_detail()`

After fetching linked files (step 2), add a step to fetch their WAV source children:

```rust
// Step 2b: For each linked stem file, fetch WAV source files
let stem_ids: Vec<i64> = files.iter().map(|f| f.id).collect();
if !stem_ids.is_empty() {
    // Find WAV files whose source_of points to any linked file
    let placeholders: Vec<String> = stem_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT f.id, f.file_path, f.file_type, f.file_size, f.stem_type, f.isrc,
                f.title, f.artist, f.bpm, f.musical_key, f.duration_ms,
                COALESCE(fl_backup.id IS NOT NULL, 0) as backed_up,
                fl_backup.path as backup_path
         FROM files f
         LEFT JOIN file_locations fl_backup ON fl_backup.file_id = f.id AND fl_backup.location_type = 'backup'
         WHERE f.source_of IN ({}) AND f.file_type = 'wav'
         ORDER BY f.stem_type",
        placeholders.join(",")
    );
    let mut query = sqlx::query_as::<_, TrackDetailFile>(&sql);
    for id in &stem_ids {
        query = query.bind(id);
    }
    let wav_files = query.fetch_all(pool).await.unwrap_or_default();
    files.extend(wav_files);
}
```

The WAV files already have `stem_type` set — the frontend can use it to render "WAV (vocals)", "WAV (bass)", etc.

#### Backend: `src/db.rs` — `TrackDetailFile` struct

Add `stem_type: Option<String>` field to `TrackDetailFile`:

```rust
#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDetailFile {
    pub id: i64,
    pub file_path: String,
    pub file_type: String,
    pub stem_type: Option<String>,  // NEW: for WAV sources
    // ... existing fields ...
}
```

Make sure the SQL queries in `get_track_detail()` include `f.stem_type` in the SELECT.

#### Frontend: `frontend/pages/track-detail.js`

Add variant cards rendering for each linked file. When a file has `stem_type` set, show it as a WAV source badge.

In the renderFileInfo section (or as a new Variants section), iterate `data.files` and show:

- File type badge (FLAC, stem.m4a, or WAV with stem_type label)
- Backup status indicator
- File size

Group by `source_of` / file type to make the relationship visible:

```
stem.m4a (124 BPM, 9m) ✓ backed up
  ├── WAV vocals    ✓
  ├── WAV bass      ✓
  ├── WAV drums     ✓
  ├── WAV instrumental ✓
  └── WAV other     ✓
FLAC                   ✓
```

### Part B: Track Local File Presence

Add explicit `file_locations` entries with `location_type = 'local'` so we can distinguish "file is on disk" from "file is in DB but deleted."

#### Migration 013 (`migrations/013_backup_discovery.sql`)

```sql
-- Migration 013: Track local file presence + backup discovery support

-- Add last_verified_local to files for tracking when the file was last confirmed on disk
-- NULL = never been local (backup-only), timestamp = last confirmed on disk
ALTER TABLE files ADD COLUMN last_verified_local INTEGER;

-- Note: DO NOT backfill file_locations.local entries here!
-- The scanner populates them on next scan (see scan_and_store_file changes).
-- A blind backfill would mark deleted files as "local" — false positives.

SELECT 'Migration 013 applied: local file tracking + backup discovery support' as status;
```

#### Rust: `src/db.rs` — `File` struct

Add `last_verified_local: Option<i64>` field to `File` struct.

#### Rust: `src/db.rs` — Scanner changes

**`scan_and_store_file()`** already preserves `source_of`/`stem_type` via COALESCE (done in earlier plan). Now also needs to create/update a `file_locations` entry with `location_type = 'local'` after the successful INSERT/UPDATE:

```rust
// In scan_and_store_file, after successful INSERT/UPDATE (before Ok(row)):
let _ = sqlx::query(
    "INSERT INTO file_locations (file_id, location_type, path, file_size, last_verified, created_at)
     VALUES (?, 'local', ?, ?, unixepoch(), unixepoch())
     ON CONFLICT(file_id, location_type) DO UPDATE SET
         file_size = excluded.file_size,
         last_verified = excluded.last_verified"
)
.bind(row.id)
.bind(&row.file_path)
.bind(row.file_size)
.execute(pool)
.await;

// Also update last_verified_local on the files row
let _ = sqlx::query("UPDATE files SET last_verified_local = unixepoch() WHERE id = ?")
    .bind(row.id)
    .execute(pool)
    .await;
```

#### Cleanup of stale local entries

The folder watcher polls active folders every 5 min. When a file that was previously scanned disappears from disk, the scanner correctly skips it (no re-scan). But the `file_locations.local` entry from the previous scan persists forever.

**Fix**: Add this cleanup to `scan_folder()` in `src/db.rs` (called by both manual scans via API and automatic scans via the folder watcher). This ensures stale local entries are purged regardless of scan trigger. After the walk loop, before `Ok(count)`, add:

This mirrors how `scan_and_store_file` works: it UPDATEs `last_scanned` for every file it encounters. Files that weren't encountered keep their old `last_scanned`:

```rust
// After folder scan completes, remove stale local entries
let folder_path = /* ... */;
sqlx::query(
    "DELETE FROM file_locations WHERE location_type = 'local'
     AND file_id IN (
         SELECT f.id FROM files f
         JOIN folders fol ON fol.folder_path = substr(f.file_path, 1, length(fol.folder_path))
         WHERE fol.id = ? AND f.last_scanned < ?
     )"
)
.bind(folder_id)
.bind(scan_start_time)
.execute(pool)
.await?;
```

This ensures `file_locations.local` always reflects reality.

#### Frontend: File detail + Storage page

Show local presence status clearly:

- ✓ Local (verified 2 days ago)
- ✓ Backed up (verified 2 days ago)
- ✗ Local (not on disk)

### Part C: Backup-Only File Discovery

Add the ability to scan the NAS and discover files that exist ONLY on backup, not in the local DB. These are files that were backed up through some other process, or backed up and then the DB record was lost.

#### New API endpoint: `POST /api/storage/discover-backup/{folder_id}`

Triggers a background task that:

1. Lists ALL files on the NAS backup destination for a folder with **full remote paths** (not just filenames)
2. For each remote file, checks if there's a matching entry in the `files` table (by filename match — all 10,622 filenames are currently unique)
3. For files that EXIST in `files` but NOT on backup: logs warning (removed from NAS?)
4. For files that exist on backup but NOT in `files`: creates a "backup-only" file record:
   - Create a `files` row with `file_path` reconstructed from `folder.folder_path` + remote relative path
   - Create `file_locations` entry with `location_type='backup'`
   - Mark as `last_verified_local = NULL` (never been local)
   - **`file_hash`**: `files.file_hash` is `NOT NULL` — use a sentinel like `"backup-only-{file_size}"` for files we can't compute a real hash on.

**Potential fragility**: The current `list_remote_files_with_depth()` in `src/backup/mod.rs` strips paths to basenames only (line 149). For backup discovery + pull-from-backup, we need full remote paths. Create a new method `list_remote_files_full()` that returns paths relative to backup base — keeps the existing function unchanged for reconciliation.

**Filename matching**: Match by reconstructed full path (`folder.folder_path` + remote relative path), not by basename alone. Though all local filenames are currently unique, the NAS can have identically-named files in different subdirectories (e.g., `Artist1/Title_vocals.wav` and `Artist2/Title_vocals.wav`).

```rust
pub struct BackupDiscoveryResult {
    pub files_on_backup: usize,       // total files found on NAS
    pub already_tracked: usize,        // files already in DB with backup record
    pub newly_discovered: usize,       // files on backup but not in DB → created
    pub missing_from_backup: Vec<(i64, String)>,  // in DB but not on backup (path, reason)
}
```

This gives the app a complete inventory of what's on backup.

### Part D: Metadata Completeness as Prune Gate

Change the prune criteria: being backed up is NOT enough. A file is safe to delete locally only when:

1. ✅ Backed up to NAS (has `file_locations` with `type='backup'`)
2. ✅ Metadata extracted (has `bpm IS NOT NULL` or `comment IS NOT NULL` — at least one)
3. ✅ Not followed (not in any followed tag)
4. ✅ Local presence confirmed (has `file_locations` with `type='local'`)

**Rationale:**

- WAV source files have NO metadata (they're raw audio) — but they're linked to stems. The condition for WAVs should be: `source_of IS NOT NULL` (linked to stem) instead of metadata check.
- Stems and FLACs should have BPM/key or comment extracted before safe deletion.
- 603 FLACs lack backup — they should be backed up first.

#### Modified prune query:

```sql
-- Step 1: backed-up file IDs (now includes WAVs)
SELECT DISTINCT fl.file_id FROM file_locations fl
JOIN files f ON f.id = fl.file_id
WHERE fl.location_type = 'backup'
  AND (f.file_type != 'wav' OR (f.file_type = 'wav' AND f.source_of IS NOT NULL));

-- Step 2: filter for metadata completeness (for non-WAV files)
-- Non-WAV: must have bpm or comment
-- WAV: source_of IS NOT NULL is already checked above
AND (
    f.file_type = 'wav'
    OR f.bpm IS NOT NULL
    OR (f.comment IS NOT NULL AND f.comment != '')
);

-- Step 3: not followed (existing filter)
-- Step 4: has local presence (should be deletable)
```

### Part F: Replace Format Preference Toggle with Interactive Prune Preview Filters

**Problem with current "Prefer stem files" toggle:**

The existing checkbox in the Storage page suffers from four issues:

1. **Wrong numbers**: The hint says "Currently 2,205 FLACs would become prune candidates" — but that's the total FLAC count, not the actual data-dependent number
2. **Ambiguous language**: "Prefer stem files" — prefer them for what? Keeping locally? Making others deletable?
3. **No preview before toggling**: You can't see what will change without toggle + re-preview
4. **Global, static preference**: A single on/off is too coarse — what if you want to keep stems for one artist and FLACs for another?

**Replace with: Format relationship badges + prune preview filters**

Remove the global toggle and the `stem_preferred` config. Instead, move the logic into the prune preview table where the user can make informed per-file decisions.

#### Backend: `PruneCandidate` struct

Add a `has_stem_variant: bool` field:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneCandidate {
    pub file_id: i64,
    pub file_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub title: String,
    pub artist: String,
    pub isrc: Option<String>,
    pub reason: String,      // "not_followed" | "wav_backed_up"
    pub backup_path: Option<String>,
    pub has_stem_variant: bool,  // NEW: same-ISRC stem.m4a exists in DB
}
```

In `get_prune_candidates()`, compute `has_stem_variant` per candidate:

```rust
let has_stem: bool = if let Some(ref isrc) = isrc {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE isrc = ? AND file_type = 'stem.m4a' AND id != ?"
    )
    .bind(isrc)
    .bind(file_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    count > 0
} else {
    false
};
```

This is computed per-row (N+1), but prune previews are typically small (bounded to a few thousand). Alternatively, batch-fetch all ISRCs in one query for efficiency.

**Remove `stem_preferred`:**

- Remove the `stem_preferred` parameter from `get_prune_candidates()`
- Remove `GET/PUT /api/storage/settings` endpoint (or keep it for future settings)
- Remove the `StemPreference` toggle from Storage page
- The prune preview now returns ALL backed-up + not-followed files regardless of format

#### Frontend: Prune preview table

Each row shows:

```
☐  FLAC   Boris Brejcha - Black Unicorn   56 MB   [stem variant ✓]   not_followed
☐  WAV/vocals  ...                                 [stem variant ✓]   wav_backed_up
```

Filter toolbar above the table:

```
☐ Show only files with stem variant    ☐ Show only FLACs    ☐ Show only WAVs
[Select all with stem variant]  [Deselect all]
```

Bulk action buttons:

- "Delete Selected" (always visible)
- "Select all redundant" — selects all files where `has_stem_variant = true`

#### Why this is better

| Aspect              |      Current toggle      |            Proposed filter table            |
| ------------------- | :----------------------: | :-----------------------------------------: |
| See what changes    |         ❌ Blind         | ✅ Each file shows `has_stem_variant` badge |
| Control granularity |     ❌ Global on/off     |            ✅ Per-file checkbox             |
| Understand impact   | ❌ "Prefer" is ambiguous |      ✅ "Has stem variant" is factual       |
| Safety              |  ❌ Toggle + re-preview  |        ✅ Deselect individual files         |
| Adaptable           |       ❌ Hardcoded       |         ✅ Filter by any attribute          |

#### Files to modify

- `src/db.rs` — `PruneCandidate` struct + add `has_stem_variant`, remove `stem_preferred` from `get_prune_candidates()`, update `get_storage_status()`
- `src/api.rs` — `prune_preview_handler`/`prune_execute_handler` pass `false` (no more setting), optionally remove settings endpoint
- `frontend/pages/storage.js` — replace toggle with filter toolbar, add `has_stem_variant` column + bulk select, remove `renderStemPreference`
- `frontend/style.css` — filter toolbar styles

#### Acceptance Criteria

- [ ] `get_prune_candidates()` no longer takes `stem_preferred` parameter
- [ ] `PruneCandidate` includes `hasStemVariant` field
- [ ] FLACs with same-ISRC stem.m4a show `hasStemVariant: true`
- [ ] FLACs without same-ISRC stem.m4a show `hasStemVariant: false`
- [ ] WAVs with `source_of` show `hasStemVariant: true` (they ARE the stem variant)
- [ ] Prune preview table has filter: "Show only files with stem variant"
- [ ] "Select all with stem variant" bulk action works
- [ ] Old toggle removed, `stem_preferred` config key deprecated
- [ ] `cargo build` passes

### Part E: Pull-from-Backup Workflow

New API endpoint: `POST /api/files/{id}/pull-from-backup`

Copies a file that exists only on backup back to local disk — into the correct folder.

**Transparent path resolution**: The file goes back where it belongs:

- FLAC backup → local FLAC directory (`/Users/momo/Music/flacs/`)
- Stem backup → local stem directory (`/Users/momo/Music/stems/`)
- The local destination is determined by: (a) which folder the file originally belonged to, or (b) if it was never local, matching its remote path to the configured folder's `backup_path`.

**Configurable format preference**: When pulling a backup-only file, the user can specify which format they want via a query parameter `?prefer=stem` or `?prefer=flac`. If the preferred format isn't available, it falls back to whatever exists. This gives the user control — "download the FLAC version" vs "download the stem version".

```rust
async fn file_pull_from_backup_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(params): Query<PullFromBackupParams>,
) -> impl IntoResponse {
    // 1. Get the file record
    // 2. Check it has a backup location
    // 3. Determine local destination:
    //    - Find which folder this file path matches (flacs vs stems)
    //    - Construct local path: folder.folder_path + relative_path
    // 4. Rsync from backup (NAS) to local
    // 5. Update file_locations: add 'local' entry
    // 6. Update last_verified_local
    // 7. Return success + local path
}
```

This completes the cycle: backup → pull → scan → extract metadata → re-backup → delete.

### Files to modify

- `migrations/013_backup_discovery.sql` — new migration
- `src/db.rs` — `File` struct + `last_verified_local`, `TrackDetailFile` + `stem_type`, extend `get_track_detail()`, scanner lints `file_locations.local`, new `discover_backup_files()`, modified prune logic, `PruneCandidate` + `has_stem_variant`, remove `stem_preferred` param from `get_prune_candidates()`
- `src/api.rs` — extend `get_track_detail` response, `POST /api/storage/discover-backup/{folder_id}`, `POST /api/files/{id}/pull-from-backup`, route additions, remove/update `GET/PUT /api/storage/settings`
- `src/tasks/mod.rs` — new `BackupDiscovery` task type + worker
- `frontend/pages/track-detail.js` — WAV source variants section
- `frontend/pages/file-detail.js` — local presence indicator (variants section already exists)
- `frontend/pages/storage.js` — replace stem toggle with filter toolbar, backup discovery button, `has_stem_variant` column + bulk select
- `frontend/style.css` — filter toolbar styles, variant card styles (reuse from file-detail)

### Acceptance Criteria

**Part A:**

- [ ] `GET /api/tracks/{id}/detail` includes WAV source files (via `source_of`) in the `files` array
- [ ] WAV files have `stemType` field populated (null for non-WAV)
- [ ] Track-detail page shows WAV source variants grouped under their parent stem file
- [ ] Boris Brejcha - Black Unicorn (#1487) shows: 1 stem + 1 FLAC + 5 WAVs (grouped)
- [ ] `cargo build` passes

**Part B:**

- [ ] Migration 013 backfills `file_locations.local` for all existing files
- [ ] `scan_and_store_file()` creates/updates `file_locations.local` on scan
- [ ] `last_verified_local` updated on scan
- [ ] File detail shows local presence status
- [ ] `cargo build` passes

**Part C:**

- [ ] `POST /api/storage/discover-backup/{folder_id}` triggers background task
- [ ] Task lists NAS files, matches against DB, creates records for backup-only files
- [ ] Result shows: files_on_backup, already_tracked, newly_discovered, missing_from_backup
- [ ] Newly discovered files get `files` row + `file_locations.backup` entry
- [ ] `cargo build` passes

**Part D:**

- [ ] Prune candidates require: backed up + (metadata complete OR is linked WAV source) + not followed
- [ ] Files lacking BPM/key/comment are excluded from prune candidates (even if backed up)
- [ ] 603 unbacked-up FLACs not in prune (as before)
- [ ] `cargo build` passes

**Part E:**

- [ ] `POST /api/files/{id}/pull-from-backup` copies file from NAS to local
- [ ] Updates `file_locations.local` entry
- [ ] Updates `last_verified_local`
- [ ] Fails gracefully if no backup location exists
- [ ] `cargo build` passes

**Part F:**

- [ ] `get_prune_candidates()` no longer takes `stem_preferred` parameter
- [ ] `PruneCandidate` includes `hasStemVariant` field
- [ ] FLACs with same-ISRC stem.m4a show `hasStemVariant: true`
- [ ] FLACs without same-ISRC stem.m4a show `hasStemVariant: false`
- [ ] WAVs with `source_of` show `hasStemVariant: true` (they ARE the stem variant)
- [ ] Prune preview table has filter: "Show only files with stem variant"
- [ ] "Select all with stem variant" bulk action works
- [ ] Old toggle removed, `stem_preferred` config key deprecated
- [ ] All prune preview features work without a full page reload
- [ ] `cargo build` passes

---

