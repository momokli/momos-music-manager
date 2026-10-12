-- Migration 035: subscription poll cadence 5 min -> 6 h.
--
-- The subscription poller polls every subscribed playlist whose interval has
-- elapsed. At the old 300 s default and ~50 subscriptions that is ~15k
-- `GET /playlists/{id}` calls/day — the dominant Spotify API load and the
-- source of the chronic app-level 429s (multi-hour `Retry-After`).
--
-- 6 h keeps Backpack membership reasonably fresh while cutting that ~72x. Fresh
-- data on demand stays available per playlist via the "sync" action
-- (POST /api/services/spotify/sync/playlists/{id}/tracks).
--
-- Only raises shorter intervals; a longer one a user set is never shortened.

UPDATE playlist_subscriptions SET poll_interval_secs = 21600 WHERE poll_interval_secs < 21600;

SELECT 'Migration 035 applied: subscription poll interval -> 21600s (6h)' as status;
