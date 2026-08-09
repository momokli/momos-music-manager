## Plan: staging-area-pull

**Status**: proposed
**Branch**: `feat/staging-area-pull` (branch from feat/fix-backpack-local-tracking)
**Depends on**: `relax-prune-safety-gates`
**Migration needed**: no

### Description

A "Staging Area" for files that need metadata extraction. Pull backed-up but
metadata-less files from NAS to local, let the user scan them with Traktor,
then auto-prune deletes them again once metadata is in the DB.

### Flow

```
1. Storage page shows "X files need metadata — on NAS only"
2. User clicks "Pull for scanning" → rsyncs files from NAS to local
3. User opens Traktor, runs BPM/Key detection on the folder
4. Folder watcher detects changed files → rescans → extracts metadata
5. Next maintainer cycle: auto-prune deletes them (now backed up + metadata complete)
```

### Backend

**New endpoint**: `POST /api/storage/stage-for-scan`

```json
// Request
{ "fileTypes": ["flac"], "limit": 100 }

// Response
{ "pulled": 73, "failed": 2, "totalCandidates": 533 }
```

Logic:

1. Query files that are: backed up, NOT local, have no metadata (bpm=null AND comment=null)
2. For each: resolve backup host, rsync from NAS to local
3. Create `file_locations.local` + update `last_verified_local`
4. Return counts

**New DB function**: `src/db/files.rs` — `get_staging_pull_candidates()`

```rust
/// Files that are on backup but not local, and need metadata extraction.
pub async fn get_staging_pull_candidates(
    pool: &Pool<Sqlite>,
    file_types: Option<Vec<String>>,
    limit: Option<i64>,
) -> Result<Vec<PullCandidate>>
```

Query:

```sql
SELECT f.*, fl.path as backup_path
FROM files f
JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'backup'
WHERE f.bpm IS NULL AND (f.comment IS NULL OR f.comment = '')
  AND f.id NOT IN (SELECT file_id FROM file_locations WHERE location_type = 'local')
  AND (? IS NULL OR f.file_type IN (...))
ORDER BY f.file_type, f.file_path
LIMIT ?
```

### Frontend

Add a "Metadata Gap" card to the Storage page:

```
┌──────────────────────────────────────────────┐
│ METADATA GAP                                 │
│                                              │
│ 533 files on backup need metadata extraction │
│ (no BPM or comment)                          │
│                                              │
│ File types: [FLAC ▾]  Limit: [100 ▾]        │
│ [Pull for Scanning]                          │
│                                              │
│ Pulled 73/100 · 2 failed                     │
│                                              │
│ After scanning, the maintainer will auto-    │
│ prune them back to backup-only.              │
└──────────────────────────────────────────────┘
```

### Agent Decomposition (TDD)

Two agents, disjoint files:

**Agent A: Backend** (`src/db/files.rs` + `src/api/storage.rs` + `tests/api_storage.rs`)

Step 1 — Write failing tests:

- `test_staging_pull_candidates_no_metadata` (unit) — returns files with no bpm/comment, excludes files with metadata, excludes files already local
- `storage_stage_for_scan` (integration) — POST returns pulled/failed counts

Step 2 — Implement:

- `get_staging_pull_candidates()` in `src/db/files.rs`
- `stage_for_scan_handler` in `src/api/storage.rs`
- Route: `POST /api/storage/stage-for-scan`
- Reuse `resolve_backup_host()` and `BackupEngine::pull_file()`

**Agent B: Frontend** (`frontend/pages/storage.js` + `frontend/style.css`)

- Add "Metadata Gap" card to Storage page
- File type dropdown + limit input + "Pull for Scanning" button
- Shows results (pulled/failed counts)
- Explains the flow (scan → auto-prune)

### Acceptance Criteria

- [ ] `POST /api/storage/stage-for-scan` pulls files from NAS
- [ ] Only pulls files without metadata (no bpm, no comment)
- [ ] Skips files already local
- [ ] Respects file type filter and limit
- [ ] Storage page shows Metadata Gap card with counts
- [ ] Pull button works, shows results
- [ ] `cargo build` + `cargo test` pass

---

