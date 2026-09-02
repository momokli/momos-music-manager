-- Telemetry event ingest — own migration chain for the server-side
-- `telemetry.db` (separate from the main client DB; `_sqlx_migrations`
-- is per database, so the two chains never conflict).
--
-- Client → `POST /api/telemetry` (Bearer auth), batches of events:
--   task.started/completed/failed, scan.completed, download.started/
--   completed/failed, app.updated, error.reported.
-- No UI actions, no heartbeats — see plans/proposed/telemetry-events.md.
--
-- NOTE: this chain must NEVER be renumbered against the main migrations/
-- directory; it is an independent namespace documented in the concept doc.

-- ── Clients ────────────────────────────────────────────────────────────────
-- One row per stable client_id (first seen + last seen are derived from
-- events; "zuletzt gesehen" view reads last_seen_at without heartbeats).
CREATE TABLE clients (
    client_id        TEXT PRIMARY KEY,
    first_seen_at    INTEGER NOT NULL,   -- unixepoch (server time)
    last_seen_at     INTEGER NOT NULL,   -- unixepoch (server time)
    last_app_version TEXT,
    last_os          TEXT
);

-- ── Events ─────────────────────────────────────────────────────────────────
-- event_id is the dedup key: idempotent retries INSERT OR IGNORE.
CREATE TABLE events (
    event_id    TEXT PRIMARY KEY,
    client_id   TEXT NOT NULL REFERENCES clients(client_id),
    type        TEXT NOT NULL,           -- dotted allowlist, e.g. task.completed
    ts          INTEGER NOT NULL,        -- unixepoch (client clock)
    received_at INTEGER NOT NULL,        -- unixepoch (server clock; retention basis)
    app_version TEXT NOT NULL,
    os          TEXT NOT NULL,
    payload     TEXT NOT NULL            -- JSON object (redacted)
);

CREATE INDEX idx_events_ts         ON events(ts);
CREATE INDEX idx_events_client_ts  ON events(client_id, ts);
CREATE INDEX idx_events_type_ts    ON events(type, ts);
CREATE INDEX idx_events_received   ON events(received_at);
