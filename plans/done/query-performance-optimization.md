## Plan: query-performance-optimization

**Status**: done ✅
**Branch**: `feat/query-performance-optimization`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — `011_file_resolved_tags.sql`

### Description

Overhaul query performance for files, playlists, and digging pages. Replace the `v_file_resolved_tags` view (5-join chain with unindexable LOWER/TRIM) with a materialized `file_resolved_tags` table. Add batch comment computation. Fix the deemix playlist join to use exact match. Extract FileFilterBuilder to eliminate duplicated filter SQL.

### Files modified

- `migrations/011_file_resolved_tags.sql` — new migration: `file_resolved_tags` table + 4 indexes + 3 missing indexes (`file_locations`, `deemix_downloads`, `spt.deleted_at`)
- `src/db.rs` — new functions: `compute_target_comments_batch()`, `get_file_resolved_tags_batch()`, `refresh_file_resolved_tags()`
- `src/api.rs` — replaced all `v_file_resolved_tags`/`v_file_tags` view references with `file_resolved_tags` table; batch comment computation in `get_files()` and `get_files_count()`; fixed deemix `LIKE '%/'` → exact match
- `src/digging.rs` — batch tag loading in `search_digging_tracks()` instead of per-row N+1 queries

### Acceptance Criteria

- [x] Migration 011 runs cleanly on fresh DB (001→011)
- [x] Migration 011 runs cleanly on existing DB with data
- [x] `file_resolved_tags` table populated from `v_file_resolved_tags` view
- [x] All `v_file_resolved_tags` and `v_file_tags` view references replaced with `file_resolved_tags` table
- [x] Batch comment computation: `get_files()` with `commentStatuses=needs_update` uses 2 queries instead of N+1
- [x] Batch tag loading: `search_digging_tracks()` uses 1 query instead of N per-row queries
- [x] Deemix join uses exact match (`=`) instead of `LIKE '%/'`, indexable
- [x] New indexes: `idx_frt_tag_name`, `idx_file_locations_file_type`, `idx_deemix_downloads_url`, `idx_spt_deleted`
- [x] `cargo build` passes
- [x] No regressions: files, playlists, digging, tracks pages all work

