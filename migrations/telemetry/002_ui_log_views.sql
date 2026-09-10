-- Telemetry full package — UI + log views.
--
-- Companion to 001_events.sql (which is DEPLOYED and must never be edited:
-- its sqlx checksum would break the lan receiver at the next start). This
-- migration only adds VIEWS over the generic `events(type, payload)` rows;
-- no tables, no DDL, no index changes. Events of the new allowlisted types
-- flow through the unchanged ingest path:
--   ui.view.opened      {"view": "<page_id>"}                 (SPA hook)
--   ui.action.*         {"ok": bool, "error_message"?: …}     (API handlers)
--   log.entry           {"level", "target", "message"}        (log shipping)
-- See plans/proposed/telemetry-full-package.md (E1/E5).
--
-- NOTE: like 001, this chain is an independent namespace — never renumber
-- against the main migrations/ directory.

-- Views opened per client per day per page id (payload.view). The page id
-- is client-validated (^[a-z0-9-]{1,64}$) before it ever reaches the wire.
CREATE VIEW v_ui_views AS
SELECT
    client_id,
    date(ts, 'unixepoch')            AS day,
    json_extract(payload, '$.view')  AS view,
    COUNT(*)                         AS views
FROM events
WHERE type = 'ui.view.opened'
GROUP BY client_id, day, view;

-- User-triggered actions per client per day per action type, split by the
-- handler's outcome (payload.ok). Error responses carry a sanitized
-- error_message; lifecycle errors stay in task.failed / error.reported.
CREATE VIEW v_ui_actions AS
SELECT
    client_id,
    date(ts, 'unixepoch')                    AS day,
    type                                     AS action_type,
    json_extract(payload, '$.ok')            AS ok,
    COUNT(*)                                 AS actions
FROM events
WHERE type LIKE 'ui.action.%'
GROUP BY client_id, day, action_type, ok;

-- Shipped log volume per client per day per level (payload.level:
-- error|warn|info|debug|trace — level is data, not a wire type). The
-- client only ships what its log_min_level filter let through.
CREATE VIEW v_log_volume AS
SELECT
    client_id,
    date(ts, 'unixepoch')                    AS day,
    json_extract(payload, '$.level')         AS level,
    COUNT(*)                                 AS entries
FROM events
WHERE type = 'log.entry'
GROUP BY client_id, day, level;
