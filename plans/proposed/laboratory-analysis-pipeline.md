## Plan: laboratory-analysis-pipeline

**Status**: proposed
**Branch**: `feat/laboratory-analysis`
**Ready for review**: no
**Depends on**: `feat/traktor-basename-matching` (already on main)
**Migration needed**: no

### Description

A pipeline for progressively analyzing files on the LAN server with Traktor
on the MacBook. Files tagged "laboratory" (or any tag) are pulled from the
LAN to the MacBook, Traktor analyzes them, the NML syncs metadata back to
the LAN via the existing basename-matching import. Afterwards files are
cleaned from the MacBook.

This completes the architecture shift: **LAN = source of truth, NAS = mirror,
MacBook = on-demand client** for both playback (backpack) and analysis
(laboratory pack).

### Flow

```
1. API returns files in a tag that need BPM/key and are local on LAN
2. MacBook script rsyncs those files from LAN → MacBook
3. User opens/closes Traktor → collection.nml updated
4. Cron rsyncs NML to LAN → basename matching imports metadata
5. Script cleans downloaded files from MacBook
```

### Current State

| Layer                       | Status                                                      |
| --------------------------- | ----------------------------------------------------------- |
| NML basename matching       | ✅ Works — 2,920 files matched from MacBook import          |
| NML cron sync (15 min)      | ✅ MacBook → LAN                                            |
| Maintainer auto-import      | ✅ Enabled, hourly cycle                                    |
| LAN file inventory          | ⬜ Only 223/13,402 files on local disk — need NAS→LAN rsync |
| API to query needs-analysis | ⬜ Not built yet                                            |

### Part A: API Endpoint

#### `GET /api/tags/{id}/needs-analysis`

Returns files in the given tag that need Traktor analysis and are available
on the LAN's local disk.

**Query params**: `format` (optional, e.g. `flac` or `stem.m4a`)

**Response**:

```json
{
  "data": {
    "tagId": 42,
    "tagName": "laboratory",
    "fileCount": 500,
    "needsBpm": 350,
    "needsKey": 200,
    "needsBoth": 150,
    "files": [
      {
        "fileId": 123,
        "filePath": "/home/momo/share/flacs/Artist - Title.flac",
        "fileType": "flac",
        "localSize": 32456789,
        "title": "Artist - Title",
        "artist": "Artist",
        "needsBpm": true,
        "needsKey": false,
        "backedUp": true
      }
    ]
  }
}
```

**SQL**: Files must be:

- In the tag (via `file_resolved_tags.tag_name`)
- Missing BPM OR missing key (`bpm IS NULL OR musical_key IS NULL`)
- Present on LAN disk (`EXISTS file_locations WHERE location_type='local'`)
- Optional: filtered by file_type

```sql
SELECT f.id, f.file_path, f.file_type, f.file_size,
       f.title, f.artist,
       f.bpm IS NULL OR f.bpm = 0 AS needs_bpm,
       f.musical_key IS NULL AS needs_key,
       (fl_backup.id IS NOT NULL) AS backed_up
FROM files f
JOIN file_resolved_tags frt ON frt.file_id = f.id AND frt.tag_name = (
    SELECT name FROM tags WHERE id = ?
)
JOIN file_locations fl_local ON fl_local.file_id = f.id AND fl_local.location_type = 'local'
LEFT JOIN file_locations fl_backup ON fl_backup.file_id = f.id AND fl_backup.location_type = 'backup'
WHERE (f.bpm IS NULL OR f.musical_key IS NULL)
  AND (? IS NULL OR f.file_type = ?)
ORDER BY f.artist, f.title
```

#### Backend: `src/api/tags.rs`

New struct + handler:

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NeedsAnalysisResponse {
    tag_id: i64,
    tag_name: String,
    file_count: usize,
    needs_bpm: usize,
    needs_key: usize,
    needs_both: usize,
    files: Vec<NeedsAnalysisFile>,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct NeedsAnalysisFile {
    file_id: i64,
    file_path: String,
    file_type: String,
    local_size: i64,
    title: Option<String>,
    artist: Option<String>,
    #[sqlx(rename = "needs_bpm")]
    needs_bpm: bool,
    #[sqlx(rename = "needs_key")]
    needs_key: bool,
    #[sqlx(rename = "backed_up")]
    backed_up: i32,  // SQLite doesn't have bool for query_as
}
```

Route:

```rust
.route("/api/tags/{id}/needs-analysis", get(tag_needs_analysis_handler))
```

#### Error states

| Case                   | Status | Body                                                 |
| ---------------------- | ------ | ---------------------------------------------------- |
| Tag not found          | 404    | `{"error": "Tag not found"}`                         |
| No files need analysis | 200    | Empty `files` array                                  |
| No files are local     | 200    | Empty `files` array (tag exists but all on NAS only) |

### Part B: MacBook Script

#### `scripts/lab-stage.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail

TAG="${1:-laboratory}"
FORMAT="${2:-flac}"
LAN="lan"
LAN_MUSIC="/home/momo/share/${FORMAT}s"  # flacs or stems
LOCAL_MUSIC="$HOME/Music/${FORMAT}s"
API="http://localhost:3000/api/tags/by-name/${TAG}/needs-analysis?format=${FORMAT}"

echo "→ Fetching files needing analysis in tag '${TAG}'..."
RESP=$(ssh "$LAN" "curl -s '$API'")
COUNT=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['fileCount'])")

if [ "$COUNT" -eq 0 ]; then
    echo "✓ All files in '${TAG}' are already analyzed. Nothing to do."
    exit 0
fi

echo "→ Pulling ${COUNT} ${FORMAT}(s) from ${LAN}..."
# Get just the relative filenames and rsync them
FILES=$(echo "$RESP" | python3 -c "
import sys, json
for f in json.load(sys.stdin)['data']['files']:
    print(f['filePath'].split('/')[-1])
")

echo "$FILES" | while read -r f; do
    rsync -avz "${LAN}:${LAN_MUSIC}/${f}" "${LOCAL_MUSIC}/" 2>&1 | tail -1
done

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  ✓ ${COUNT} files staged for analysis"
echo "  1. Open Traktor — it will detect ${COUNT} new files"
echo "  2. Wait for waveform/BPM analysis to finish"
echo "  3. Close Traktor — collection.nml is saved"
echo ""
echo "  The NML will sync to LAN automatically within 15 min."
echo "  Press ENTER after closing Traktor to clean up..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
read -r

echo "→ Cleaning up downloaded files..."
echo "$FILES" | while read -r f; do
    rm -f "${LOCAL_MUSIC}/${f}"
done

echo "✓ Cleanup complete. Metadata will appear on LAN after next NML sync."
```

### Files to modify

| File                   | Change                                                                                                     |
| ---------------------- | ---------------------------------------------------------------------------------------------------------- |
| `src/api/tags.rs`      | Add `tag_needs_analysis_handler` + route + `NeedsAnalysisResponse`/`NeedsAnalysisFile` structs (~80 lines) |
| `tests/api_tags.rs`    | 5 integration tests (~120 lines)                                                                           |
| `scripts/lab-stage.sh` | New script (~50 lines)                                                                                     |

### TDD: Tests (written FIRST, fail before implementation)

#### Integration tests — `tests/api_tags.rs`:

All tests use `seed_basic_data` + `refresh_file_resolved_tags()`.

`seed_basic_data` provides files:

- id=1: flac, BPM=128, key=4m, local+backup (FULLY analyzed)
- id=2: stem.m4a, BPM=128.5, key=4m, local+backup (FULLY analyzed)
- id=3: flac, BPM=140, key=8m, backup only (FULLY analyzed, NOT local)
- id=4: flac, BPM=NULL, key=NULL, backup only (NEEDS analysis, NOT local — won't appear)

File 4 has no BPM/key but also no local file_locations entry — it won't appear in results
because it's not local. We need an additional seed for tests that require a local file
needing analysis.

**New seed helper** — `tests/common/mod.rs`:

```rust
pub async fn seed_lab_scenario(pool: &Pool<Sqlite>) {
    seed_basic_data(pool).await;

    // File 5: needs analysis (no BPM, no key), IS local, IS backed up
    // Linked to a new tag "Laboratory" via playlist matching
    sqlx::query(
        "INSERT OR IGNORE INTO files (id, file_path, file_type, file_size, last_modified, title, artist, isrc, file_hash)
         VALUES (5, '/test/stems/Needs - Analysis.flac', 'flac', 5000000, 1700000000, 'Needy Track', 'Test Artist', 'US005', 'hash5')"
    ).execute(pool).await.unwrap();

    sqlx::query(
        "INSERT OR IGNORE INTO file_locations (file_id, location_type, path, file_size, last_verified)
         VALUES (5, 'local', '/test/stems/Needs - Analysis.flac', 5000000, 1700000000)"
    ).execute(pool).await.unwrap();

    // Tag with matching playlist
    sqlx::query(
        "INSERT OR IGNORE INTO tags (id, name, category_id) VALUES (20, 'Laboratory', 1)"
    ).execute(pool).await.unwrap();

    sqlx::query(
        "INSERT OR IGNORE INTO service_playlists (id, service, playlist_id, name, snapshot_id)
         VALUES (4, 'spotify', 'spotify:playlist:444', 'Laboratory', 'snap4')"
    ).execute(pool).await.unwrap();

    // Link track to playlist (via file 5's spotify_id)
    sqlx::query("UPDATE files SET spotify_id = 'spotify:track:eee' WHERE id = 5")
        .execute(pool).await.unwrap();

    sqlx::query(
        "INSERT OR IGNORE INTO service_tracks (id, service, service_id, title, artist, isrc, imported_at)
         VALUES (5, 'spotify', 'spotify:track:eee', 'Needy Track', 'Test Artist', 'US005', 1700000000)"
    ).execute(pool).await.unwrap();

    sqlx::query(
        "INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, position, added_at)
         VALUES (4, 5, 0, 1700000000)"
    ).execute(pool).await.unwrap();

    refresh_file_resolved_tags(pool).await.unwrap();
}
```

| #   | Test name                                       | What it proves                                                         |
| --- | ----------------------------------------------- | ---------------------------------------------------------------------- |
| 1   | `tags_needs_analysis_returns_files_needing_bpm` | File 5 (no BPM, no key) appears. File 1 (has BPM+key) does not.        |
| 2   | `tags_needs_analysis_excludes_fully_analyzed`   | Files with both BPM and key are excluded                               |
| 3   | `tags_needs_analysis_excludes_non_local`        | File 4 (no BPM, no key, backup only) excluded because not local        |
| 4   | `tags_needs_analysis_tag_not_found`             | `/api/tags/9999/needs-analysis` returns 404                            |
| 5   | `tags_needs_analysis_filter_by_format`          | `?format=stem.m4a` returns only stems                                  |
| 6   | `tags_needs_analysis_counts_are_correct`        | `needsBpm`, `needsKey`, `needsBoth`, `fileCount` match the files array |

### Acceptance Criteria

**API:**

- [ ] `GET /api/tags/{id}/needs-analysis` returns files in tag needing BPM/key
- [ ] Only files with `file_locations.local` appear (must be on LAN disk)
- [ ] Files with both BPM and key are excluded
- [ ] `?format=flac` filters by file type
- [ ] Tag not found → 404 with error message
- [ ] Empty tag (no files) → 200 with empty files array, counts=0
- [ ] Counts match: `needsBpm` + `needsKey` - `needsBoth` = total files with any need
- [ ] `backedUp` field accurately reflects `file_locations.backup` existence

**Script:**

- [ ] `scripts/lab-stage.sh laboratory flac` pulls FLACs from LAN to MacBook
- [ ] Prompts user to open/close Traktor
- [ ] Cleans downloaded files after user confirmation
- [ ] Works when no files need analysis (exits cleanly)

**Tests:**

- [ ] 6 integration tests pass (`cargo test --test api_tags`)
- [ ] All existing tests still pass
- [ ] `cargo build` passes

### Out of scope

- Pulling from LAN to MacBook via the web UI (script-based for now)
- Automatic detection of "analyzed" status (NML sync handles this)
- File cleanup on LAN side after analysis (files stay — they're the source of truth)
- Stems analysis (FLACs first, stems have same BPM/key as corresponding FLAC)

### Agent Decomposition (2 agents, zero file conflicts)

| Agent | Files                                                         | Work                                                     | Tests               |
| ----- | ------------------------------------------------------------- | -------------------------------------------------------- | ------------------- |
| **A** | `src/api/tags.rs`, `tests/api_tags.rs`, `tests/common/mod.rs` | API endpoint + handler + integration tests + seed helper | ~6 integration      |
| **B** | `scripts/lab-stage.sh`                                        | MacBook pull+clean script                                | Manual verification |

All 2 agents can run in parallel — zero file conflicts.

---

