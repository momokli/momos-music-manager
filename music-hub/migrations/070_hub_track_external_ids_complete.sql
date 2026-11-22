-- Complete the track -> external-id satellite and stop it from drifting again.
--
-- Playlist and likes ingest wrote `hub_tracks` but not `hub_track_external_ids`,
-- so every track first seen through those paths had no external id row (3.596 on
-- prod). Discovery, the feature backfill and the track page look the id up in
-- this table, so those tracks were invisible to them.
--
-- 1. Backfill one row per track that still lacks its own id.
-- 2. Enforce one owner per (service, external_id).

INSERT OR IGNORE INTO hub_track_external_ids
    (track_id, service, external_id, url, source, fetched_at)
SELECT t.id,
       t.service,
       t.service_track_id,
       CASE WHEN t.service = 'spotify'
            THEN 'https://open.spotify.com/track/' || t.service_track_id
            ELSE NULL END,
       'backfill',
       t.first_seen_at
  FROM hub_tracks t
 WHERE t.service_track_id IS NOT NULL
   AND TRIM(t.service_track_id) <> ''
   AND NOT EXISTS (
       SELECT 1
         FROM hub_track_external_ids e
        WHERE e.track_id = t.id
          AND e.service = t.service
          AND e.external_id = t.service_track_id
   );

-- `hub_tracks(service, service_track_id)` is already unique, so the same
-- service id cannot legitimately belong to two tracks.
CREATE UNIQUE INDEX IF NOT EXISTS ux_hub_track_ext_ids_service_external
    ON hub_track_external_ids(service, external_id);
