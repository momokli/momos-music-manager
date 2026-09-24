-- Migration 030: bounded retries for failed music-api imports.
--
-- `absent` (the service has no streamable match) stays terminal — a track that
-- does not exist on Deezer will never appear. `failed` is different: the common
-- reasons are transport-level ("download timeout", "file not found after
-- download"), which are transient — the service was busy or the download was
-- cut off. Marking those terminal silently drops library tracks the user asked
-- to download.
--
-- A `failed` ISRC is therefore re-ordered until it has failed `attempts` times,
-- then it settles (terminal). `attempts` counts failures across re-orders.

ALTER TABLE music_api_imports ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;

SELECT 'Migration 030 applied: music_api_imports.attempts added' as status;
