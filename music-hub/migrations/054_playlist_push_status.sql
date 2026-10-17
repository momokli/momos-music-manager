-- Bi-way playlist push status (#283): when a tag/track change is mirrored back
-- into the Spotify *source* playlist, remember the outcome per playlist.
-- Separate from the fetch (read) status columns `fetch_status`/`fetch_error`.
ALTER TABLE hub_playlists ADD COLUMN last_push_at     TEXT;
ALTER TABLE hub_playlists ADD COLUMN last_push_status TEXT;
ALTER TABLE hub_playlists ADD COLUMN last_push_error  TEXT;
ALTER TABLE hub_playlists ADD COLUMN last_push_action TEXT;

SELECT 'Migration 054 applied: playlist push status (bi-way sync)' as status;
