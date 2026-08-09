## Plan: traktor-basename-matching

**Status**: proposed
**Branch**: `feat/traktor-basename-matching`
**Ready for review**: no
**Depends on**: `feat/purge-ghost-records` (folder_id on files)
**Migration needed**: no

### Description

Fix Traktor import to match files by **filename basename** instead of full
absolute path. Currently `import_traktor_metadata()` reconstructs the
absolute path from `<LOCATION DIR="..." FILE="..."/>`, normalizes it, and
looks it up in a `HashMap<full_path, file_id>`. This breaks when the DB
is imported from one machine to another with different mount points
(e.g. MacBook `/Users/momo/Music/flacs/` → LAN `/home/momo/share/flacs/`).

After this change, matching uses only the `FILE` attribute's basename.
Since all 13,402 files on the LAN have **unique basenames** (verified via
`COUNT(DISTINCT basename) = COUNT(*)`), there are zero collisions.

### Current State

**Matching**: NML absolute path → normalized → HashMap lookup → full path match.

```rust
// Current: builds map[normalized_full_path] = file_id
let path_map: HashMap<String, (i64, String)> = rows
    .into_iter()
    .map(|(id, path)| (normalize_path(&path), (id, path)))
    .collect();
```

**Problem**: NML from MacBook has `DIR="/:Users/:momo/:Music/:flacs/:"` but
LAN file paths are `/home/momo/share/flacs/...`. Normalized full paths differ
→ zero matches.

### Fix: Basename matching

Replace the full-path HashMap with a basename → file_id map:

```rust
// New: builds map[basename] = file_id
let basename_map: HashMap<String, i64> = rows
    .into_iter()
    .map(|(id, path)| {
        let basename = std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        (basename.to_lowercase(), id)
    })
    .collect();
```

Then during the entry loop, instead of reconstructing the full NML path:

```rust
// Before (current):
let abs_path = traktor_path_to_abs(&entry.dir, &entry.file);
let abs_str = abs_path.to_string_lossy();
let normalized = normalize_path(&abs_str);
if let Some((file_id, _)) = path_map.get(&normalized) { ... }

// After (new):
let basename = entry.file.to_lowercase();
if let Some(file_id) = basename_map.get(&basename) { ... }
```

**Collision resolution**: If two files share a basename (none do currently,
but defensively), the later one overwrites the HashMap entry. Since
`rows` comes from `SELECT id, file_path FROM files`, the ordering is
undefined. A future version could sort by format preference
(stem.m4a > flac > wav > mp3), but this isn't needed now — all
basenames on the LAN are unique.

### What stays the same

- **Traktor entry format**: `DIR` and `FILE` attributes parsed as before
- **Update SQL**: `UPDATE files SET play_count=COALESCE(...), bpm=COALESCE(...)` unchanged
- **COALESCE behavior**: Existing data is preserved, only gaps are filled
- **`traktor_path_to_abs()`**: Kept for the rare case where a collection entry has
  a path that DOES match a local file's full path (fast path that still works)
- **Batch update**: Same chunking pattern (100 per batch)

### Files to modify

| File             | Change                                                                                                    |
| ---------------- | --------------------------------------------------------------------------------------------------------- |
| `src/traktor.rs` | Replace `path_map` with `basename_map` in `import_traktor_metadata()` (~20 lines changed, lines ~327-375) |

### Acceptance Criteria

- [ ] NML entry with `FILE="ANNA - Surrender.flac"` matches a LAN file `/home/momo/share/flacs/ANNA - Surrender.flac` regardless of the NML's `DIR` attribute
- [ ] NML entry with mismatched `DIR` and `FILE` still matches if basename is unique
- [ ] When no file shares the basename, the entry is skipped (counted as unmatched)
- [ ] Existing COALESCE behavior preserved: re-import with NULL values doesn't overwrite
- [ ] BPM, key, rating, play_count, last_played all populated from NML basename matches
- [ ] Import stats report correct `matched` count (should ~match file count on LAN)
- [ ] All 8 existing `traktor.rs` tests pass unchanged (test paths are basename-unique)
- [ ] `cargo build` passes
- [ ] `cargo test` passes (all existing + unchanged)
- [ ] Live test: upload MacBook `collection.nml` → `/api/traktor/import` → all LAN files get metadata

### How to test on the LAN server

```bash
# Upload your MacBook's collection.nml and import
scp ~/Documents/Native\ Instruments/Traktor\ 4.5.0/collection.nml lan:/tmp/
curl -X POST http://lan:3000/api/traktor/import \
  -H 'Content-Type: application/json' \
  -d '{"path": "/tmp/collection.nml"}'

# Verify — should show matched count ~13,000
# Then check a file's BPM/key were populated:
curl -s http://lan:3000/api/files?limit=3 | jq '.data[] | {title, bpm, musicalKey, rating, playCount}'
```

### Future: `track_metadata` table (follow-up plan, out of scope)

After basename matching works, the next step is a `track_metadata` table
keyed by `(artist, title COLLATE NOCASE)` that stores canonical
BPM/key/rating/play_count/last_played independently of file paths. The
Traktor import would UPSERT into this table in addition to updating
`files`. The Files page would COALESCE from `track_metadata`. This
enables:

- Metadata survives deleting and re-scanning files
- Multiple file formats (stem + flac) share the same metadata
- Import from any machine without needing files in DB at all

---

