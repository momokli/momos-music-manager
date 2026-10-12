-- Migration 005: v_playlist_tag_category view for category ID-based filtering
--
-- Creates a view that resolves playlist → tag → category, used by the
-- playlists page category filter. Filtering by category ID (integer) is
-- more stable than filtering by prefix letter, especially when categories
-- can be renamed or new ones added.
--
-- This view is referenced in playlists_handler in src/api.rs.

CREATE VIEW v_playlist_tag_category AS
SELECT DISTINCT
    sp.id AS playlist_id,
    tc.id AS category_id,
    tc.name AS category_name,
    tc.prefix
FROM service_playlists sp
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN tag_categories tc ON tc.id = t.category_id;

-- ============================================================
-- Verify view exists
-- ============================================================

SELECT CASE
    WHEN (
        SELECT COUNT(*) FROM sqlite_master
        WHERE type = 'view' AND name = 'v_playlist_tag_category'
    ) = 1
    THEN 'OK'
    ELSE 'FAIL'
END AS check_view_created;

-- ============================================================
-- Status
-- ============================================================

SELECT 'Migration 005 applied: v_playlist_tag_category view' as status;
