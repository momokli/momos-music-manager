ALTER TABLE files ADD COLUMN folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_files_folder_id ON files(folder_id);

-- Temporarily disable FK enforcement for the backfill.
-- The files table was created before folders existed in some migration
-- environments, so FK checks would fail on UPDATE even though target rows exist.
PRAGMA foreign_keys = OFF;

-- Backfill: set folder_id for all existing files based on path prefix.
-- A file belongs to the longest-matching folder path.
UPDATE files
SET folder_id = (
    SELECT fol.id FROM folders fol
    WHERE files.file_path LIKE (fol.folder_path || '/%')
       OR files.file_path = fol.folder_path
    ORDER BY length(fol.folder_path) DESC
    LIMIT 1
);

PRAGMA foreign_keys = ON;

SELECT 'Migration 021 applied: folder_id on files with backfill' as status;
