-- Spotify's `collaborative` flag: true when several accounts can write to the
-- playlist ("contributed" playlists owned by one but co-edited by others).
ALTER TABLE hub_playlists ADD COLUMN collaborative INTEGER NOT NULL DEFAULT 0;
