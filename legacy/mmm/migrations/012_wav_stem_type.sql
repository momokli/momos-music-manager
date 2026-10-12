-- Migration 012: Add stem_type column for nuo-stems WAV source files
-- WAV source files (vocals/bass/drums/instrumental/other) are linked to parent stem.m4a via source_of

ALTER TABLE files ADD COLUMN stem_type TEXT CHECK (
    stem_type IS NULL OR stem_type IN ('vocals', 'bass', 'drums', 'instrumental', 'other')
);

CREATE INDEX IF NOT EXISTS idx_files_stem_type ON files(stem_type);

SELECT 'Migration 012 applied: stem_type column on files' as status;
