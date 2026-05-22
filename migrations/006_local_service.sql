-- Migration 006: Add local service support
--
-- Changes:
--   1. Update service_tracks CHECK constraint to allow 'local' service
--   2. Update v_file_track_link to match service='local' tracks
--   3. Recreate dependent views (v_file_tags, v_file_resolved_tags, v_tag_file_counts)
--
-- A local service_track has service='local' and service_id=CAST(file.id AS TEXT),
-- enabling the digging workflow to persist suggestions as playlists.

-- ============================================================
-- Step 1: Update service_tracks CHECK constraint to allow 'local'
-- ============================================================
-- SQLite cannot ALTER CHECK constraints, so we recreate the table.

CREATE TABLE service_tracks_v2 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    service TEXT NOT NULL CHECK (service IN ('spotify', 'soundcloud', 'youtube', 'deemix', 'local')),
    service_id TEXT NOT NULL,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album TEXT,
    isrc TEXT,
    duration_ms INTEGER,
    metadata_json TEXT,
    imported_at INTEGER DEFAULT (unixepoch()),
    updated_at INTEGER DEFAULT (unixepoch()),
    UNIQUE(service, service_id)
);

INSERT INTO service_tracks_v2
    (id, service, service_id, title, artist, album, isrc, duration_ms, metadata_json, imported_at, updated_at)
SELECT id, service, service_id, title, artist, album, isrc, duration_ms, metadata_json, imported_at, updated_at
FROM service_tracks;

DROP TABLE service_tracks;
ALTER TABLE service_tracks_v2 RENAME TO service_tracks;

-- Recreate indexes
CREATE INDEX IF NOT EXISTS idx_service_tracks_service_id ON service_tracks(service, service_id);
CREATE INDEX IF NOT EXISTS idx_service_tracks_isrc ON service_tracks(isrc);

-- ============================================================
-- Step 2: Drop views that depend on v_file_track_link
-- ============================================================

DROP VIEW IF EXISTS v_tag_file_counts;
DROP VIEW IF EXISTS v_file_resolved_tags;
DROP VIEW IF EXISTS v_file_tags;

-- ============================================================
-- Step 3: Recreate v_file_track_link with local service match
-- ============================================================

DROP VIEW IF EXISTS v_file_track_link;
CREATE VIEW v_file_track_link AS
SELECT f.id AS file_id, st.id AS track_id
FROM files f
JOIN service_tracks st ON (
    st.isrc = f.isrc
    OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
    OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
    OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
    OR (st.service = 'local' AND st.service_id = CAST(f.id AS TEXT))
);

-- ============================================================
-- Step 4: Recreate v_file_tags (from migration 004)
-- ============================================================

CREATE VIEW v_file_tags AS
SELECT DISTINCT f.id AS file_id,
       t.id AS tag_id, t.name AS tag_name,
       t.sort_order, t.created_at,
       tc.id AS category_id, tc.name AS category_name,
       tc.is_default, tc.prefix
FROM files f
JOIN v_file_track_link v ON v.file_id = f.id
JOIN service_playlist_tracks spt ON spt.track_id = v.track_id
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN tag_categories tc ON tc.id = t.category_id;

-- ============================================================
-- Step 5: Recreate v_file_resolved_tags (from migration 004)
-- ============================================================

CREATE VIEW v_file_resolved_tags AS
SELECT DISTINCT
    f.id AS file_id,
    rt.tag_id,
    rt.tag_name,
    rt.sort_order,
    rt.created_at,
    rt.category_id,
    rt.category_name,
    rt.prefix
FROM files f
JOIN v_file_track_link v ON v.file_id = f.id
JOIN service_playlist_tracks spt ON spt.track_id = v.track_id
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN v_resolved_tags rt ON rt.source_tag_id = t.id;

-- ============================================================
-- Step 6: Recreate v_tag_file_counts (from migration 004)
-- ============================================================

CREATE VIEW v_tag_file_counts AS
SELECT vft.tag_id, COUNT(DISTINCT vft.file_id) AS file_count
FROM v_file_tags vft
GROUP BY vft.tag_id;

-- ============================================================
-- Verification
-- ============================================================

SELECT CASE
    WHEN (SELECT COUNT(*) FROM sqlite_master WHERE type = 'view' AND name = 'v_file_track_link') = 1
    THEN 'OK'
    ELSE 'FAIL'
END AS check_v_file_track_link;

SELECT CASE
    WHEN (SELECT COUNT(*) FROM sqlite_master WHERE type = 'view' AND name = 'v_file_tags') = 1
    THEN 'OK'
    ELSE 'FAIL'
END AS check_v_file_tags;

SELECT CASE
    WHEN (SELECT COUNT(*) FROM sqlite_master WHERE type = 'view' AND name = 'v_file_resolved_tags') = 1
    THEN 'OK'
    ELSE 'FAIL'
END AS check_v_file_resolved_tags;

SELECT CASE
    WHEN (SELECT COUNT(*) FROM sqlite_master WHERE type = 'view' AND name = 'v_tag_file_counts') = 1
    THEN 'OK'
    ELSE 'FAIL'
END AS check_v_tag_file_counts;

SELECT 'Migration 006 applied: local service support' as status;
