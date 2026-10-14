-- Artist + album entities (#artist-entities, plan P0). Artists and albums become
-- first-class rows (3NF mirror of the Spotify payload) instead of being flattened
-- into `hub_tracks.artists` / `hub_tracks.album` at ingest. Provider ids live in
-- their own side tables (mirrors `hub_track_external_ids`).
--
-- Additive only. `hub_tracks.artists` / `hub_tracks.album` stay as denormalized
-- display caches (kept in sync at ingest) while consumers migrate to the entity
-- tables. The one-time backfill of historical rows lives in Rust
-- (`artists::backfill_artist_entities`), not in SQL — splitting the free-text
-- artist strings in pure SQLite is error-prone.

CREATE TABLE hub_artists (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,           -- display name (as last seen)
    sort_name     TEXT,
    image_url     TEXT,
    followers     INTEGER,
    popularity    INTEGER,
    genres        TEXT,                    -- comma-joined Spotify artist genres
    first_seen_at TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_hub_artists_name  ON hub_artists(name COLLATE NOCASE);
CREATE INDEX        IF NOT EXISTS idx_hub_artists_sort  ON hub_artists(sort_name);

-- Provider ids (Spotify artist id, MusicBrainz id, …) — the real identity.
CREATE TABLE hub_artist_external_ids (
    artist_id   INTEGER NOT NULL REFERENCES hub_artists(id) ON DELETE CASCADE,
    service     TEXT NOT NULL,
    external_id TEXT NOT NULL,
    url         TEXT,
    source      TEXT,
    fetched_at  TEXT,
    PRIMARY KEY (artist_id, service, external_id)
);
CREATE INDEX IF NOT EXISTS idx_hub_artist_ext_lookup ON hub_artist_external_ids(service, external_id);

-- Track ↔ artist (ordered: primary, feat., …).
CREATE TABLE hub_track_artists (
    track_id    INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    artist_id   INTEGER NOT NULL REFERENCES hub_artists(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (track_id, artist_id)
);
CREATE INDEX IF NOT EXISTS idx_hub_track_artists_artist ON hub_track_artists(artist_id);

-- Albums as entities (track -> album is N:1), so we stop flattening `album` too.
CREATE TABLE hub_albums (
    id            INTEGER PRIMARY KEY,
    title         TEXT NOT NULL,
    album_type    TEXT,            -- album | single | compilation | appears_on
    release_date  TEXT,
    total_tracks  INTEGER,
    label         TEXT,
    image_url     TEXT,
    first_seen_at TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_hub_albums_title ON hub_albums(title COLLATE NOCASE);

CREATE TABLE hub_album_external_ids (
    album_id    INTEGER NOT NULL REFERENCES hub_albums(id) ON DELETE CASCADE,
    service     TEXT NOT NULL,
    external_id TEXT NOT NULL,
    url         TEXT,
    source      TEXT,
    fetched_at  TEXT,
    PRIMARY KEY (album_id, service, external_id)
);
CREATE INDEX IF NOT EXISTS idx_hub_album_ext_lookup ON hub_album_external_ids(service, external_id);

ALTER TABLE hub_tracks ADD COLUMN album_id INTEGER REFERENCES hub_albums(id);
CREATE INDEX IF NOT EXISTS idx_hub_tracks_album_id ON hub_tracks(album_id);
