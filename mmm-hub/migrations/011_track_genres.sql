-- Track genres (community tags). Last.fm is the practical source for electronic
-- music; MusicBrainz coverage is sparse. `genre = ''` marks "checked, none".
CREATE TABLE hub_track_genres (
    track_id INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    genre    TEXT NOT NULL,
    source   TEXT NOT NULL DEFAULT 'lastfm',
    PRIMARY KEY (track_id, source, genre)
);
CREATE INDEX idx_hub_track_genres_track ON hub_track_genres(track_id);
