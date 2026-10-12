-- Store the delivered audio format(s) alongside the cached music-api state, so
-- pages can show e.g. "ready · flac" / "ready · mp3-320" without a per-track
-- status request. `source_format` is the single classified format
-- (`flac` | `mp3-320` | `mp3-128`), `formats` is the raw comma-separated list.
ALTER TABLE hub_music_state ADD COLUMN source_format TEXT;
ALTER TABLE hub_music_state ADD COLUMN formats TEXT;
ALTER TABLE hub_music_state ADD COLUMN deezer_id TEXT;

SELECT 'Migration 028 applied: hub_music_state format columns added' as status;
