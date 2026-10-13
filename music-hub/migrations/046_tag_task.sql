-- Daily/weekly tagging task (#346). A task is the stable, per-period list of
-- uncomplete tracks for one user, most-played first. One task row exists per
-- (user, cadence, period_start); its items are persisted so the queue does not
-- change within a period and is only regenerated at the period boundary.

CREATE TABLE hub_tag_tasks (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    cadence      TEXT NOT NULL CHECK (cadence IN ('daily', 'weekly')),
    -- UTC date (YYYY-MM-DD); daily = calendar day, weekly = ISO-week Monday.
    period_start TEXT NOT NULL,
    generated_at TEXT NOT NULL,
    UNIQUE (user_id, cadence, period_start)
);

CREATE INDEX IF NOT EXISTS idx_hub_tag_tasks_user
    ON hub_tag_tasks(user_id, cadence, period_start);

CREATE TABLE hub_tag_task_items (
    task_id  INTEGER NOT NULL REFERENCES hub_tag_tasks(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES hub_tracks(id)    ON DELETE CASCADE,
    rank     INTEGER NOT NULL,
    plays    INTEGER NOT NULL DEFAULT 0,
    -- Missing describing families, canonical order, comma-separated.
    missing  TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (task_id, track_id)
);

CREATE INDEX IF NOT EXISTS idx_hub_tag_task_items_task
    ON hub_tag_task_items(task_id, rank);
