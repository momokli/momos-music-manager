-- Migration 020: Add keys, rating_min, play_count_min to dynamic_bundles
-- Matches the filter dimensions available on the Tracks page.

ALTER TABLE dynamic_bundles ADD COLUMN keys TEXT;             -- JSON array of Camelot keys, e.g. '["4m","5m","8m"]'
ALTER TABLE dynamic_bundles ADD COLUMN rating_min INTEGER;   -- Minimum rating (0-5)
ALTER TABLE dynamic_bundles ADD COLUMN play_count_min INTEGER; -- Minimum play count

SELECT 'Migration 020 applied: added key/rating/play_count filters to dynamic_bundles' as status;
