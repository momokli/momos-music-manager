-- Archived (kept) tag links: when a track leaves a source playlist whose
-- `keep_on_remove = 1`, its tag link is preserved here so `rebuild` keeps it.
CREATE TABLE hub_track_tag_archive (
    tag_id             INTEGER NOT NULL REFERENCES hub_tags(id) ON DELETE CASCADE,
    track_id           INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    source_playlist_id INTEGER,
    created_at         TEXT,
    PRIMARY KEY (tag_id, track_id)
);
CREATE INDEX idx_hub_track_tag_archive_track ON hub_track_tag_archive(track_id);

SELECT 'Migration 031 applied: tag archive (keep_on_remove)' as status;
