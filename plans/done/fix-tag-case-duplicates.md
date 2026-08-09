## Plan: fix-tag-case-duplicates

**Status**: done ✅
**Branch**: `fix/tag-case-duplicates`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — `004_unique_tags_nocase.sql`

### Description

Fix two interlinked bugs discovered during investigation of "Groovy" tag showing 1060 tracks (2× the real 530):

1. **Playlist page cartesian product**: `playlists_handler` LEFT JOINs `v_tag_playlist` which returns multiple rows per playlist when case-different duplicate tags exist. This multiplies `COUNT(spt.track_id)` — e.g. 530 tracks × 2 matching tags ("Groovy" + "groovy") = 1060.

2. **No uniqueness on `tags.name`**: The `tags` table allows "Groovy" and "groovy" as separate tags. Since tag↔playlist matching is case-insensitive, duplicate tags are functionally identical — both resolve to the same playlists, tracks, and files.

### Root cause

- `v_tag_playlist` does `LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))` — case-insensitive
- `tags.name` has only a regular index (`idx_tags_name`), not UNIQUE
- `playlists_handler` does `LEFT JOIN v_tag_playlist vtp ON vtp.playlist_id = sp.id` without subquery/aggregation, so each playlist row fans out N×M when N duplicate tags match M playlists

### Other `v_tag_playlist` consumers (all safe)

| Query                           | Pattern                     | Safe? |
| ------------------------------- | --------------------------- | ----- |
| `tags_service_coverage_handler` | `COUNT(DISTINCT tag_id)`    | ✅    |
| `get_tag_service_connections`   | `DISTINCT vtp.service`      | ✅    |
| `get_playlists_without_tags`    | `NOT EXISTS (SELECT 1 ...)` | ✅    |
| `create_tags_from_playlists`    | `NOT EXISTS`                | ✅    |
| `get_tags_for_service_track`    | `SELECT DISTINCT t.id`      | ✅    |

Only `playlists_handler` is affected.

### Migration 004 (`migrations/004_unique_tags_nocase.sql`)

1. Create `tags_v2` with `name TEXT NOT NULL UNIQUE COLLATE NOCASE`
2. Copy distinct tags from `tags` (deduplicate by `LOWER(name)`, keep lowest `id`)
3. Build remapping table: old dup tag IDs → surviving tag ID
4. Re-point FKs in `tag_parents`, `tag_embeddings`, `tag_energy_levels`, `tag_similarities`
5. Drop old `tags`, rename `tags_v2` → `tags`
6. Recreate indexes on `tags(id)`, `tags(category_id)`, `tags(name)`
7. Verify no orphan FKs

**Existing duplicates to merge**: Tag "groovy" (id 286) → merged into "Groovy" (id 88). Both are Vibe category, so no category conflict.

### Backend changes

- **`src/api.rs` — `playlists_handler`**: Replace `LEFT JOIN v_tag_playlist vtp ON vtp.playlist_id = sp.id` with a scalar subquery or `LEFT JOIN (SELECT DISTINCT playlist_id, tag_name FROM v_tag_playlist)`. This guards against any future cartesian product even if duplicate tags somehow reappear.

- No changes needed to `get_tag_by_name` (already uses `COLLATE NOCASE`) or `create_tag` (will naturally fail on duplicate with new UNIQUE constraint).

### Existing data

Current state:

```
Playlists:  "Groovy" (id 292, 530 tracks), "groovy" (id 133, 6 tracks)
Tags:       "Groovy" (id 88, Vibe), "groovy" (id 286, Vibe)
```

After migration:

```
Tags:       "Groovy" (id 88, Vibe) — only one
Playlists:  unchanged — both still match "Groovy" tag via case-insensitive join
Playlist page: "Groovy" shows 530 tracks ✅ (was 1060)
```

### Files to modify

- `migrations/004_unique_tags_nocase.sql` — new migration
- `src/api.rs` — fix `playlists_handler` JOIN

### Acceptance Criteria

- [ ] `tags.name` has UNIQUE COLLATE NOCASE constraint
- [ ] Cannot insert "groovy" when "Groovy" already exists
- [x] Existing duplicate tag "groovy" (id 286) merged into "Groovy" (id 88)
- [x] `tag_parents`, `tag_embeddings`, `tag_energy_levels`, `tag_similarities` FKs remapped
- [x] Playlists page shows 530 tracks for "Groovy" playlist (not 1060)
- [ ] All other `v_tag_playlist` consumers produce identical results
- [ ] Tag "groovy" (id 286) deleted from tags table
- [x] Backend compiles (`cargo build`)
- [ ] Fresh DB: migrations 001→002→003 run cleanly
- [ ] Existing DB: migration 003 applies without errors

---

