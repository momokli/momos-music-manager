-- Tag layer: playlists are *sources* that resolve to a normalized tag. A tag can
-- be fed by several playlists (across users/services). Meta-playlists are excluded
-- at resolve time. `hub_track_resolved_tags` is the materialized result.
CREATE TABLE hub_tags (
    id         INTEGER PRIMARY KEY,
    slug       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE hub_tag_sources (
    tag_id      INTEGER NOT NULL REFERENCES hub_tags(id) ON DELETE CASCADE,
    playlist_id INTEGER NOT NULL REFERENCES hub_playlists(id) ON DELETE CASCADE,
    user_id     INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    service     TEXT NOT NULL,
    PRIMARY KEY (tag_id, playlist_id)
);

CREATE TABLE hub_track_resolved_tags (
    track_id INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    tag_id   INTEGER NOT NULL REFERENCES hub_tags(id) ON DELETE CASCADE,
    PRIMARY KEY (track_id, tag_id)
);

CREATE INDEX idx_hub_tag_sources_tag ON hub_tag_sources(tag_id);
CREATE INDEX idx_hub_trt_tag ON hub_track_resolved_tags(tag_id);

-- Track -> tag (name) resolution for display.
CREATE VIEW hub_v_track_tags AS
SELECT rt.track_id, t.id AS tag_id, t.name AS tag, t.slug
  FROM hub_track_resolved_tags rt
  JOIN hub_tags t ON t.id = rt.tag_id;
