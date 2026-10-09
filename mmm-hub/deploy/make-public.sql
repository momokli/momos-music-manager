-- Builds the sanitized read-only copy consumed by Datasette / SchemaSpy.
-- Never expose hub.db directly: the copy drops secrets.
--
--   ATTACH the live DB, then materialize each table/view.
--   Excluded: hub_users.password_hash, hub_service_accounts.{access,refresh}_token,
--             the whole hub_web_sessions table.

ATTACH '/home/momo/mmm-hub/hub.db' AS src;

CREATE TABLE hub_users AS
  SELECT id, slug, display_name, created_at FROM src.hub_users;

CREATE TABLE hub_service_accounts AS
  SELECT id, user_id, service, remote_user_id, display_name, authorized_at,
         connected_at, updated_at, likes_status, likes_synced_at, likes_error
    FROM src.hub_service_accounts;

CREATE TABLE hub_tracks AS
  SELECT id, service, service_track_id, isrc, title, artists, album, duration_ms,
         explicit, image_url, first_seen_at
    FROM src.hub_tracks;

CREATE TABLE hub_track_external_ids AS
  SELECT track_id, service, external_id, url, source, fetched_at
    FROM src.hub_track_external_ids;

CREATE TABLE hub_playlists      AS SELECT * FROM src.hub_playlists;
CREATE TABLE hub_playlist_tracks AS SELECT * FROM src.hub_playlist_tracks;
CREATE TABLE hub_liked_tracks    AS SELECT * FROM src.hub_liked_tracks;

CREATE TABLE hub_v_track_presence AS
  SELECT hlt.track_id AS track_id, hlt.user_id AS user_id, 'liked' AS source,
         NULL AS playlist_id, NULL AS playlist_name, hlt.liked_at AS added_at
    FROM hub_liked_tracks hlt
  UNION ALL
  SELECT hpt.track_id, hp.user_id, 'playlist', hp.id, hp.name, hpt.added_at
    FROM hub_playlist_tracks hpt JOIN hub_playlists hp ON hp.id = hpt.playlist_id;

CREATE TABLE hub_v_track_playlists AS
  SELECT hpt.track_id AS track_id, hp.user_id AS user_id, hp.id AS playlist_id, hp.name AS playlist_name
    FROM hub_playlist_tracks hpt JOIN hub_playlists hp ON hp.id = hpt.playlist_id;

CREATE TABLE hub_v_shared_tracks AS
  WITH presence AS (
    SELECT track_id, user_id FROM hub_liked_tracks
    UNION
    SELECT hpt.track_id, hp.user_id FROM hub_playlist_tracks hpt JOIN hub_playlists hp ON hp.id = hpt.playlist_id
  )
  SELECT track_id, COUNT(DISTINCT user_id) AS user_count, GROUP_CONCAT(DISTINCT user_id) AS user_ids
    FROM presence GROUP BY track_id HAVING COUNT(DISTINCT user_id) >= 2;

CREATE TABLE hub_v_user_overlap AS
  WITH presence AS (
    SELECT track_id, user_id FROM hub_liked_tracks
    UNION
    SELECT hpt.track_id, hp.user_id FROM hub_playlist_tracks hpt JOIN hub_playlists hp ON hp.id = hpt.playlist_id
  ), distinct_presence AS (
    SELECT DISTINCT track_id, user_id FROM presence
  )
  SELECT a.user_id AS user_a_id, b.user_id AS user_b_id, COUNT(*) AS shared_tracks
    FROM distinct_presence a
    JOIN distinct_presence b ON a.track_id = b.track_id AND a.user_id < b.user_id
   GROUP BY a.user_id, b.user_id;
