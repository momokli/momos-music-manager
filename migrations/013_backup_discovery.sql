-- Migration 013: Track local file presence + backup discovery support

-- Add last_verified_local to files for tracking when the file was last confirmed on disk
-- NULL = never been local (backup-only), timestamp = last confirmed on disk
ALTER TABLE files ADD COLUMN last_verified_local INTEGER;

-- Note: DO NOT backfill file_locations.local entries here!
-- The scanner populates them on next scan (see scan_and_store_file changes).
-- A blind backfill would mark deleted files as "local" -- false positives.

SELECT 'Migration 013 applied: local file tracking + backup discovery support' as status;
