-- Backpacks (#327): user-named bundles of "sources". Each item points at exactly
-- ONE source kind: a tag, a playlist, a smart lens, or a single literal track.
-- At most one backpack per user is the "active set" (enforced by a partial
-- UNIQUE index). Schema only — resolution lives in the resolver sibling issues
-- (#329 sources, #330 resolver, #331 active scope).

CREATE TABLE hub_backpacks (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    is_active  INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Guarantee "at most one active backpack per user": the partial unique index only
-- covers is_active = 1 rows. *At least* one active row is an application concern
-- (set-on-create / set-active toggle; see #331).
CREATE UNIQUE INDEX IF NOT EXISTS idx_hub_backpacks_active_user
    ON hub_backpacks(user_id) WHERE is_active = 1;

CREATE INDEX IF NOT EXISTS idx_hub_backpacks_user
    ON hub_backpacks(user_id);

-- Names are unique per user (case-insensitive).
CREATE UNIQUE INDEX IF NOT EXISTS idx_hub_backpacks_user_name
    ON hub_backpacks(user_id, name COLLATE NOCASE);

CREATE TABLE hub_backpack_items (
    id                 INTEGER PRIMARY KEY,
    backpack_id        INTEGER NOT NULL REFERENCES hub_backpacks(id) ON DELETE CASCADE,
    -- Which of the 4 source kinds this item is.
    source_kind        TEXT NOT NULL CHECK (source_kind IN ('tag', 'playlist', 'lens', 'track')),
    -- Exactly one of the following is set, matching source_kind (see CHECK below).
    source_tag_id      INTEGER REFERENCES hub_tags(id)      ON DELETE CASCADE,
    source_playlist_id INTEGER REFERENCES hub_playlists(id) ON DELETE CASCADE,
    -- Lens key; see `curate::LENSES` (e.g. newest | liked | unheard). Validated
    -- in the app, not at the DB level (the lens list changes with the code).
    source_lens        TEXT,
    source_track_id    INTEGER REFERENCES hub_tracks(id)    ON DELETE CASCADE,
    position           INTEGER NOT NULL DEFAULT 0,
    added_at           TEXT NOT NULL,
    -- Enforce the "exactly one source column per kind" invariant at DB level.
    CHECK (
        (source_kind = 'tag'      AND source_tag_id      IS NOT NULL
                                  AND source_playlist_id IS NULL
                                  AND source_lens        IS NULL
                                  AND source_track_id    IS NULL) OR
        (source_kind = 'playlist' AND source_playlist_id IS NOT NULL
                                  AND source_tag_id      IS NULL
                                  AND source_lens        IS NULL
                                  AND source_track_id    IS NULL) OR
        (source_kind = 'lens'     AND source_lens        IS NOT NULL
                                  AND source_tag_id      IS NULL
                                  AND source_playlist_id IS NULL
                                  AND source_track_id    IS NULL) OR
        (source_kind = 'track'    AND source_track_id    IS NOT NULL
                                  AND source_tag_id      IS NULL
                                  AND source_playlist_id IS NULL
                                  AND source_lens        IS NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_hub_backpack_items_backpack
    ON hub_backpack_items(backpack_id);
CREATE INDEX IF NOT EXISTS idx_hub_backpack_items_tag
    ON hub_backpack_items(source_tag_id)      WHERE source_tag_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_hub_backpack_items_playlist
    ON hub_backpack_items(source_playlist_id) WHERE source_playlist_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_hub_backpack_items_track
    ON hub_backpack_items(source_track_id)    WHERE source_track_id IS NOT NULL;
