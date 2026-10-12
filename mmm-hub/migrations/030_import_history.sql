-- Import history (plan H1–H3, issues #224–#226).
--
-- H1: a ledger of every import run, across all sources (spotify sync, traktor
--     collection upload, later soundcloud/youtube). One row per run.
-- H2: change events between two imports — added/removed/changed entities
--     (playlist membership, track meta deltas, traktor playcount/rating/…),
--     cross-source. One row per detected change.

CREATE TABLE hub_import_runs (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    source      TEXT NOT NULL,                 -- 'spotify' | 'traktor' | 'soundcloud' | 'youtube' | …
    started_at  TEXT NOT NULL,                 -- ISO-8601
    finished_at TEXT,                          -- NULL while running
    status      TEXT NOT NULL DEFAULT 'running', -- running | ok | error | partial
    stats       TEXT                           -- JSON blob (counts, source metadata)
);

CREATE INDEX idx_hub_import_runs_user   ON hub_import_runs(user_id, started_at);
CREATE INDEX idx_hub_import_runs_source ON hub_import_runs(source, started_at);

CREATE TABLE hub_import_events (
    id          INTEGER PRIMARY KEY,
    run_id      INTEGER NOT NULL REFERENCES hub_import_runs(id) ON DELETE CASCADE,
    user_id     INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    source      TEXT NOT NULL,
    entity_type TEXT NOT NULL,                 -- 'playlist_track' | 'track' | 'traktor_playcount' | …
    entity_ref  TEXT NOT NULL,                 -- stable reference (id/slug) within entity_type
    change      TEXT NOT NULL CHECK (change IN ('added', 'removed', 'changed')),
    before      TEXT,                          -- previous value/serialized entity (NULL for added)
    after       TEXT,                          -- new value/serialized entity (NULL for removed)
    at          TEXT NOT NULL
);

CREATE INDEX idx_hub_import_events_run    ON hub_import_events(run_id);
CREATE INDEX idx_hub_import_events_scope  ON hub_import_events(user_id, source, at);
CREATE INDEX idx_hub_import_events_entity ON hub_import_events(entity_type, entity_ref);

SELECT 'Migration 030 applied: import-run ledger + change events' as status;
