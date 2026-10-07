-- Migration 033: rediscovery_pushes ledger.
--
-- Records every time a forgotten track was pushed back to a listener
-- (Issue #79, Milestone 1.15.0, feature M1: v_track_forgotten_facts from 032).
-- The ledger lets the rediscovery scheduler avoid re-pushing the same track
-- before its cooldown has elapsed.
--
-- FK semantics:
--   * service_tracks       -> ON DELETE CASCADE   (hard-deleting a track must
--     drop its push history; a track with no row cannot be re-pushed).
--   * service_playlists    -> ON DELETE SET NULL  (a deleted/generated playlist
--     pack must NOT erase history; the push happened, we just lose the pointer
--     to the pack). playlist_id is therefore NULLABLE — SQLite aborts an
--     `ON DELETE SET NULL` action when the child column is NOT NULL.
--
-- NB: SQLite only enforces FK actions with `PRAGMA foreign_keys=ON`
-- (sqlx enables it by default; integration tests set it explicitly).

CREATE TABLE IF NOT EXISTS rediscovery_pushes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    track_id    INTEGER NOT NULL REFERENCES service_tracks(id) ON DELETE CASCADE,
    playlist_id INTEGER REFERENCES service_playlists(id) ON DELETE SET NULL,
    pushed_at   INTEGER NOT NULL,
    facet_json  TEXT,
    slot        INTEGER
);

CREATE INDEX IF NOT EXISTS idx_rediscovery_pushes_track ON rediscovery_pushes(track_id, pushed_at);
CREATE INDEX IF NOT EXISTS idx_rediscovery_pushes_slot  ON rediscovery_pushes(slot, pushed_at);

SELECT 'Migration 033 applied: rediscovery_pushes ledger' as status;
