-- Migration 007: No-op
--
-- The snapshot_id column was consolidated into migration 006_local_service.sql.
-- This file exists so that databases which already applied the original
-- migration 007 do not break on startup.
--
-- Future fresh installs will apply 001 → 002 → 003 → 004 → 005 → 006 and
-- get snapshot_id from migration 006.

SELECT 'Migration 007 no-op: snapshot_id already in service_playlists (consolidated into 006)' as status;
