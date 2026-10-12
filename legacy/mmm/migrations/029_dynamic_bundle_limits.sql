-- Dynamic bundle limits + Camelot diversification.
--
-- `limit_count`  — cap the bundle to the top N files (NULL = all).
-- `rank_by`      — scoring for "top": 'rating_playcount' (default), 'rating',
--                  'playcount', 'recent', 'none'.
-- `diversify_keys` — round-robin the selection across the 24 Camelot keys
--                    (plus a '(none)' bucket) so each key is represented, then
--                    fill the remaining slots by global score.
ALTER TABLE dynamic_bundles ADD COLUMN limit_count INTEGER;
ALTER TABLE dynamic_bundles ADD COLUMN rank_by TEXT;
ALTER TABLE dynamic_bundles ADD COLUMN diversify_keys BOOLEAN NOT NULL DEFAULT 0;
