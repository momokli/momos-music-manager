-- Direct track↔tag relations, so a track can be tagged without belonging to a
-- playlist. `tags::rebuild` folds these into `hub_track_resolved_tags`.
CREATE TABLE hub_track_tag_manual (
    track_id   INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    tag_id     INTEGER NOT NULL REFERENCES hub_tags(id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    created_at TEXT,
    PRIMARY KEY (track_id, tag_id, user_id)
);
CREATE INDEX idx_hub_track_tag_manual_tag ON hub_track_tag_manual(tag_id);
CREATE INDEX idx_hub_track_tag_manual_track ON hub_track_tag_manual(track_id);

-- Per (tag, playlist) source policy when a track leaves the source playlist:
--   0 = drop the tag from the track (default), 1 = keep it (archive).
ALTER TABLE hub_tag_sources ADD COLUMN keep_on_remove INTEGER NOT NULL DEFAULT 0;

-- Opt-in bi-way sync: when a track is tagged, push it to a linked playlist.
ALTER TABLE hub_tags ADD COLUMN sync_playlist INTEGER NOT NULL DEFAULT 0;

SELECT 'Migration 029 applied: direct track tagging + source policy + sync flag' as status;
