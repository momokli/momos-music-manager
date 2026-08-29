-- Migration 023: Manual file↔track link overrides
--
-- Adds a corrections table that takes precedence over automatic
-- ISRC/service_id matching in v_file_track_link.
--
-- link_type = 'include' → explicitly link this file to this track
-- link_type = 'exclude' → explicitly prevent automatic linking
--
-- The view uses a UNION: manual includes always win, then automatic
-- matches minus any excluded pairs.

-- Step 1: Create the corrections table
CREATE TABLE IF NOT EXISTS file_track_corrections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES service_tracks(id) ON DELETE CASCADE,
    link_type TEXT NOT NULL CHECK (link_type IN ('include', 'exclude')),
    reason TEXT,
    created_at INTEGER DEFAULT (unixepoch()),
    UNIQUE(file_id, track_id)
);

CREATE INDEX IF NOT EXISTS idx_ftc_file ON file_track_corrections(file_id);
CREATE INDEX IF NOT EXISTS idx_ftc_track ON file_track_corrections(track_id);

-- Step 2: Drop dependent views (must drop leaf views first)
DROP VIEW IF EXISTS v_tag_file_counts;
DROP VIEW IF EXISTS v_file_resolved_tags;
DROP VIEW IF EXISTS v_file_tags;
DROP VIEW IF EXISTS v_file_track_link;

-- Step 3: Recreate v_file_track_link with correction overrides
--
-- Manual includes always win (UNION places them first).
-- Automatic matches are excluded when an 'exclude' correction exists for that pair.
CREATE VIEW v_file_track_link AS
-- Manual includes (always win)
SELECT file_id, track_id FROM file_track_corrections WHERE link_type = 'include'
UNION
-- Automatic matches, minus excluded pairs
SELECT f.id AS file_id, st.id AS track_id
FROM files f
JOIN service_tracks st ON (
    st.isrc = f.isrc
    OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
    OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
    OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
    OR (st.service = 'local' AND st.service_id = CAST(f.id AS TEXT))
)
WHERE NOT EXISTS (
    SELECT 1 FROM file_track_corrections ftc
    WHERE ftc.file_id = f.id
      AND ftc.track_id = st.id
      AND ftc.link_type = 'exclude'
);

-- Step 4: Recreate v_file_tags (identical to migration 008)
CREATE VIEW v_file_tags AS
SELECT DISTINCT f.id AS file_id,
       t.id AS tag_id, t.name AS tag_name,
       t.sort_order, t.created_at,
       tc.id AS category_id, tc.name AS category_name,
       tc.is_default, tc.prefix
FROM files f
JOIN v_file_track_link v ON v.file_id = f.id
JOIN service_playlist_tracks spt ON spt.track_id = v.track_id
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN tag_categories tc ON tc.id = t.category_id
WHERE sp.archive_deleted = 1 OR spt.deleted_at IS NULL;

-- Step 5: Recreate v_file_resolved_tags (identical to migration 008)
CREATE VIEW v_file_resolved_tags AS
SELECT DISTINCT
    f.id AS file_id,
    rt.tag_id,
    rt.tag_name,
    rt.sort_order,
    rt.created_at,
    rt.category_id,
    rt.category_name,
    rt.prefix
FROM files f
JOIN v_file_track_link v ON v.file_id = f.id
JOIN service_playlist_tracks spt ON spt.track_id = v.track_id
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN v_resolved_tags rt ON rt.source_tag_id = t.id
WHERE sp.archive_deleted = 1 OR spt.deleted_at IS NULL;

-- Step 6: Recreate v_tag_file_counts (identical to migration 008)
CREATE VIEW v_tag_file_counts AS
SELECT vft.tag_id, COUNT(DISTINCT vft.file_id) AS file_count
FROM v_file_tags vft
GROUP BY vft.tag_id;

SELECT 'Migration 023 applied: file_track_corrections table + corrected v_file_track_link' as status;
