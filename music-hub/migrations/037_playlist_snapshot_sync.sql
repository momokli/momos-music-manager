-- Incremental playlist re-fetch: remember the remote snapshot at the time we
-- last fetched a playlist's items. The worker then only re-fetches playlists
-- whose `snapshot_id` changed since — saving Spotify API calls / 429 headroom.
ALTER TABLE hub_playlists ADD COLUMN last_synced_snapshot TEXT;
