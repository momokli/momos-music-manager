-- Turn per-user "categories" into "groups" with membership roles
-- (owner/contributor/subscriber) and a many-to-many tag↔group relation, so a
-- tag can live in several groups and groups can be shared.
ALTER TABLE hub_tag_categories RENAME TO hub_tag_groups;
DROP INDEX IF EXISTS idx_hub_tag_categories_owner;
DROP INDEX IF EXISTS idx_hub_tags_category;
CREATE INDEX idx_hub_tag_groups_owner ON hub_tag_groups(owner_user_id);

CREATE TABLE hub_group_members (
    group_id   INTEGER NOT NULL REFERENCES hub_tag_groups(id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    role       TEXT NOT NULL CHECK (role IN ('owner', 'contributor', 'subscriber')),
    created_at TEXT,
    PRIMARY KEY (group_id, user_id)
);
-- Every existing group's owner becomes its 'owner' member.
INSERT INTO hub_group_members (group_id, user_id, role, created_at)
    SELECT id, owner_user_id, 'owner', created_at FROM hub_tag_groups;

CREATE TABLE hub_group_tags (
    tag_id   INTEGER NOT NULL REFERENCES hub_tags(id) ON DELETE CASCADE,
    group_id INTEGER NOT NULL REFERENCES hub_tag_groups(id) ON DELETE CASCADE,
    PRIMARY KEY (tag_id, group_id)
);
-- Migrate the old single-category link into the n:m relation.
INSERT INTO hub_group_tags (tag_id, group_id)
    SELECT id, category_id FROM hub_tags WHERE category_id IS NOT NULL;

CREATE INDEX idx_hub_group_tags_group ON hub_group_tags(group_id);
CREATE INDEX idx_hub_group_members_user ON hub_group_members(user_id);
