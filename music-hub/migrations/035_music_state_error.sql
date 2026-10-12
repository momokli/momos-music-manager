-- Store the optional error/reason reported by `GET /isrc/{isrc}` alongside the
-- cached state (e.g. `absent` + "no data", `failed` + "download timeout"), so
-- the track page and the downloads table can show *why* a track is not ready
-- without a per-track status request.
ALTER TABLE hub_music_state ADD COLUMN error TEXT;

SELECT 'Migration 035 applied: hub_music_state error column added' as status;
