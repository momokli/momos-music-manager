-- TTL marker for the music-api metadata backfill (ISRC → BPM/key authority).
-- Separate from `fetched_at` so a music-api "queried, no data" pass does not
-- shorten the ReccoBeats re-check cycle. Nullable: absent = never checked.
ALTER TABLE hub_track_features ADD COLUMN metadata_checked_at TEXT;
