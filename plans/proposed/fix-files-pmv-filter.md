## Plan: fix-files-pmv-filter

**Status**: proposed
**Branch**: `fix/files-pmv-filter`
**Ready for review**: no
**Depends on**: `fix/scan-folder-task-tracking`
**Migration needed**: no

### Description

The Files PMV filter reads the `[PMV]` bracket string from the `comment`
column using `SUBSTR(files.comment, 2, 1)` — but that's a display/export
artifact, not the actual tag category data. The correct approach is to
query `file_resolved_tags.prefix`, which reflects the actual Phase/Mood/Vibe
tags assigned to a file through the tag→playlist→track→file resolution chain.

The Tracks PMV filter already does this correctly using
`track_resolved_tags.prefix`. Files uses the wrong data source in
**three separate places**: `get_files()`, `get_files_count()`, and
`build_files_filter_sql()`.

### Root cause

The `[PMV]` bracket in the comment string is a write-only export artifact.
When a file has Mood and Vibe tags, the comment writer writes `[ MV] tags...`
— but this string can go stale (comment not yet written, tags changed since
last write). The actual truth is in `file_resolved_tags.prefix`.

### Fix: Replace SUBSTR with file_resolved_tags EXISTS

**`get_files()`** (~line 7159) and **`get_files_count()`** (~line 7627):

```rust
// Before (wrong — parses comment string):
sql.push_str(" AND (files.comment IS NOT NULL AND files.comment LIKE '[___]%' AND     (SUBSTR(files.comment, 2, 1) = 'P' OR SUBSTR(files.comment, 3, 1) = 'P' OR SUBSTR(files.comment, 4, 1) = 'P'))");

// After (correct — queries actual tag category data):
sql.push_str(" AND EXISTS (SELECT 1 FROM file_resolved_tags frt     WHERE frt.file_id = f.id AND LOWER(frt.prefix) IN ('p','m','v'))");
```

**Categories filter (OR logic)**:

```sql
AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) IN (?,?,...))
```

**Full aggregate (AND logic)**:

```sql
AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) = 'p')
AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) = 'm')
AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) = 'v')
```

**Partial aggregate (OR logic)**:

```sql
AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) IN ('p','m','v'))
```

**None aggregate**:

```sql
AND NOT EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) IN ('p','m','v'))
```

**`build_files_filter_sql()`** (~line 1910) — same fix, same SQL pattern.

### Files to modify

- `src/api.rs` — replace PMV `SUBSTR` logic in `get_files()` (~lines 7159-7208)
- `src/api.rs` — replace PMV `SUBSTR` logic in `get_files_count()` (~lines 7627-7665)
- `src/api.rs` — replace PMV `SUBSTR` logic in `build_files_filter_sql()` (~lines 1910-1950)
- `tests/api_files.rs` — fix `files_filter_pmv_categories` and `files_filter_pmv_aggregate_full` tests (no longer need `[PMV]` comment strings, just need `file_resolved_tags` with P/M/V prefixes)
- `tests/api_tracks.rs` — add PMV filter tests for tracks (tracks already use correct `track_resolved_tags.prefix` mechanism)

### Seed data implications

After the fix, PMV filter tests don't need comment strings at all. They need:

- Tags in Phase, Mood, and Vibe categories
- Playlist→tag name matching
- Track→playlist linking
- File→track linking via ISRC/spotify_id
- `refresh_file_resolved_tags()` called after seeding

The existing `seed_tag_hierarchy()` already creates Mood (id=11, "shadow") and
Vibe (id=12, "techno") tags. With file 1 linked to those via parent resolution,
the PMV filter becomes testable. We just need to add a Phase-category tag to
complete the set.

### Acceptance Criteria

- [ ] `get_files()` PMV filter uses `file_resolved_tags.prefix`, not `SUBSTR(comment)`
- [ ] `get_files_count()` PMV filter uses same correct mechanism
- [ ] `build_files_filter_sql()` PMV filter uses same correct mechanism
- [ ] `?pmvCategories=m` returns files with Mood-category tags
- [ ] `?pmvCategories=v` returns files with Vibe-category tags
- [ ] `?pmvAggregate=full` returns files with P+M+V tags (all three)
- [ ] `?pmvAggregate=partial` returns files with at least one PMV tag
- [ ] `?pmvAggregate=none` returns files with no PMV tags
- [ ] Count endpoint matches list endpoint for all PMV filter variants
- [ ] All 190 existing tests still pass
- [ ] `cargo build` passes

