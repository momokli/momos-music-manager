-- Track-tag history + "last tagged" (issue #296): when a track came to carry a
-- tag and when it lost it, plus the last time a track was manually tagged.
--
-- Playlist-derived membership is recomputed (materialized) and is not historized
-- here; this records deliberate/manual tagging, which is what the revisit flow
-- (#297) needs.

ALTER TABLE hub_tracks ADD COLUMN last_tagged_at TEXT;

CREATE TABLE hub_tag_membership_history (
    track_id   INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    tag_id     INTEGER NOT NULL REFERENCES hub_tags(id) ON DELETE CASCADE,
    added_at   TEXT NOT NULL,
    removed_at TEXT
);
CREATE INDEX idx_hub_tmh_track ON hub_tag_membership_history(track_id);
CREATE INDEX idx_hub_tmh_tag ON hub_tag_membership_history(tag_id);
CREATE INDEX idx_hub_tracks_last_tagged ON hub_tracks(last_tagged_at);

-- Backfill from the existing manual links.
INSERT INTO hub_tag_membership_history (track_id, tag_id, added_at)
    SELECT track_id, tag_id, COALESCE(created_at, '') FROM hub_track_tag_manual;

UPDATE hub_tracks SET last_tagged_at = (
    SELECT MAX(m.created_at) FROM hub_track_tag_manual m WHERE m.track_id = hub_tracks.id
);
