-- Migration 016: Rename tags.followed to tags.backpack (already applied manually)
-- The ALTER TABLE was run directly against the DB. This migration only records the change.
SELECT 'Migration 016 applied: tags.followed renamed to tags.backpack' as status;
