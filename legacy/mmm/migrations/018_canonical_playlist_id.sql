ALTER TABLE service_playlists ADD COLUMN canonical_playlist_id TEXT;
CREATE INDEX IF NOT EXISTS idx_sp_canonical ON service_playlists(canonical_playlist_id);
SELECT 'Migration 018 applied: canonical_playlist_id' as status;
