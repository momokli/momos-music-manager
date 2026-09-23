-- Migration 025: Backpack concept — unify "subscribed playlists" + "tags.backpack"
-- into a single transport model.
--
-- Context (issue #32): the previous system submitted *each* marked playlist to
-- deemix separately and tracked *each* playlist as its own row in
-- `deemix_downloads` (playlist-granular, `spotify_playlist_url UNIQUE`). The
-- Backpack concept replaces that with ONE aggregated Spotify playlist ("Backpack")
-- and exactly ONE deemix transport row.
--
-- This migration (additive, idempotent, no data loss on source data):
--   1. Adds `is_backpack` to `deemix_downloads` to mark the single transport row.
--   2. Consolidates the N legacy per-playlist rows into ONE pending Backpack row
--      (status is aggregated: any active download wins → queued; else failed;
--      else completed; track counts are summed; latest error is kept).
--   3. Removes the now-consolidated legacy rows.
--
-- `deemix_downloads` is *derived* state (a mirror of the deemix queue + user
-- actions), so consolidating it loses no source data: subscriptions
-- (`playlist_subscriptions`) and `tags.backpack` flags are untouched.
--
-- The real Backpack Spotify URL is only known after the playlist is first
-- materialised at runtime. Until then we use a stable sentinel URL; the runtime
-- (`crate::backpack::record_backpack_submit`) replaces it with the real URL and
-- deletes any other `is_backpack = 1` row on first materialisation.

-- 1. Marker column for the single transport row.
ALTER TABLE deemix_downloads ADD COLUMN is_backpack BOOLEAN NOT NULL DEFAULT 0;

-- 2. Aggregate legacy rows into one pending Backpack row.
INSERT INTO deemix_downloads
    (spotify_playlist_url, playlist_name, status, track_count_total,
     track_count_downloaded, error_message, is_backpack, created_at, updated_at)
SELECT
    'https://open.spotify.com/playlist/backpack',
    'Backpack',
    CASE
        WHEN SUM(CASE WHEN status IN ('queued', 'downloading') THEN 1 ELSE 0 END) > 0 THEN 'queued'
        WHEN SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) > 0 THEN 'failed'
        ELSE 'completed'
    END,
    COALESCE(SUM(track_count_total), 0),
    COALESCE(SUM(track_count_downloaded), 0),
    (SELECT error_message FROM deemix_downloads
      WHERE error_message IS NOT NULL AND is_backpack = 0
      ORDER BY updated_at DESC LIMIT 1),
    1,
    MIN(created_at),
    MAX(updated_at)
FROM deemix_downloads
WHERE is_backpack = 0
HAVING COUNT(*) > 0;

-- 3. Remove the consolidated legacy rows (only the single Backpack row remains).
DELETE FROM deemix_downloads WHERE is_backpack = 0;

SELECT 'Migration 025 applied: backpack concept (deemix_downloads consolidated + is_backpack added)' as status;
