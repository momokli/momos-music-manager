-- Migration 032: distinguish curated playlists from the likes mirror and our own
-- generated packs. Only 'curated' playlists count towards a track's
-- "how well-used is this" score — likes are the baseline every track shares, and
-- 'generated' is our own output (counting either would make a track look used and
-- it would never resurface).

ALTER TABLE service_playlists ADD COLUMN playlist_kind TEXT NOT NULL DEFAULT 'curated';
-- values: 'curated' | 'liked' | 'generated'

-- The manual likes mirror (a real Spotify playlist named "liked"/"likes").
UPDATE service_playlists SET playlist_kind = 'liked'
 WHERE LOWER(TRIM(name)) IN ('liked', 'likes');

-- Existing generated dailies from the daily-tagging-queue feature.
UPDATE service_playlists SET playlist_kind = 'generated'
 WHERE service = 'local' AND name LIKE 'Daily-%';

CREATE INDEX idx_service_playlists_kind ON service_playlists(playlist_kind);

-- ── Forgotten-facts view ────────────────────────────────────────────────────
-- `last_touched_at` = the last time the track was added to ANY playlist
-- (curated or liked). The primary rediscovery signal.
-- The ADR-072 guard applies everywhere: tombstones only exist for archiving
-- playlists, but the guard is repeated so the view is correct standalone.
CREATE VIEW v_track_forgotten_facts AS
WITH playlist_facts AS (
    SELECT
        spt.track_id,
        SUM(CASE WHEN sp.playlist_kind = 'curated' THEN 1 ELSE 0 END) AS playlist_count,
        MAX(spt.added_at) AS last_touched_at,
        MAX(CASE WHEN sp.playlist_kind = 'liked' THEN spt.added_at END) AS liked_at
    FROM service_playlist_tracks spt
    JOIN service_playlists sp ON sp.id = spt.playlist_id
    WHERE sp.playlist_kind IN ('curated', 'liked')
      AND (sp.archive_deleted = 1 OR spt.deleted_at IS NULL)
    GROUP BY spt.track_id
)
SELECT
    pf.track_id,
    pf.playlist_count,
    pf.last_touched_at,
    pf.liked_at,
    (pf.liked_at IS NOT NULL) AS liked
FROM playlist_facts pf;

SELECT 'Migration 032 applied: playlist_kind + v_track_forgotten_facts' as status;
