-- Curated playlists: user-built, tag/lens-filled playlists that can be pushed
-- to Spotify (issue #316). A playlist's track set is either hand-picked
-- (`manual`), derived from a tag (`tag`), or from a smart lens (`lens`).

CREATE TABLE hub_curated_playlists (
    id                  INTEGER PRIMARY KEY,
    user_id             INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    name                TEXT NOT NULL,
    description         TEXT NOT NULL DEFAULT '',
    is_public           INTEGER NOT NULL DEFAULT 0,
    -- Set once the playlist has been created on Spotify.
    spotify_playlist_id TEXT,
    -- How the track set is derived: manual | tag | lens
    fill_mode           TEXT NOT NULL DEFAULT 'manual',
    fill_tag_id         INTEGER REFERENCES hub_tags(id) ON DELETE SET NULL,
    -- Lens key (see `curate::LENSES`), e.g. newest | liked.
    fill_lens           TEXT,
    fill_limit          INTEGER NOT NULL DEFAULT 50,
    -- Auto-push once a day when set.
    daily               INTEGER NOT NULL DEFAULT 0,
    last_push_at        TEXT,
    last_push_status    TEXT,
    last_push_error     TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_hub_curated_user ON hub_curated_playlists(user_id);

-- Hand-picked tracks for `manual` playlists (also the materialised shape we
-- push to Spotify for every mode).
CREATE TABLE hub_curated_items (
    playlist_id INTEGER NOT NULL REFERENCES hub_curated_playlists(id) ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL DEFAULT 0,
    added_at    TEXT NOT NULL,
    PRIMARY KEY (playlist_id, track_id)
);
CREATE INDEX IF NOT EXISTS idx_hub_curated_items_track ON hub_curated_items(track_id);
