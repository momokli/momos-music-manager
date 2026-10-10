-- Traktor collection ingest (per user). Tracks the collection metadata needed
-- by the ripeness/scoring engine: play count, last played, rating, and the
-- occurrence of a track in Traktor playlists / collection / session history.
CREATE TABLE hub_traktor_tracks (
    user_id     INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    play_count  INTEGER NOT NULL DEFAULT 0,
    last_played TEXT,
    rating      INTEGER,           -- NULL = none, else 1..5
    imported_at TEXT,
    PRIMARY KEY (user_id, track_id)
);

-- A Traktor node (playlist, the collection itself, or a session/history list).
CREATE TABLE hub_traktor_playlists (
    id          INTEGER NOT NULL,  -- Traktor node id (per user, from the NML)
    user_id     INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    node_type   TEXT NOT NULL,     -- 'playlist' | 'collection' | 'session'
    imported_at TEXT,
    PRIMARY KEY (user_id, id)
);

CREATE TABLE hub_traktor_playlist_tracks (
    user_id     INTEGER NOT NULL,
    playlist_id INTEGER NOT NULL,
    track_id    INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, playlist_id, track_id),
    FOREIGN KEY (user_id, playlist_id)
        REFERENCES hub_traktor_playlists(user_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_hub_traktor_tracks_track ON hub_traktor_tracks(track_id);
CREATE INDEX idx_hub_traktor_pl_tracks_track ON hub_traktor_playlist_tracks(track_id);

-- Consolidated per-track Traktor signal for the scoring engine.
CREATE VIEW hub_v_track_traktor AS
SELECT t.track_id                                              AS track_id,
       MAX(t.play_count)                                       AS play_count,
       MAX(t.last_played)                                      AS last_played,
       MAX(t.rating)                                           AS rating,
       COUNT(DISTINCT t.user_id)                               AS users,
       (SELECT COUNT(*) FROM hub_traktor_playlist_tracks pt
          JOIN hub_traktor_playlists p
            ON p.user_id = pt.user_id AND p.id = pt.playlist_id
         WHERE pt.track_id = t.track_id
           AND p.node_type IN ('playlist', 'collection'))      AS playlist_occurrence,
       (SELECT COUNT(*) FROM hub_traktor_playlist_tracks pt
          JOIN hub_traktor_playlists p
            ON p.user_id = pt.user_id AND p.id = pt.playlist_id
         WHERE pt.track_id = t.track_id
           AND p.node_type = 'session')                        AS session_occurrence
  FROM hub_traktor_tracks t
 GROUP BY t.track_id;
