-- 003_tag_parents.sql
-- Tag parent resolution: a Setlist tag can resolve to one or more parent tags
-- in P/M/V/E categories. Comment generation uses parent tags instead of the original.

-- tag_parents: links a source tag to its parent tags
CREATE TABLE tag_parents (
    id INTEGER PRIMARY KEY,
    tag_id INTEGER NOT NULL,
    parent_tag_id INTEGER NOT NULL,
    created_at INTEGER DEFAULT (unixepoch()),
    FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE,
    FOREIGN KEY (parent_tag_id) REFERENCES tags(id) ON DELETE CASCADE,
    UNIQUE (tag_id, parent_tag_id)
);
CREATE INDEX idx_tag_parents_tag_id ON tag_parents(tag_id);
CREATE INDEX idx_tag_parents_parent_tag_id ON tag_parents(parent_tag_id);

-- v_resolved_tags: for every tag, returns the effective tags after parent resolution.
-- Tags with parents → their parent tags (with parent's category/prefix).
-- Tags without parents → the tag itself (with its own category/prefix).
-- Tags that are themselves parents of other tags still appear as themselves.
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

-- v_file_resolved_tags: like v_file_tags, but resolves through tag parents.
-- Used by compute_target_comment so comments use parent tag names + categories.
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
