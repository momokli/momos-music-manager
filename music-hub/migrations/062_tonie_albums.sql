-- Tonie-Album-Queue (plans/proposed/tonie-album-queue.md, M1+M2+M3).
--
-- Neue Quellenart `albums`: der Tonie wird aus Deezer-Alben gefuellt statt aus
-- Spotify-Playlists. Grund: ISRC ist editionsgebunden, Spotify und Deezer
-- schneiden dieselbe Hoerspiel-Folge unterschiedlich.
--
-- Additiv: bestehende Bindings bleiben `playlist` + `album_mode='newest'`.

ALTER TABLE hub_tonie_bindings ADD COLUMN album_mode TEXT NOT NULL DEFAULT 'newest';

-- Warenkorb: explizite Album-Reihenfolge pro Binding (album_mode='basket').
CREATE TABLE hub_tonie_album_items (
    id          INTEGER PRIMARY KEY,
    binding_id  INTEGER NOT NULL REFERENCES hub_tonie_bindings(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    album_id    TEXT NOT NULL,              -- Deezer-Album-ID
    album_title TEXT,
    seconds     INTEGER,                    -- Dauer zum Zeitpunkt des Hinzufuegens
    added_at    TEXT NOT NULL,
    UNIQUE (binding_id, album_id)
);

CREATE INDEX idx_hub_tonie_album_items_binding
    ON hub_tonie_album_items(binding_id, position);

-- Ledger: ein Album gilt als gehoert, sobald es einmal Teil eines ok-Runs war.
-- Pro User, nicht pro Tonie (das Kind hoert die Folge unabhaengig vom Tonie).
CREATE TABLE hub_tonie_albums_seen (
    user_id       INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    album_id      TEXT NOT NULL,
    album_title   TEXT,
    first_used_at TEXT NOT NULL,
    last_used_at  TEXT NOT NULL,
    PRIMARY KEY (user_id, album_id)
);

-- Cache der Deezer-Album-Metadaten: die Artist-Album-Liste liefert weder
-- nb_tracks noch Dauer, also braucht das Album-Grid einen Tracklist-Call pro
-- Album. Tracks werden als JSON mitgespeichert (TTL entscheidet der Client).
CREATE TABLE hub_deezer_albums (
    album_id     TEXT PRIMARY KEY,
    artist_id    TEXT,
    title        TEXT,
    release_date TEXT,
    record_type  TEXT,
    cover_url    TEXT,
    seconds      INTEGER,
    tracks_json  TEXT NOT NULL,
    fetched_at   TEXT NOT NULL
);

SELECT 'Migration 062 applied: tonie album queue' AS status;
