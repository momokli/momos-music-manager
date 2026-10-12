-- Migration 028: content hash of the canonicalised file (remote object store).
--
-- The remote object store keys objects by the SHA-256 of the *canonical* file
-- (the file with MMM's Comment tag cleared). `content_hash` records that hash
-- for every file MMM has canonicalised, so the store sync task can ask the
-- store `POST /objects/check` whether an object is present without recomputing
-- it, and so `file_locations('backup', 'store:<hash>')` is a real hash fact.
--
-- `files.file_hash` is unrelated: it is a `size-mtime` change detector, not a
-- content digest. This is a new, additive column.

ALTER TABLE files ADD COLUMN content_hash TEXT;
CREATE INDEX IF NOT EXISTS idx_files_content_hash ON files(content_hash);

SELECT 'Migration 028 applied: files.content_hash added' as status;
