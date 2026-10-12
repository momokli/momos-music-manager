-- Spotify playlist owner (the account that created the playlist), distinct from
-- the hub user who has it in their library. Populated on playlist fetch/ingest.
ALTER TABLE hub_playlists ADD COLUMN owner_id   TEXT;
ALTER TABLE hub_playlists ADD COLUMN owner_name TEXT;
