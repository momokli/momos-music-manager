-- Collectives: a group of users that *has* groups. Every collective member is a
-- contributor in all of the collective's groups. Replaces the earlier
-- parent-group inheritance (migrated below).
CREATE TABLE hub_collectives (
    id            INTEGER PRIMARY KEY,
    slug          TEXT NOT NULL UNIQUE COLLATE NOCASE,
    name          TEXT NOT NULL,
    icon          TEXT NOT NULL DEFAULT '',
    owner_user_id INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    created_at    TEXT NOT NULL
);

CREATE TABLE hub_collective_members (
    collective_id INTEGER NOT NULL REFERENCES hub_collectives(id) ON DELETE CASCADE,
    user_id       INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    role          TEXT NOT NULL CHECK (role IN ('owner', 'member')),
    created_at    TEXT,
    PRIMARY KEY (collective_id, user_id)
);

ALTER TABLE hub_tag_groups ADD COLUMN collective_id INTEGER
    REFERENCES hub_collectives(id) ON DELETE SET NULL;

-- Migrate: every existing "parent group" becomes a collective; its children are
-- re-pointed at that collective.
INSERT INTO hub_collectives (slug, name, icon, owner_user_id, created_at)
    SELECT 'c-' || g.slug, g.name, COALESCE(g.icon, ''), g.owner_user_id, g.created_at
      FROM hub_tag_groups g
     WHERE EXISTS (SELECT 1 FROM hub_tag_groups c WHERE c.parent_group_id = g.id);

INSERT INTO hub_collective_members (collective_id, user_id, role, created_at)
    SELECT col.id, m.user_id,
           CASE WHEN m.role = 'owner' THEN 'owner' ELSE 'member' END, m.created_at
      FROM hub_tag_groups pg
      JOIN hub_collectives col
        ON col.slug = 'c-' || pg.slug AND col.owner_user_id = pg.owner_user_id
      JOIN hub_group_members m ON m.group_id = pg.id;

UPDATE hub_tag_groups SET collective_id = (
    SELECT col.id FROM hub_collectives col
     WHERE col.slug = 'c-' || (SELECT pg.slug FROM hub_tag_groups pg WHERE pg.id = hub_tag_groups.parent_group_id)
       AND col.owner_user_id = (SELECT pg.owner_user_id FROM hub_tag_groups pg WHERE pg.id = hub_tag_groups.parent_group_id)
) WHERE parent_group_id IS NOT NULL;

-- The structural parent groups (now collectives) are removed when empty.
DELETE FROM hub_tag_groups
 WHERE id IN (
    SELECT pg.id FROM hub_tag_groups pg
     JOIN hub_collectives col ON col.slug = 'c-' || pg.slug
                              AND col.owner_user_id = pg.owner_user_id
 )
   AND NOT EXISTS (SELECT 1 FROM hub_group_tags gt WHERE gt.group_id = hub_tag_groups.id);

CREATE INDEX idx_hub_tag_groups_collective ON hub_tag_groups(collective_id);
CREATE INDEX idx_hub_collective_members_user ON hub_collective_members(user_id);
