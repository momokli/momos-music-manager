-- Migration 008: Soft-delete playlist tracks + archive_deleted toggle
--
-- Changes:
--   1. Add deleted_at to service_playlist_tracks (NULL = active, timestamp = deleted)
--   2. Add archive_deleted to service_playlists (default 0)
--   3. Set archive_deleted = 1 for subscribed/followed playlists
--   4. Recreate v_file_tags with archive_deleted filter
--   5. Recreate v_file_resolved_tags with archive_deleted filter
--   6. Recreate v_tag_file_counts

-- Step 1: Add deleted_at to service_playlist_tracks
ALTER TABLE service_playlist_tracks ADD COLUMN deleted_at INTEGER;

-- Step 2: Add archive_deleted to service_playlists
ALTER TABLE service_playlists ADD COLUMN archive_deleted BOOLEAN NOT NULL DEFAULT 0;

-- Step 3: Set archive_deleted = 1 for subscribed playlists
UPDATE service_playlists SET archive_deleted = 1
WHERE EXISTS (
    SELECT 1 FROM playlist_subscriptions ps
    WHERE ps.service = service_playlists.service
      AND ps.playlist_id = service_playlists.playlist_id
);

-- Step 4: Drop dependent views
DROP VIEW IF EXISTS v_tag_file_counts;
DROP VIEW IF EXISTS v_file_resolved_tags;
DROP VIEW IF EXISTS v_file_tags;

-- Step 5: Recreate v_file_tags with archive_deleted filter
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

-- Step 6: Recreate v_file_resolved_tags with archive_deleted filter
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

-- Step 7: Recreate v_tag_file_counts
CREATE VIEW v_tag_file_counts AS
SELECT vft.tag_id, COUNT(DISTINCT vft.file_id) AS file_count
FROM v_file_tags vft
GROUP BY vft.tag_id;

SELECT 'Migration 008 applied: soft-delete playlist tracks + archive_deleted toggle' as status;
