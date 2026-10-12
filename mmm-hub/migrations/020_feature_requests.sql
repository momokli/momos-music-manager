-- On-demand BPM/key (audio-feature) requests. A user can prioritise a set of
-- tracks (e.g. the shared tracks on /overlap) so the enrichment worker fetches
-- their ReccoBeats features before its generic backlog scan.
-- Rows are deleted once the worker has attempted a lookup (found or not).
CREATE TABLE hub_feature_requests (
    track_id     INTEGER PRIMARY KEY REFERENCES hub_tracks(id) ON DELETE CASCADE,
    requested_at TEXT NOT NULL
);
