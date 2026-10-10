-- Audio features from ReccoBeats (Spotify's audio-features replacement; no auth).
-- One row per track once we've tried the lookup (found=0 = tried, not available).
CREATE TABLE hub_track_features (
    track_id         INTEGER PRIMARY KEY REFERENCES hub_tracks(id) ON DELETE CASCADE,
    found            INTEGER NOT NULL DEFAULT 1,
    reccobeats_id    TEXT,
    isrc             TEXT,
    bpm              REAL,
    key_pitch        INTEGER,   -- 0..11 (Spotify pitch class)
    key_mode         INTEGER,   -- 0 = minor, 1 = major
    camelot          TEXT,      -- derived, e.g. "8B"
    energy           REAL,
    danceability     REAL,
    valence          REAL,
    acousticness     REAL,
    instrumentalness REAL,
    liveness         REAL,
    loudness         REAL,
    speechiness      REAL,
    source           TEXT NOT NULL DEFAULT 'reccobeats',
    fetched_at       TEXT
);
CREATE INDEX idx_hub_track_features_bpm ON hub_track_features(bpm);
