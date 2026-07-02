-- Migration 023: Add 'tidal' to service_config CHECK constraint.
-- SQLite doesn't support ALTER TABLE ... ALTER CONSTRAINT, so we recreate the table.

-- Create new table with updated CHECK constraint
CREATE TABLE service_config_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    service TEXT NOT NULL CHECK (service IN ('spotify', 'soundcloud', 'youtube', 'deemix', 'tidal')),
    refresh_token TEXT,
    metadata_json TEXT,
    access_token TEXT,
    token_expiry INTEGER,
    user_id TEXT,
    playlist_id TEXT,
    is_connected BOOLEAN NOT NULL DEFAULT 0,
    last_checked INTEGER,
    last_synced INTEGER,
    remote_playlists_count INTEGER NOT NULL DEFAULT 0,
        remote_tracks_count INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(service)
);

-- Copy existing data
INSERT INTO service_config_new SELECT * FROM service_config;

-- Drop old table and rename
DROP TABLE service_config;
ALTER TABLE service_config_new RENAME TO service_config;

SELECT 'Migration 023 applied: added tidal to service_config CHECK constraint' as status;
