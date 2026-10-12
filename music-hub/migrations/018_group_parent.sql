-- Groups can be nested: a group's `parent_group_id` is the "contributor group"
-- that governs it. Membership in the parent inherits down to all child groups
-- (computed at read time in the app).
ALTER TABLE hub_tag_groups ADD COLUMN parent_group_id INTEGER
    REFERENCES hub_tag_groups(id) ON DELETE SET NULL;
CREATE INDEX idx_hub_tag_groups_parent ON hub_tag_groups(parent_group_id);
