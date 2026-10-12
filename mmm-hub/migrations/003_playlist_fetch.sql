-- Playlist fetch control — the backend worker picks these up.
ALTER TABLE hub_playlists ADD COLUMN is_owned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE hub_playlists ADD COLUMN enabled_for_fetch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE hub_playlists ADD COLUMN fetch_status TEXT;
ALTER TABLE hub_playlists ADD COLUMN fetch_error TEXT;
