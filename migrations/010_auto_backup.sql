ALTER TABLE folders ADD COLUMN auto_backup BOOLEAN NOT NULL DEFAULT 1;

SELECT 'Migration 010 applied: auto_backup column on folders' as status;
