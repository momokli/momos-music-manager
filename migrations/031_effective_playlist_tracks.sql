-- Migration 031: "removed" means removed — unless the playlist archives.
--
-- `service_playlist_tracks.deleted_at` is a tombstone, and it is only meaningful
-- for *archiving* playlists (`archive_deleted = 1`): that is the mode whose whole
-- point is "removed tracks stay around for tagging" (see
-- `set_playlist_archive_deleted`). For every other playlist the sync's
-- soft-delete-before-reinsert strategy left tombstones behind — thousands of
-- them — and those leaked into tag resolution, backpack membership and comment
-- targets, because all of those resolve playlist membership by reading
-- `service_playlist_tracks` directly.
--
-- 1. Drop the tombstones that do not belong to an archiving playlist.
-- 2. Recreate `v_track_tags` with the guard every other resolution view already
--    carries (`v_file_resolved_tags`, `v_file_tags` from migration 008/023):
--        sp.archive_deleted = 1 OR spt.deleted_at IS NULL

DELETE FROM service_playlist_tracks
 WHERE deleted_at IS NOT NULL
   AND playlist_id IN (SELECT id FROM service_playlists WHERE archive_deleted = 0);

DROP VIEW IF EXISTS v_track_tags;

CREATE VIEW v_track_tags AS
SELECT DISTINCT
    spt.track_id,
    t.id AS tag_id,
    t.name AS tag_name,
    tc.id AS category_id,
    tc.name AS category_name,
    tc.prefix,
    tc.is_default
FROM service_playlist_tracks spt
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN tag_categories tc ON tc.id = t.category_id
WHERE sp.archive_deleted = 1 OR spt.deleted_at IS NULL;

SELECT 'Migration 031 applied: tombstones purged + v_track_tags guarded' as status;
