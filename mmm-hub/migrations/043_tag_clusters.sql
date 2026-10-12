-- Semantic tag clusters (#261–#268): group semantically-related tags of the
-- same type (e.g. Mood "dark" = dark, düster, …). Built automatically from name
-- similarity (+ co-occurrence), but controllable by the collective.
--
-- Clusters feed the recommendation engine and give a coarser, shared handle on
-- top of the per-user 1:1 tags.

CREATE TABLE hub_tag_clusters (
    id         INTEGER PRIMARY KEY,
    tag_type   TEXT NOT NULL DEFAULT '',
    label      TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_hub_cluster_type_label
    ON hub_tag_clusters(tag_type, label COLLATE NOCASE);

CREATE TABLE hub_tag_cluster_members (
    cluster_id INTEGER NOT NULL REFERENCES hub_tag_clusters(id) ON DELETE CASCADE,
    tag_id     INTEGER NOT NULL REFERENCES hub_tags(id) ON DELETE CASCADE,
    added_at   TEXT NOT NULL,
    PRIMARY KEY (cluster_id, tag_id)
);
CREATE INDEX IF NOT EXISTS idx_hub_cluster_members_tag
    ON hub_tag_cluster_members(tag_id);
