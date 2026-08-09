## Plan: wav-source-linking

**Status**: proposed
**Branch**: `feat/wav-source-linking`
**Ready for review**: no
**Depends on**: `feat/file-lifecycle-management` (already merged)
**Migration needed**: yes — `012_wav_stem_type.sql`

### Description

Three-phase plan to properly handle nuo-stems WAV source files: link them to parent stems via `source_of`, track which stem part each WAV is (`stem_type`), make backed-up+linked WAVs prunable, and enrich track metadata with file variant information.

### Investigation Results (2026-05-28)

| File Type | Count |   Size | Backed Up | source_of set | stem_type tracked |
| --------- | ----: | -----: | :-------: | :-----------: | :---------------: |
| wav       | 6,647 | 277 GB | 6,647 ✅  |     0 ❌      |        ❌         |
| stem.m4a  | 1,770 |  90 GB |   1,770   |      N/A      |        N/A        |
| flac      | 2,205 |  64 GB |   1,602   |      N/A      |        N/A        |

**File system layout:**

```
/Users/momo/Music/stems/
├── WILL FERRO - Dreams.stem.m4a          ← stem file (top-level)
├── WILL_FERRO_Dreams/                    ← WAV source subdir (1,330 of these)
│   ├── WILL FERRO - Dreams_vocals.wav
│   ├── WILL FERRO - Dreams_bass.wav
│   ├── WILL FERRO - Dreams_drums.wav
│   ├── WILL FERRO - Dreams_instrumental.wav
│   └── WILL FERRO - Dreams_other.wav
```

**Naming convention discovered:** WAV files follow `{stem_name}_{stem_type}.wav` where `stem_type ∈ {vocals, bass, drums, instrumental, other}`. The stem file is `{stem_name}.stem.m4a` in the parent directory. This is reliably parseable — the stem*type is always the text after the LAST `*`before`.wav`, if it matches the known set.

**What's broken:**

1. `ScanWavSources` worker counts WAVs but never calls `set_file_source_of()` — `linked_to_stems` is declared `0usize` and never incremented (see `src/tasks/mod.rs` lines ~2520-2560)
2. `BackupWavs` worker passes `file_id=0` to `record_backup_result()` (line 2329), so it can't link backup records to the right files. (Current backup records came from the regular `BackupFolder` task, which passes correct `file.id`.)
3. `get_prune_candidates()` explicitly excludes WAVs with `f.file_type != 'wav'` — even backed-up + linked WAVs can't be pruned
4. No `stem_type` column exists — we know a WAV is a source file but not which part it is
5. No track enrichment — no way to see "this track has FLAC + stem + 5 WAV source files"

### Phase 1: Add stem_type + Populate source_of Linking

#### Migration 012 (`migrations/012_wav_stem_type.sql`)

```sql
-- Add stem_type column for tracking which nuo-stems part a WAV source file represents
ALTER TABLE files ADD COLUMN stem_type TEXT CHECK (
    stem_type IS NULL OR stem_type IN ('vocals', 'bass', 'drums', 'instrumental', 'other')
);

CREATE INDEX IF NOT EXISTS idx_files_stem_type ON files(stem_type);

SELECT 'Migration 012 applied: stem_type column on files' as status;
```

Rationale for dedicated column over `metadata_json`:

- `files` already uses dedicated columns for audio metadata (title, artist, genre, bpm, musical_key, etc.) — this fits the pattern
- CHECK constraint ensures data integrity at DB level
- Directly queryable: `SELECT * FROM files WHERE stem_type = 'vocals'`
- No JSON parsing overhead
- Self-documenting schema

#### Rust: `src/db.rs` — `File` struct

Add `stem_type: Option<String>` field to `File` struct (after `source_of`):

```rust
// Source WAV linking (WAV source subdirectory → stem file)
pub source_of: Option<i64>,

// Stem type for WAV source files (vocals, bass, drums, instrumental, other)
pub stem_type: Option<String>,
```

Update both `extract_minimal_file_metadata` and `extract_audio_metadata_from_file` to set `stem_type: None`.

#### Rust: `src/db.rs` — Preserve `source_of` and `stem_type` during re-scan

**Critical:** `scan_and_store_file()` (line 762) does INSERT + ON CONFLICT UPDATE without including `source_of` in either clause. If a WAV file is re-scanned (by folder watcher or manual scan), the linkage established by `ScanWavSources` would be silently lost. Same applies to the new `stem_type`.

Fix: add both columns to INSERT and use COALESCE in ON CONFLICT UPDATE to preserve existing values:

```rust
// In the INSERT column list, add:
source_of, stem_type,

// In VALUES, add two more bindings:
.bind(&file.source_of)
.bind(&file.stem_type)

// In ON CONFLICT DO UPDATE SET, add:
source_of = COALESCE(excluded.source_of, files.source_of),
stem_type = COALESCE(excluded.stem_type, files.stem_type),
```

Using COALESCE ensures: on first insert, values come from the file struct (NULL for both); on re-scan, the previously-set `source_of` and `stem_type` are preserved because the incoming values from `extract_audio_metadata_from_file` are NULL.

#### Rust: `src/db.rs` — `get_file_by_path()`

Note: `get_file_by_path()` already exists at `src/db.rs` line 1016 — no need to create it. Reuse the existing function.

#### Rust: `src/db.rs` — WAV→stem matching

New function `link_wav_to_stem()`:

```rust
/// Parse a WAV filename and link it to its parent stem file.
///
/// Pattern: `{stem_name}_{stem_type}.wav` where stem_type ∈ {vocals,bass,drums,instrumental,other}
/// The stem file is `{stem_name}.stem.m4a` in the parent of the parent directory.
///
/// Returns Some(file_id of stem) on success, None if no matching stem found.
pub async fn link_wav_to_stem(
    pool: &Pool<Sqlite>,
    wav_file_id: i64,
    wav_file_path: &str,
) -> Result<Option<(i64, String)>> {
    let path = std::path::Path::new(wav_file_path);
    let filename = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    // Known stem types in nuo-stems
    const STEM_TYPES: &[&str] = &["vocals", "bass", "drums", "instrumental", "other"];

    // Extract stem_type: text after last '_' before '.wav'
    let stem_name_no_ext = filename.strip_suffix(".wav").unwrap_or(filename);
    let (stem_name, stem_type) = if let Some(last_underscore) = stem_name_no_ext.rfind('_') {
        let candidate = &stem_name_no_ext[last_underscore + 1..];
        if STEM_TYPES.contains(&candidate) {
            (&stem_name_no_ext[..last_underscore], candidate.to_string())
        } else {
            // Unknown suffix — not a stem part WAV
            return Ok(None);
        }
    } else {
        // No underscore — not a stem part WAV
        return Ok(None);
    };

    // The stem file is in the parent of the parent directory:
    // /stems/ARTIST_Title/Artist - Title_vocals.wav
    //   → parent = /stems/ARTIST_Title
    //   → parent's parent = /stems
    //   → stem = /stems/Artist - Title.stem.m4a
    let parent = path.parent();  // /stems/ARTIST_Title
    let stems_root = parent.and_then(|p| p.parent());  // /stems

    let expected_stem_path = if let Some(root) = stems_root {
        format!("{}/{}.stem.m4a", root.display(), stem_name)
    } else {
        return Ok(None);
    };

    // Look up the stem file
    let stem = sqlx::query_as::<_, File>(
        "SELECT * FROM files WHERE file_path = ? AND file_type = 'stem.m4a'"
    )
    .bind(&expected_stem_path)
    .fetch_optional(pool)
    .await?;

    match stem {
        Some(s) => {
            // Link: set source_of and stem_type
            sqlx::query("UPDATE files SET source_of = ?, stem_type = ? WHERE id = ?")
                .bind(s.id)
                .bind(&stem_type)
                .bind(wav_file_id)
                .execute(pool)
                .await?;
            Ok(Some((s.id, stem_type)))
        }
        None => Ok(None),
    }
}
```

Key design decisions in this algorithm:

- Uses the LAST `_` before `.wav` to find stem*type — works for titles containing `*`(e.g.,`Artist\_-_Title_vocals.wav`)
- Checks against known stem_type values — silently skips unknown suffixes
- Uses `file_path = ?` exact match lookup — indexed, fast
- Stem is in parent-of-parent directory — derived from WAV path structure, no need to guess directory naming

#### Rust: `src/tasks/mod.rs` — Fix `ScanWavSources` worker

Replace the stub counting loop (~lines 2550-2600) with actual linking:

```rust
let mut wav_indexed = 0usize;
let mut linked_to_stems = 0usize;  // was: const 0usize

for (i, subdir_name) in subdirs.iter().enumerate() {
    // ... cancel check ...

    let local_subdir = format!("{}/{}", local_dir.trim_end_matches('/'), subdir_name);
    let dir_path = std::path::Path::new(&local_subdir);

    if !dir_path.is_dir() { continue; }

    if let Ok(entries) = std::fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.extension().and_then(|e| e.to_str()) != Some("wav") {
                continue;
            }
            wav_indexed += 1;

            // Look up the WAV file in DB by path
            let wav_path_str = entry_path.to_string_lossy().to_string();
            if let Ok(Some(wav_file)) = crate::db::get_file_by_path(&db_clone, &wav_path_str).await {
                match crate::db::link_wav_to_stem(&db_clone, wav_file.id, &wav_path_str).await {
                    Ok(Some((stem_id, stem_type))) => {
                        linked_to_stems += 1;
                        tracing::debug!(
                            "Linked WAV {} (type={}) → stem #{}",
                            wav_path_str, stem_type, stem_id
                        );
                    }
                    Ok(None) => {
                        tracing::debug!("No matching stem for WAV: {}", wav_path_str);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to link WAV {}: {}", wav_path_str, e);
                    }
                }
            }
        }
    }
    // ... progress update ...
}

let msg = format!(
    "WAV source scan complete: {} WAV files indexed, {} linked to stems in {} subdirectories",
    wav_indexed, linked_to_stems, subdirs.len()
);
```

#### Rust: `src/tasks/mod.rs` — Fix `BackupWavs` worker

Replace `crate::db::record_backup_result(&db_clone, 0, true, file_size, &remote_wav_path)` at line 2329 with a proper file lookup:

```rust
// Look up the WAV file in DB by local path to get correct file_id
let local_wav_path = entry_path.to_string_lossy().to_string();
let file_id = if let Ok(Some(f)) = crate::db::get_file_by_path(&db_clone, &local_wav_path).await {
    f.id
} else {
    continue;  // skip files not in DB
};
let _ = crate::db::record_backup_result(
    &db_clone,
    file_id,
    true,
    file_size,
    &remote_wav_path,
)
.await;
```

### Phase 2: Allow Pruning of Backed-up + Linked WAVs

#### Rust: `src/db.rs` — Modify `get_prune_candidates()`

**Remove** the `f.file_type != 'wav'` exclusion. Replace with conditional logic:

- For **non-WAV** files: same logic as before (backed up + not followed → candidate)
- For **WAV** files: backed up + `source_of IS NOT NULL` + not followed → candidate with `reason = "wav_backed_up"`

Change the initial fetch from:

```sql
WHERE fl.location_type = 'backup' AND f.file_type != 'wav'
```

to:

```sql
WHERE fl.location_type = 'backup'
  AND (f.file_type != 'wav' OR (f.file_type = 'wav' AND f.source_of IS NOT NULL))
```

This ensures:

- WAVs without `source_of` (not yet linked) are NOT prune candidates — we need the metadata first
- WAVs with `source_of` that are backed up → eligible for pruning
- Non-WAV files: behavior unchanged

In the reason assignment, add:

```rust
let reason = if row.file_type == "wav" {
    "wav_backed_up".to_string()
} else {
    "not_followed".to_string()
};
```

Also add `reason` to the SQL SELECT, importing the value from the file_type:

```sql
SELECT f.id, f.file_path, f.file_type, f.file_size, ...
```

Then in Rust, assign reason based on file_type.

### Phase 3: Track Enrichment API

#### API: `GET /api/files/{id}/variants`

Returns all file variants for a track, grouped by common identity. Groups files by:

- Same ISRC (most reliable)
- Same `source_of` parent (WAVs belonging to same stem)

Response:

```json
{
  "fileId": 4362,
  "title": "Games People Play",
  "artist": "Paula van Klar",
  "isrc": "US7NS2500009",
  "variants": [
    {
      "id": 4362,
      "fileType": "stem.m4a",
      "filePath": "...",
      "fileSize": 12345,
      "backedUp": true
    },
    {
      "id": 4042,
      "fileType": "stem.m4a",
      "filePath": "...",
      "fileSize": 12345,
      "backedUp": true
    },
    {
      "id": 9801,
      "fileType": "flac",
      "filePath": "...",
      "fileSize": 45678,
      "backedUp": true
    },
    {
      "id": 9802,
      "fileType": "wav",
      "stemType": "vocals",
      "filePath": "...",
      "fileSize": 89012,
      "backedUp": true
    },
    {
      "id": 9803,
      "fileType": "wav",
      "stemType": "bass",
      "filePath": "...",
      "fileSize": 89012,
      "backedUp": true
    }
  ]
}
```

Implementation in `src/db.rs`:

```rust
pub async fn get_file_variants(pool: &Pool<Sqlite>, file_id: i64) -> Result<FileVariants> {
    // First, get the file to find its ISRC
    let file = get_file_by_id(pool, file_id).await?.ok_or_else(|| anyhow!("File not found"))?;

    // Find all files with same ISRC (if ISRC is not null)
    let mut variant_ids = std::collections::HashSet::new();
    variant_ids.insert(file.id);

    if let Some(ref isrc) = file.isrc {
        let same_isrc: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM files WHERE isrc = ? AND id != ?"
        )
        .bind(isrc)
        .bind(file.id)
        .fetch_all(pool)
        .await?;
        variant_ids.extend(same_isrc);
    }

    // Also include WAV source files (source_of points to this stem)
    let wav_sources: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM files WHERE source_of = ? AND file_type = 'wav'"
    )
    .bind(file.id)
    .fetch_all(pool)
    .await?;
    variant_ids.extend(wav_sources);

    // If this file is a WAV, include its stem parent and siblings
    if let Some(source_of) = file.source_of {
        variant_ids.insert(source_of);
        let sibling_wavs: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM files WHERE source_of = ? AND id != ?"
        )
        .bind(source_of)
        .bind(file.id)
        .fetch_all(pool)
        .await?;
        variant_ids.extend(sibling_wavs);
    }

    // Fetch full details for all variants
    let placeholders: Vec<String> = variant_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT f.id, f.file_path, f.file_type, f.file_size, f.stem_type,
                CASE WHEN fl.id IS NOT NULL THEN 1 ELSE 0 END as backed_up
         FROM files f
         LEFT JOIN file_locations fl ON fl.file_id = f.id AND fl.location_type = 'backup'
         WHERE f.id IN ({})
         ORDER BY f.file_type, f.stem_type",
        placeholders.join(",")
    );
    // ... bind and fetch ...
}
```

#### Route

```rust
.route("/api/files/{id}/variants", get(file_variants_handler))
```

#### Frontend: File Detail page (`frontend/pages/file-detail.js`)

Add a "Variants" section below the metadata, showing:

- List of all file variants with type badges (stem, flac, wav-vocals, wav-bass, etc.)
- Backup status per variant (✓ backed up / ✗ local only)
- File size per variant

### Files to modify

- `migrations/012_wav_stem_type.sql` — new migration
- `src/db.rs` — `File` struct + `stem_type`, `link_wav_to_stem()`, `get_file_by_path()`, `get_file_variants()`, update `get_prune_candidates()`
- `src/tasks/mod.rs` — fix `ScanWavSources` worker (actual linking), fix `BackupWavs` worker (correct file_id)
- `src/api.rs` — add `GET /api/files/{id}/variants` route + handler
- `frontend/pages/file-detail.js` — variants section
- `frontend/style.css` — variant badge styles

### Acceptance Criteria

**Phase 1:**

- [ ] Migration 012 runs cleanly on fresh DB (001→012)
- [ ] Migration 012 runs cleanly on existing DB with data
- [ ] `stem_type` column added with CHECK constraint
- [ ] `link_wav_to_stem()` correctly parses: `WILL FERRO - Dreams_vocals.wav` → stem_type=`vocals`, links to `WILL FERRO - Dreams.stem.m4a`
- [ ] `link_wav_to_stem()` handles edge cases: unknown suffix → skips, no underscore → skips, no matching stem → skips
- [ ] `link_wav_to_stem()` handles titles with `_` (e.g., `Artist_-_Title_vocals.wav`)
- [ ] `link_wav_to_stem()` handles names with parentheses (e.g., `Jon.K - Madness (Malandra Jr. Remix)_bass.wav`)
- [ ] `ScanWavSources` task populates `source_of` and `stem_type` for WAVs with matching stem files (~81% of 6,647 = ~5,405 linked; remaining ~1,242 skipped gracefully)
- [ ] `ScanWavSources` task logs counts: WAVs indexed, linked to stems, skipped
- [ ] `BackupWavs` task uses correct file_id in `record_backup_result()`
- [ ] `scan_and_store_file()` preserves existing `source_of` and `stem_type` on re-scan (COALESCE)
- [ ] `cargo build` passes

**Phase 2:**

- [ ] Backed-up WAVs with `source_of IS NOT NULL` appear as prune candidates with `reason = "wav_backed_up"`
- [ ] Backed-up WAVs without `source_of` are NOT prune candidates (not yet linked)
- [ ] Non-WAV prune behavior unchanged
- [ ] `cargo build` passes

**Phase 3:**

- [ ] `GET /api/files/{id}/variants` returns all variants grouped by ISRC + source_of
- [ ] Response includes `fileType`, `stemType` (for WAVs), `fileSize`, `backedUp`
- [ ] File detail page shows variants section
- [ ] `cargo build` passes

### One-time operation after deploy

After Phase 1 is deployed, run the `ScanWavSources` task on folder #1 (stems) to populate `source_of` and `stem_type` for all 6,647 existing WAVs. This is a one-time batch — future scans via the folder watcher will pick up new WAVs incrementally.

---

