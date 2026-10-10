-- Cache of music-api per-ISRC state, so pages can show ready/total without one
-- status request per track on every load. Refreshed explicitly (order/refresh
-- actions) and read for the progress indicator.
CREATE TABLE hub_music_state (
    isrc       TEXT PRIMARY KEY,
    state      TEXT NOT NULL,
    checked_at TEXT NOT NULL
);
