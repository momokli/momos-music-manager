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

-- ── Views (evaluation surface — no dashboard in this PR) ───────────────────

-- Tasks per hour per client per task_type (payload.task_type), split by
-- lifecycle status (task.started/completed/failed).
CREATE VIEW v_tasks_per_hour AS
SELECT
    client_id,
    strftime('%Y-%m-%d %H:00', ts, 'unixepoch') AS hour,
    json_extract(payload, '$.task_type')        AS task_type,
    type                                        AS event_type,
    COUNT(*)                                    AS events
FROM events
WHERE type LIKE 'task.%'
GROUP BY client_id, hour, task_type, event_type;

-- Error rate per client per day: task.failed vs task.completed. Cancelled
-- tasks are deliberately not counted (no task.cancelled event type).
CREATE VIEW v_error_rate AS
SELECT
    client_id,
    date(ts, 'unixepoch') AS day,
    SUM(CASE WHEN type = 'task.failed'    THEN 1 ELSE 0 END) AS failed,
    SUM(CASE WHEN type = 'task.completed' THEN 1 ELSE 0 END) AS completed,
    ROUND(
        100.0 * SUM(CASE WHEN type = 'task.failed' THEN 1 ELSE 0 END)
        / NULLIF(SUM(CASE WHEN type = 'task.completed' THEN 1 ELSE 0 END), 0),
        1
    ) AS error_rate_pct
FROM events
WHERE type IN ('task.failed', 'task.completed')
GROUP BY client_id, day;

-- Downloads per source (payload.source: deemix | spotdl) per day, split by
-- lifecycle status (download.started/completed/failed).
CREATE VIEW v_downloads_by_source AS
SELECT
    client_id,
    json_extract(payload, '$.source') AS source,
    type                             AS event_type,
    date(ts, 'unixepoch')            AS day,
    COUNT(*)                         AS events
FROM events
WHERE type LIKE 'download.%'
GROUP BY client_id, source, event_type, day;

-- Scan duration trend per client per day: avg + p95 of payload.duration_ms
-- (scan.completed events). p95 via sorted window offset.
CREATE VIEW v_scan_duration_trend AS
SELECT
    e.client_id,
    date(e.ts, 'unixepoch') AS day,
    COUNT(*) AS scans,
    ROUND(AVG(CAST(json_extract(e.payload, '$.duration_ms') AS INTEGER)), 0)
        AS avg_duration_ms,
    (
        SELECT CAST(json_extract(d.payload, '$.duration_ms') AS INTEGER)
        FROM events d
        WHERE d.type = 'scan.completed'
          AND d.client_id = e.client_id
          AND date(d.ts, 'unixepoch') = date(e.ts, 'unixepoch')
        ORDER BY CAST(json_extract(d.payload, '$.duration_ms') AS INTEGER) ASC
        LIMIT 1
        OFFSET MAX(
            (SELECT COUNT(*) FROM events c
              WHERE c.type = 'scan.completed'
                AND c.client_id = e.client_id
                AND date(c.ts, 'unixepoch') = date(e.ts, 'unixepoch')) * 95 / 100 - 1,
            0
        )
    ) AS p95_duration_ms
FROM events e
WHERE e.type = 'scan.completed'
GROUP BY e.client_id, date(e.ts, 'unixepoch');

-- Current app version per client (refreshed on every ingest).
CREATE VIEW v_client_versions AS
SELECT
    client_id,
    last_app_version,
    last_os,
    first_seen_at,
    last_seen_at
FROM clients;

-- "Zuletzt gesehen" per client — event-driven (no heartbeats by design):
-- a quiet client (no tasks/scans/downloads) ages out of this view.
CREATE VIEW v_clients_last_seen AS
SELECT
    client_id,
    first_seen_at,
    last_seen_at,
    datetime(last_seen_at, 'unixepoch') AS last_seen_iso
FROM clients;
