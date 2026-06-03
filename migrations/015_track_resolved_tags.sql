-- Migration 015: Materialized track_resolved_tags table for track→tag query performance
--
-- The v_track_tags view chains 4 joins with case-insensitive LOWER(TRIM) matching,
-- which SQLite cannot index. Every track-level tag query (Tracks page filters,
-- digging PMV/tag filters, track detail) performs a full scan of the join chain.
--
-- This migration creates a materialized lookup table that pre-computes
-- the resolved tag→track mapping.
--
-- The table is populated by calling refresh_track_resolved_tags() after any
-- data changes (syncs, tag parent changes, etc.).
--
-- On-disk impact: ~track_count × avg_tags_per_track rows (typ. 50k–200k rows)

-- Step 1: Create the materialized table
CREATE TABLE IF NOT EXISTS track_resolved_tags (
    track_id INTEGER NOT NULL REFERENCES service_tracks(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL,
    tag_name TEXT NOT NULL,
    category_id INTEGER NOT NULL,
    category_name TEXT NOT NULL,
    prefix TEXT NOT NULL,
    is_default BOOLEAN DEFAULT FALSE,
    PRIMARY KEY (track_id, tag_id)
);

-- Step 2: Indexes for common filter patterns
CREATE INDEX IF NOT EXISTS idx_trt_tag_name ON track_resolved_tags(tag_name);
CREATE INDEX IF NOT EXISTS idx_trt_prefix ON track_resolved_tags(prefix);
CREATE INDEX IF NOT EXISTS idx_trt_track_id ON track_resolved_tags(track_id);

-- Step 3: Populate from v_track_tags view
INSERT OR IGNORE INTO track_resolved_tags (track_id, tag_id, tag_name, category_id, category_name, prefix, is_default)
SELECT DISTINCT
    vtt.track_id, vtt.tag_id, vtt.tag_name, vtt.category_id, vtt.category_name, vtt.prefix, vtt.is_default
FROM v_track_tags vtt;

SELECT 'Migration 015 applied: track_resolved_tags materialized table created' as status;
