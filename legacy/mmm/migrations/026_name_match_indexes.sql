-- Migration 026: expression indexes for the tag↔playlist name match
--
-- Tags are linked to playlists by name (`LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))`)
-- in `v_track_tags`, `v_tag_playlist` and a number of direct queries. A plain
-- index on `name` cannot serve that predicate, so SQLite fell back to a nested
-- loop that scanned the whole `service_playlist_tracks` table (61k rows) once
-- per tag (524) — ~32M row visits.
--
-- Measured on a production-scale library (61,635 playlist tracks / 500 playlists
-- / 524 tags) before this migration:
--
--   backpack query via v_track_tags          8.8 s
--   full v_track_tags materialisation        8.8 s
--   the same query under pool contention     245 s   <- /api/backpack timeout
--
-- After (expression indexes present, no ANALYZE needed):
--
--   backpack query via v_track_tags          0.05 s
--   full v_track_tags materialisation        0.04 s
--
-- These are pure query accelerators: no view, query or semantic changes, so
-- every consumer benefits at once — `v_track_tags`, `v_tag_playlist`,
-- `refresh_track_resolved_tags`, the digging queries, and the API tag filters.
--
-- `track_resolved_tags.tag_name` deliberately gets no such index: it is queried
-- as `trt.tag_name IN (...)` (plain, collation-aware), which the existing
-- `idx_trt_tag_name` already serves.

-- Backs the tag→playlist / playlist→tag name join.
CREATE INDEX IF NOT EXISTS idx_tags_name_norm
    ON tags (LOWER(TRIM(name)));

-- Backs the playlist side of the same join, plus the literal
-- `LOWER(TRIM(sp.name)) = 'backpack'` lookups used by the Backpack transport.
CREATE INDEX IF NOT EXISTS idx_service_playlists_name_norm
    ON service_playlists (LOWER(TRIM(name)));

-- `file_resolved_tags.tag_name` is filtered as `LOWER(TRIM(tag_name)) IN (...)`
-- by the Files-page tag filters; the existing plain `idx_frt_tag_name` cannot
-- serve that expression.
CREATE INDEX IF NOT EXISTS idx_frt_tag_name_norm
    ON file_resolved_tags (LOWER(TRIM(tag_name)));

SELECT 'Migration 026 applied: name-match expression indexes created' as status;
