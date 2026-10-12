-- Tag hierarchy from MMM ("parent tags" / aliases). A tag can have parent tags
-- (e.g. "Beatport Top 100 - Progressive House" -> house, progressive). Used to
-- make name filters hierarchy-aware (a parent match also matches its children).
CREATE TABLE hub_tag_parents (
    tag_id        INTEGER NOT NULL REFERENCES hub_tags(id) ON DELETE CASCADE,
    parent_tag_id INTEGER NOT NULL REFERENCES hub_tags(id) ON DELETE CASCADE,
    created_at    TEXT,
    PRIMARY KEY (tag_id, parent_tag_id)
);
CREATE INDEX idx_hub_tag_parents_parent ON hub_tag_parents(parent_tag_id);
