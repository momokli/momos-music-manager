-- Migration 007: Add snapshot_id to service_playlists for global poller change detection
-- The global poller compares stored snapshot_id with Spotify's current value to detect
-- playlist changes without fetching tracks unnecessarily.

ALTER TABLE service_playlists ADD COLUMN snapshot_id TEXT;

-- Create index for efficient lookup during poll cycles
CREATE INDEX idx_service_playlists_snapshot ON service_playlists(service, snapshot_id);

SELECT 'Migration 007 applied: added snapshot_id to service_playlists' as status;
