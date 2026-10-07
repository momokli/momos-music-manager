-- Migration 033: BPM//key system playlists.
--
-- For every (BPM, key) combination present in the library we materialise a real
-- Spotify playlist on the user's account. These rows reuse the existing
-- `playlist_kind = 'generated'` marker (migration 032) so they never influence
-- tag matching, comment write-out, usage / "last touched" scoring or normal
-- playlist polling.
--
-- `system_key` is the stable combo id, independent of the display template:
--   'bpm_key:{bpm}:{canonical_key}'  e.g. 'bpm_key:124:12A'
-- It is NULL for every normal (curated/liked/generated) playlist.

ALTER TABLE service_playlists ADD COLUMN system_key TEXT;

CREATE UNIQUE INDEX idx_service_playlists_system_key
    ON service_playlists(service, system_key) WHERE system_key IS NOT NULL;

SELECT 'Migration 033 applied: service_playlists.system_key' as status;
