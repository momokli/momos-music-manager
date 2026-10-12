-- Migration 004: Make tags.name UNIQUE COLLATE NOCASE — deduplicate and remap FKs
--
-- Problem:
--   tags.name has UNIQUE COLLATE BINARY (default), which allows case-different
--   duplicates like "Groovy" and "groovy". Since all tag↔playlist matching is
--   case-insensitive (via LOWER(TRIM(...))), these duplicates are functionally
--   identical but cause cartesian products in JOINs (e.g. playlists page shows
--   2× the real track count).
--
-- Solution:
--   1. Create tags_v2 with name TEXT NOT NULL UNIQUE COLLATE NOCASE
--   2. Deduplicate: copy distinct lowercased names, keeping lowest id
--   3. Remap FKs in tag_parents, tag_embeddings, tag_energy_levels, tag_similarities
--   4. Drop old tags, rename tags_v2 → tags
--   5. Recreate indexes
--   6. Verify no orphan FKs

-- ============================================================
-- Step 1: Create tags_v2 with COLLATE NOCASE
-- ============================================================

CREATE TABLE tags_v2 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    category_id INTEGER NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER DEFAULT (unixepoch()),
    reviewed_at INTEGER,
    FOREIGN KEY (category_id) REFERENCES tag_categories(id) ON DELETE CASCADE
);

-- ============================================================
-- Step 2: Copy deduplicated tags
-- Keep the row with the lowest id for each case-insensitive name.
-- Known duplicate: "groovy" (id 286) → "Groovy" (id 88) — both Vibe.
-- ============================================================

INSERT INTO tags_v2 (id, name, category_id, sort_order, created_at, reviewed_at)
SELECT MIN(id), name, category_id, sort_order, created_at, reviewed_at
FROM tags
GROUP BY LOWER(TRIM(name));

-- ============================================================
-- Step 3: Remap foreign keys
--
-- Build a mapping from old duplicate tag IDs → surviving tag ID.
-- Old IDs that don't appear in tags_v2 (duplicates) need their FK references
-- redirected to the surviving id.
-- ============================================================

-- Create a temporary remapping table for clarity
CREATE TEMP TABLE tag_remap AS
SELECT old.id AS old_id, new.id AS new_id
FROM tags old
JOIN tags_v2 new ON LOWER(TRIM(old.name)) = LOWER(TRIM(new.name))
WHERE old.id != new.id;

-- Remap tag_parents.tag_id
UPDATE tag_parents
SET tag_id = (
    SELECT new_id FROM tag_remap WHERE old_id = tag_id
)
WHERE tag_id IN (SELECT old_id FROM tag_remap);

-- Remap tag_parents.parent_tag_id
UPDATE tag_parents
SET parent_tag_id = (
    SELECT new_id FROM tag_remap WHERE old_id = parent_tag_id
)
WHERE parent_tag_id IN (SELECT old_id FROM tag_remap);

-- Remap tag_embeddings.tag_id
UPDATE tag_embeddings
SET tag_id = (
    SELECT new_id FROM tag_remap WHERE old_id = tag_id
)
WHERE tag_id IN (SELECT old_id FROM tag_remap);

-- Remap tag_energy_levels.tag_id
UPDATE tag_energy_levels
SET tag_id = (
    SELECT new_id FROM tag_remap WHERE old_id = tag_id
)
WHERE tag_id IN (SELECT old_id FROM tag_remap);

-- Remap tag_similarities.tag_id (tag_a_id)
UPDATE tag_similarities
SET tag_a_id = (
    SELECT new_id FROM tag_remap WHERE old_id = tag_a_id
)
WHERE tag_a_id IN (SELECT old_id FROM tag_remap);

-- Remap tag_similarities.similar_tag_id (tag_b_id)
UPDATE tag_similarities
SET tag_b_id = (
    SELECT new_id FROM tag_remap WHERE old_id = tag_b_id
)
WHERE tag_b_id IN (SELECT old_id FROM tag_remap);

-- Clean up temp table
DROP TABLE IF EXISTS tag_remap;

-- ============================================================
-- Step 4: Drop dependent views (they reference `tags`)
-- ============================================================

DROP VIEW IF EXISTS v_file_resolved_tags;
DROP VIEW IF EXISTS v_resolved_tags;
DROP VIEW IF EXISTS v_tag_file_counts;
DROP VIEW IF EXISTS v_file_tags;
DROP VIEW IF EXISTS v_tags_with_categories;
DROP VIEW IF EXISTS v_tag_categories;
DROP VIEW IF EXISTS v_tag_playlist;

-- ============================================================
-- Step 5: Drop old tags table and rename tags_v2 → tags
-- ============================================================

DROP TABLE tags;
ALTER TABLE tags_v2 RENAME TO tags;

-- ============================================================
-- Step 6: Recreate indexes
-- ============================================================

CREATE INDEX idx_tags_name ON tags(name);
CREATE INDEX idx_tags_category_id ON tags(category_id);

-- ============================================================
-- Step 7: Recreate dependent views
-- ============================================================

CREATE VIEW v_tag_playlist AS
SELECT t.id AS tag_id, t.name AS tag_name, t.category_id,
       sp.id AS playlist_id, sp.name AS playlist_name, sp.service
FROM tags t
JOIN service_playlists sp ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name));

CREATE VIEW v_tag_categories AS
SELECT tc.*, (SELECT COUNT(*) FROM tags WHERE category_id = tc.id) as tag_count
FROM tag_categories tc;

CREATE VIEW v_tags_with_categories AS
SELECT t.id, t.name, t.category_id, t.sort_order, t.created_at, t.reviewed_at,
       tc.name as category, tc.icon as category_icon
FROM tags t
LEFT JOIN tag_categories tc ON t.category_id = tc.id;

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

CREATE VIEW v_tag_file_counts AS
SELECT vft.tag_id, COUNT(DISTINCT vft.file_id) AS file_count
FROM v_file_tags vft
GROUP BY vft.tag_id;

CREATE VIEW v_resolved_tags AS
SELECT DISTINCT
    tp.tag_id AS source_tag_id,
    t_parent.id AS tag_id,
    t_parent.name AS tag_name,
    tc_parent.id AS category_id,
    tc_parent.name AS category_name,
    tc_parent.prefix,
    tc_parent.sort_order,
    t_parent.created_at
FROM tag_parents tp
JOIN tags t_parent ON t_parent.id = tp.parent_tag_id
JOIN tag_categories tc_parent ON tc_parent.id = t_parent.category_id

UNION ALL

SELECT DISTINCT
    t.id AS source_tag_id,
    t.id AS tag_id,
    t.name AS tag_name,
    tc.id AS category_id,
    tc.name AS category_name,
    tc.prefix,
    tc.sort_order,
    t.created_at
FROM tags t
JOIN tag_categories tc ON tc.id = t.category_id
WHERE NOT EXISTS (
    SELECT 1 FROM tag_parents tp WHERE tp.tag_id = t.id
);

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
-- Step 8: Verify no orphan FKs and no duplicate names remain
-- ============================================================

-- Verify duplicate 'groovy' is gone (COLLATE BINARY to avoid NOCASE matching 'Groovy')
SELECT CASE
    WHEN (SELECT COUNT(*) FROM tags WHERE name = 'groovy' COLLATE BINARY) = 0
    THEN 'OK'
    ELSE 'FAIL'
END AS check_groovy_removed;

-- Verify 'Groovy' survives
SELECT CASE
    WHEN (SELECT COUNT(*) FROM tags WHERE name = 'Groovy') = 1
    THEN 'OK'
    ELSE 'FAIL'
END AS check_groovy_survives;

-- Verify no orphan FKs in tag_parents.tag_id
SELECT CASE
    WHEN (SELECT COUNT(*) FROM tag_parents tp LEFT JOIN tags t ON t.id = tp.tag_id WHERE t.id IS NULL) = 0
    THEN 'OK'
    ELSE 'FAIL'
END AS check_tag_parents_tag_id_orphans;

-- Verify no orphan FKs in tag_parents.parent_tag_id
SELECT CASE
    WHEN (SELECT COUNT(*) FROM tag_parents tp LEFT JOIN tags t ON t.id = tp.parent_tag_id WHERE t.id IS NULL) = 0
    THEN 'OK'
    ELSE 'FAIL'
END AS check_tag_parents_parent_tag_id_orphans;

-- Verify no orphan FKs in tag_embeddings
SELECT CASE
    WHEN (SELECT COUNT(*) FROM tag_embeddings te LEFT JOIN tags t ON t.id = te.tag_id WHERE t.id IS NULL) = 0
    THEN 'OK'
    ELSE 'FAIL'
END AS check_tag_embeddings_orphans;

-- Verify no orphan FKs in tag_energy_levels
SELECT CASE
    WHEN (SELECT COUNT(*) FROM tag_energy_levels tel LEFT JOIN tags t ON t.id = tel.tag_id WHERE t.id IS NULL) = 0
    THEN 'OK'
    ELSE 'FAIL'
END AS check_tag_energy_levels_orphans;

-- Verify no orphan FKs in tag_similarities (tag_a_id)
SELECT CASE
    WHEN (SELECT COUNT(*) FROM tag_similarities ts LEFT JOIN tags t ON t.id = ts.tag_a_id WHERE t.id IS NULL) = 0
    THEN 'OK'
    ELSE 'FAIL'
END AS check_tag_similarities_tag_a_id_orphans;

-- Verify no orphan FKs in tag_similarities (tag_b_id)
SELECT CASE
    WHEN (SELECT COUNT(*) FROM tag_similarities ts LEFT JOIN tags t ON t.id = ts.tag_b_id WHERE t.id IS NULL) = 0
    THEN 'OK'
    ELSE 'FAIL'
END AS check_tag_similarities_tag_b_id_orphans;

-- ============================================================
-- Status
-- ============================================================

SELECT 'Migration 004 applied: tags.name UNIQUE COLLATE NOCASE' as status;
