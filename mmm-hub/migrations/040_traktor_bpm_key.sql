-- Traktor collection.nml: also carry BPM + musical key per track (in addition to
-- play count / last played / rating), so uploading the NML backfills the hub's
-- BPM/key for the user's library (MMM-style).

ALTER TABLE hub_traktor_tracks ADD COLUMN bpm REAL;
-- Camelot notation derived from the Traktor KEY attribute (e.g. "8m" -> "8A").
ALTER TABLE hub_traktor_tracks ADD COLUMN camelot TEXT;

DROP VIEW IF EXISTS hub_v_track_traktor;
CREATE VIEW hub_v_track_traktor AS
SELECT t.track_id                                              AS track_id,
       MAX(t.play_count)                                       AS play_count,
       MAX(t.last_played)                                      AS last_played,
       MAX(t.rating)                                           AS rating,
       MAX(t.bpm)                                              AS bpm,
       MAX(t.camelot)                                          AS camelot,
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
