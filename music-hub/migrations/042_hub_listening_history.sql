-- Listening history (#319): record when a user listens to a track (from the
-- now-playing poll), and let unknown Spotify tracks be ingested on the fly.

CREATE TABLE hub_listening_history (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    played_at   TEXT NOT NULL,
    progress_ms INTEGER,
    duration_ms INTEGER,
    source      TEXT NOT NULL DEFAULT 'spotify'
);
CREATE INDEX IF NOT EXISTS idx_hub_listen_user_time
    ON hub_listening_history(user_id, played_at DESC);
CREATE INDEX IF NOT EXISTS idx_hub_listen_track ON hub_listening_history(track_id);
