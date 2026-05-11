-- Migration 003: Add remote_unique_count to service_playlists
-- Tracks the number of unique tracks in a playlist after a sync.
-- remote_track_count = Spotify's tracks.total (all items, incl. duplicates/episodes)
-- remote_unique_count = distinct tracks from the sync stream (unique only)
-- The difference shows "noise" (duplicates, episodes) that doesn't need syncing.

ALTER TABLE service_playlists ADD COLUMN remote_unique_count INTEGER NOT NULL DEFAULT 0;

-- Verification
SELECT 'Migration 003 applied: added remote_unique_count to service_playlists' as status;
