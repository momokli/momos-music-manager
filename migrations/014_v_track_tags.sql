-- Migration 014: v_track_tags view
-- Resolves every service track's tags through its playlists.
-- Chain: service_playlist_tracks → service_playlists → tags → tag_categories
-- Used by Tags/PMV filter queries on the Tracks page.
CREATE VIEW IF NOT EXISTS v_track_tags AS
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
JOIN tag_categories tc ON tc.id = t.category_id;

SELECT 'Migration 014 applied: v_track_tags view created' as status;
