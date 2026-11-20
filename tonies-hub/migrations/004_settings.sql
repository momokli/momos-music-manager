-- Key/value settings. DB values are only a fallback: env always wins.
-- No secret seeding — values are written by the admin UI/CLI.
CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

SELECT 'Migration 004 applied: settings' AS status;
