-- Tags become an explicit, per-user layer: a tag exists *only* once a user
-- creates it — typically by promoting one of their playlists to a tag (1:1,
-- same name, linked by playlist id). The previous global auto-derived tags are
-- discarded and must be recreated deliberately.
DROP VIEW IF EXISTS hub_v_track_tags;
DROP TABLE IF EXISTS hub_track_resolved_tags;
DROP TABLE IF EXISTS hub_tag_sources;
DROP TABLE IF EXISTS hub_tags;

CREATE TABLE hub_tags (
    id            INTEGER PRIMARY KEY,
    owner_user_id INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    slug          TEXT NOT NULL COLLATE NOCASE,
    name          TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    UNIQUE (owner_user_id, slug)
);

-- A tag is fed by >=1 playlist (its "source"). Normally exactly one, created
-- together with the tag; kept as a table so a tag can later aggregate several.
CREATE TABLE hub_tag_sources (
    tag_id      INTEGER NOT NULL REFERENCES hub_tags(id) ON DELETE CASCADE,
    playlist_id INTEGER NOT NULL REFERENCES hub_playlists(id) ON DELETE CASCADE,
    user_id     INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    service     TEXT NOT NULL,
    created_at  TEXT,
    PRIMARY KEY (tag_id, playlist_id)
);

CREATE TABLE hub_track_resolved_tags (
    track_id INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    tag_id   INTEGER NOT NULL REFERENCES hub_tags(id) ON DELETE CASCADE,
    PRIMARY KEY (track_id, tag_id)
);

CREATE INDEX idx_hub_tags_owner ON hub_tags(owner_user_id);
CREATE INDEX idx_hub_tag_sources_tag ON hub_tag_sources(tag_id);
CREATE INDEX idx_hub_tag_sources_playlist ON hub_tag_sources(playlist_id);
CREATE INDEX idx_hub_trt_tag ON hub_track_resolved_tags(tag_id);

CREATE VIEW hub_v_track_tags AS
SELECT rt.track_id, t.id AS tag_id, t.name AS tag, t.slug, t.owner_user_id
  FROM hub_track_resolved_tags rt
  JOIN hub_tags t ON t.id = rt.tag_id;
