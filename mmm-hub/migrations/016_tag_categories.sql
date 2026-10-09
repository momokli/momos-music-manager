-- Per-user tag categories (like Momo's Music Manager). A tag belongs to at most
-- one category; categories carry a pickable icon and can be adopted (copied)
-- from other users.
CREATE TABLE hub_tag_categories (
    id            INTEGER PRIMARY KEY,
    owner_user_id INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    slug          TEXT NOT NULL COLLATE NOCASE,
    icon          TEXT NOT NULL DEFAULT '',
    sort_order    INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL,
    UNIQUE (owner_user_id, slug)
);

ALTER TABLE hub_tags ADD COLUMN category_id INTEGER REFERENCES hub_tag_categories(id) ON DELETE SET NULL;
CREATE INDEX idx_hub_tags_category ON hub_tags(category_id);
CREATE INDEX idx_hub_tag_categories_owner ON hub_tag_categories(owner_user_id);
