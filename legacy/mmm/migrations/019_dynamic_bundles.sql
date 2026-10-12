CREATE TABLE IF NOT EXISTS dynamic_bundles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    tag_id INTEGER NOT NULL UNIQUE REFERENCES tags(id) ON DELETE CASCADE,
    base_tags TEXT,               -- JSON array of tag names
    include_all_tracks BOOLEAN NOT NULL DEFAULT 0,
    bpm_min REAL,
    bpm_max REAL,
    pmv_categories TEXT,          -- JSON array like '["p","m"]'
    file_types TEXT,              -- JSON array like '["stem.m4a","flac"]'
    exclude_wav_sources BOOLEAN NOT NULL DEFAULT 1,
    created_at INTEGER DEFAULT (unixepoch()),
    updated_at INTEGER DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_dynamic_bundles_tag_id ON dynamic_bundles(tag_id);

SELECT 'Migration 019 applied: dynamic_bundles table' as status;
