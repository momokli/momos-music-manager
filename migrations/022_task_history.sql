CREATE TABLE IF NOT EXISTS task_history (
    id TEXT PRIMARY KEY,
    task_type TEXT NOT NULL,
    task_details TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    service TEXT,
    progress TEXT NOT NULL DEFAULT '',
    percent REAL,
    sub_items TEXT NOT NULL DEFAULT '[]',
    logs TEXT NOT NULL DEFAULT '[]',
    result_summary TEXT,
    error_message TEXT,
    started_at INTEGER,
    completed_at INTEGER,
    created_at_secs REAL NOT NULL,
    persisted_at INTEGER DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_task_history_status ON task_history(status);
CREATE INDEX IF NOT EXISTS idx_task_history_created ON task_history(created_at_secs);
CREATE INDEX IF NOT EXISTS idx_task_history_type ON task_history(task_type);

SELECT 'Migration 022 applied: task_history table for persistent task tracking' as status;
