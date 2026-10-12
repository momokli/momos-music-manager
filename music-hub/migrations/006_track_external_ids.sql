CREATE TABLE hub_track_external_ids (
    track_id    INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    service     TEXT NOT NULL,
    external_id TEXT NOT NULL,
    url         TEXT,
    source      TEXT,
    fetched_at  TEXT,
    PRIMARY KEY (track_id, service, external_id)
);
CREATE INDEX IF NOT EXISTS idx_hub_track_ext_ids_track ON hub_track_external_ids(track_id);
INSERT INTO hub_track_external_ids (track_id, service, external_id, url, source, fetched_at)
SELECT id, service, service_track_id,
       CASE WHEN service = 'spotify'
            THEN 'https://open.spotify.com/track/' || service_track_id
            ELSE NULL END,
       'ingest', NULL
  FROM hub_tracks
 WHERE service_track_id IS NOT NULL AND service_track_id <> '';
