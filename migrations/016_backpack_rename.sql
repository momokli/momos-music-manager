-- Migration 016: Rename tags.followed to tags.backpack
-- Uses SQLite 3.25+ ALTER TABLE RENAME COLUMN syntax.

ALTER TABLE tags RENAME COLUMN followed TO backpack;

SELECT 'Migration 016 applied: tags.followed renamed to tags.backpack' as status;
