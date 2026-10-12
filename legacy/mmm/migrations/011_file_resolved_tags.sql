-- Migration 011: Materialized file_resolved_tags table for query performance
--
-- The v_file_resolved_tags view chains 5 joins with case-insensitive LOWER(TRIM)
-- matching, which SQLite cannot index. Every query that uses this view performs
-- a full scan of the join chain. This migration creates a materialized lookup
-- table that pre-computes the resolved tag→file mapping.
--
-- The table is populated by calling refresh_file_resolved_tags() after any
-- data changes (syncs, tag parent changes, etc.) and maintained via triggers.

-- Step 1: Create the materialized table
CREATE TABLE IF NOT EXISTS file_resolved_tags (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL,
    tag_name TEXT NOT NULL,
    category_id INTEGER NOT NULL,
    category_name TEXT NOT NULL,
    prefix TEXT NOT NULL,
    sort_order INTEGER DEFAULT 0,
    created_at INTEGER,
    is_default BOOLEAN DEFAULT FALSE,
    PRIMARY KEY (file_id, tag_id)
);

CREATE INDEX IF NOT EXISTS idx_frt_tag_id ON file_resolved_tags(tag_id);
CREATE INDEX IF NOT EXISTS idx_frt_category_id ON file_resolved_tags(category_id);
CREATE INDEX IF NOT EXISTS idx_frt_prefix ON file_resolved_tags(prefix);
CREATE INDEX IF NOT EXISTS idx_frt_tag_name ON file_resolved_tags(tag_name);

-- Step 2: Populate the table from the existing view
INSERT OR IGNORE INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at, is_default)
SELECT DISTINCT
    vfr.file_id,
    vfr.tag_id,
    vfr.tag_name,
    vfr.category_id,
    vfr.category_name,
    vfr.prefix,
    vfr.sort_order,
    vfr.created_at,
    COALESCE((SELECT tc.is_default FROM tag_categories tc WHERE tc.id = vfr.category_id), 0)
FROM v_file_resolved_tags vfr;

-- Step 3: Add missing indexes for other performance improvements

-- Composite index for file_locations EXISTS subquery on backed_up filter
CREATE INDEX IF NOT EXISTS idx_file_locations_file_type ON file_locations(file_id, location_type);

-- Index for deemix playlists join (avoid LIKE %/ scan)
CREATE INDEX IF NOT EXISTS idx_deemix_downloads_url ON deemix_downloads(spotify_playlist_url);

-- Index for service_playlist_tracks deleted_at filtering in views
CREATE INDEX IF NOT EXISTS idx_spt_deleted ON service_playlist_tracks(deleted_at);

SELECT 'Migration 011 applied: materialized file_resolved_tags + indexes' as status;
