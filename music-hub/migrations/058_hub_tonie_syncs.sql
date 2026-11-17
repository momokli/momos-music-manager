-- Tonies sync run log (#502, M2 "Sync-Run"): one row per started sync of a
-- Kreativ-Tonie. Additive and small — it only records what the run did, never
-- a credential.
--
-- A row is inserted with `status='running'` **before** the first API write, so
-- every run leaves a trace even when it fails halfway (then `status='error'` +
-- `error`). `finished_at`/`chapters_*`/`seconds`/`added`/`removed`/`skipped_json`
-- are filled by the finishing step. `skipped_json` is a JSON array of
-- `{title,isrc,reason}` — display-only, no secret.
--
-- `binding_id` cascades with the binding, so removing a binding drops its log.
-- No CHECK constraint (validated in `tonies::run`, same convention as 056/057).

CREATE TABLE hub_tonie_syncs (
    id              INTEGER PRIMARY KEY,
    binding_id      INTEGER NOT NULL REFERENCES hub_tonie_bindings(id) ON DELETE CASCADE,
    started_at      TEXT NOT NULL,
    finished_at     TEXT,
    status          TEXT NOT NULL,          -- 'running'|'ok'|'noop'|'timeout'|'error'
    chapters_before INTEGER,
    chapters_after  INTEGER,
    seconds         INTEGER,
    added           INTEGER,
    removed         INTEGER,
    skipped_json    TEXT,
    error           TEXT
);

CREATE INDEX idx_hub_tonie_syncs_binding_started
    ON hub_tonie_syncs(binding_id, started_at DESC);

SELECT 'Migration 058 applied: tonie sync runs' AS status;
