-- Speed up album grouping/detail (`GROUP BY album`, `WHERE album = ?`).
CREATE INDEX idx_hub_tracks_album ON hub_tracks(album);

SELECT 'Migration 032 applied: hub_tracks.album index' as status;
