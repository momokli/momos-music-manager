-- Speed up relationship-based tag recommendations (artist neighbourhood join).
CREATE INDEX idx_hub_tracks_artists ON hub_tracks(artists);

SELECT 'Migration 034 applied: hub_tracks.artists index' as status;
