-- Backpack content materialisation (#682, slice B1): the resolved track set of
-- every backpack item, persisted so `/backpacks` can show per-source counts and
-- availability ("x/y verfügbar") without resolving live or calling music-api.
--
-- One row per (item, track). `backend` is the source kind literal that produced
-- the track: `tag` | `playlist` | `track` | `lens:<key>`.
--
-- Both FKs cascade: deleting an item (or a track) removes its materialised rows,
-- so remove/clear/delete need no manual cleanup.
CREATE TABLE hub_backpack_item_tracks (
    item_id     INTEGER NOT NULL REFERENCES hub_backpack_items(id) ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES hub_tracks(id)          ON DELETE CASCADE,
    backend     TEXT NOT NULL,
    computed_at TEXT NOT NULL,
    PRIMARY KEY (item_id, track_id)
);

CREATE INDEX IF NOT EXISTS idx_hub_backpack_item_tracks_track
    ON hub_backpack_item_tracks(track_id);