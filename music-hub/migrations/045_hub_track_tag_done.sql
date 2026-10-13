-- Track "tagged-complete" flag (#345, T5-1 / F1). A track counts as fully
-- tagged either automatically — every configured describing tag-type family
-- (MOOD/GENRE/VIBE/PHASE/MERKMAL) has at least one resolved tag — or because the
-- user manually marked it done. The manual mark is per user and overrides the
-- auto rule.

CREATE TABLE hub_track_tag_done (
    user_id   INTEGER NOT NULL REFERENCES hub_users(id)  ON DELETE CASCADE,
    track_id  INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    marked_at TEXT NOT NULL,
    PRIMARY KEY (user_id, track_id)
);

CREATE INDEX IF NOT EXISTS idx_hub_ttd_track
    ON hub_track_tag_done(track_id);
