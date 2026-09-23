-- Migration 024: App-Settings (KV) — z. B. Autoupdate-Toggle + letzter Check
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
SELECT 'Migration 024 applied: settings KV table' as status;
