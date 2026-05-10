-- Migration 002: Add fetch tracking columns to service_playlists
-- Tracks last_fetched_at timestamp and remote_track_count for each playlist
-- Used by the sync worker to track playlist fetch state

ALTER TABLE service_playlists ADD COLUMN last_fetched_at INTEGER;
ALTER TABLE service_playlists ADD COLUMN remote_track_count INTEGER NOT NULL DEFAULT 0;

-- Verification
SELECT 'Migration 002 applied successfully: added last_fetched_at + remote_track_count to service_playlists' as status;
