-- Migration 009: File lifecycle management
-- Adds: file_locations table, followed on tags, source_of on files, scan_sources + backup_path on folders

-- file_locations: tracks where a file physically exists (local vs backup)
CREATE TABLE IF NOT EXISTS file_locations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    location_type TEXT NOT NULL CHECK (location_type IN ('local', 'backup')),
    path TEXT NOT NULL,
    file_size INTEGER,
    last_verified INTEGER,
    created_at INTEGER DEFAULT (unixepoch()),
    UNIQUE(file_id, location_type)
);

-- tags: add followed flag (for tag-based file presence policies)
ALTER TABLE tags ADD COLUMN followed BOOLEAN NOT NULL DEFAULT 0;

-- files: add source_of for WAV→stem parent linking (WAV source subdirectory → stem.m4a)
-- source_of references the stem file's id in the files table
ALTER TABLE files ADD COLUMN source_of INTEGER REFERENCES files(id);

-- folders: opt-in to scan WAV source subdirectories + backup destination path
ALTER TABLE folders ADD COLUMN scan_sources BOOLEAN NOT NULL DEFAULT 0;
ALTER TABLE folders ADD COLUMN backup_path TEXT;

-- Indexes for performance
CREATE INDEX IF NOT EXISTS idx_file_locations_file_id ON file_locations(file_id);
CREATE INDEX IF NOT EXISTS idx_file_locations_type ON file_locations(location_type);
CREATE INDEX IF NOT EXISTS idx_files_source_of ON files(source_of);
CREATE INDEX IF NOT EXISTS idx_tags_followed ON tags(followed);

SELECT 'Migration 009 applied: file lifecycle management' as status;
