-- Migration 027: Backpack ISRC orders against the music-api service.
--
-- MMM no longer pushes a Spotify playlist URL at deemix. Instead it orders the
-- ISRCs of Backpack tracks that have no local file from the `music-api` service
-- (an ISRC order/consume API in front of deemix) and imports the delivered
-- files. This table is the per-ISRC state machine for that flow:
--
--   ordered  -> the ISRC was placed in a music-api order
--   ready    -> the service reports a downloadable file (not yet imported)
--   imported -> the file was written to disk and a scan was triggered
--   absent   -> the service has no (streamable) match — terminal
--   failed   -> transport/deemix rejection — terminal
--
-- `absent`/`failed` are terminal: such ISRCs are never re-ordered, so a broken
-- track does not keep an order loop alive.

CREATE TABLE IF NOT EXISTS music_api_imports (
    isrc        TEXT PRIMARY KEY,
    state       TEXT NOT NULL,          -- ordered | ready | imported | absent | failed
    format      TEXT,                   -- flac | 320 | 128 (what was placed)
    file_path   TEXT,
    deezer_id   TEXT,
    error       TEXT,
    order_id    TEXT,
    updated_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_music_api_imports_state ON music_api_imports(state);

SELECT 'Migration 027 applied: music_api_imports created' as status;
