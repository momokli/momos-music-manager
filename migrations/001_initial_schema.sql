-- Migration 001: Initial Schema for Momo's Music Manager
-- Fresh green field start with 8-table schema
-- Updated: 2026-04-19 - Remove sync fields from service_config (in-memory task tracking)
-- Date: $(date)

-- 1. TAG CATEGORIES
CREATE TABLE tag_categories (
    id INTEGER PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    icon TEXT NOT NULL DEFAULT '',
    prefix CHAR(1) UNIQUE NOT NULL,
    sort_order INTEGER DEFAULT 0,
    is_default BOOLEAN DEFAULT FALSE,
    created_at INTEGER DEFAULT (unixepoch())
);

-- 2. TAGS (Name + Category, UNIQUE name)
CREATE TABLE tags (
    id INTEGER PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    category_id INTEGER NOT NULL,
    sort_order INTEGER DEFAULT 0,
    created_at INTEGER DEFAULT (unixepoch()),
    reviewed_at INTEGER,
    FOREIGN KEY (category_id) REFERENCES tag_categories(id)
);

-- 3. SERVICE TRACKS (keine BPM/Key Spalten!)
CREATE TABLE service_tracks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    service TEXT NOT NULL CHECK (service IN ('spotify', 'soundcloud', 'youtube')),
    service_id TEXT NOT NULL,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album TEXT,
    isrc TEXT,
    duration_ms INTEGER,
    metadata_json TEXT,
    imported_at INTEGER DEFAULT (unixepoch()),
    updated_at INTEGER DEFAULT (unixepoch()),
    UNIQUE(service, service_id)
);

-- 4. SERVICE PLAYLISTS (kein FK zu tags!)
CREATE TABLE service_playlists (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    service TEXT NOT NULL,
    playlist_id TEXT NOT NULL,
    name TEXT NOT NULL,  -- wird mit tags.name gematcht
    description TEXT,
    metadata_json TEXT,
    imported_at INTEGER DEFAULT (unixepoch()),
    updated_at INTEGER DEFAULT (unixepoch()),
    UNIQUE(service, playlist_id)
);

-- 5. PLAYLIST TRACKS (many-to-many)
CREATE TABLE service_playlist_tracks (
    playlist_id INTEGER NOT NULL,
    track_id INTEGER NOT NULL,
    position INTEGER,
    added_at INTEGER DEFAULT (unixepoch()),
    PRIMARY KEY (playlist_id, track_id),
    FOREIGN KEY (track_id) REFERENCES service_tracks(id) ON DELETE CASCADE,
    FOREIGN KEY (playlist_id) REFERENCES service_playlists(id) ON DELETE CASCADE
);

-- 6. FILES (mit direkten Service-IDs, BPM/Key NUR hier)
CREATE TABLE files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path TEXT NOT NULL UNIQUE,
    file_hash TEXT NOT NULL,
    file_type TEXT NOT NULL CHECK (file_type IN ('flac', 'mp3', 'stem.m4a', 'wav', 'opus')),
    file_size INTEGER NOT NULL,
    last_modified INTEGER NOT NULL,
    isrc TEXT,
    last_scanned INTEGER DEFAULT (unixepoch()),

    -- Audio-Metadaten
    title TEXT,
    artist TEXT,
    album TEXT,
    album_artist TEXT,
    track_number INTEGER,
    total_tracks INTEGER,
    disc_number INTEGER,
    total_discs INTEGER,
    genre TEXT,
    year INTEGER,
    composer TEXT,
    comment TEXT,
    duration_ms INTEGER,
    bitrate INTEGER,
    sample_rate INTEGER,
    channels INTEGER,

    -- BPM/Key NUR aus Traktor/EXIF
    bpm REAL,
    musical_key TEXT,

    -- Traktor Stats
    rating INTEGER DEFAULT 0,
    play_count INTEGER DEFAULT 0,
    last_played INTEGER,

    -- DIREKTE SERVICE-IDs (keine matches Tabelle!)
    spotify_id TEXT,
    soundcloud_id TEXT,
    youtube_id TEXT,

    -- Timestamps
    created_at INTEGER DEFAULT (unixepoch()),
    updated_at INTEGER DEFAULT (unixepoch())
);

-- 7. SERVICE CONFIG (OAuth + basic service info - NO SYNC FIELDS)
-- Sync state tracked in-memory, not in database
CREATE TABLE service_config (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    service TEXT NOT NULL CHECK (service IN ('spotify', 'soundcloud', 'youtube')),
    refresh_token TEXT,
    access_token TEXT,
    token_expiry INTEGER,
    user_id TEXT,
    playlist_id TEXT,
    is_connected BOOLEAN NOT NULL DEFAULT 0,
    last_checked INTEGER,
    last_synced INTEGER,
    remote_playlists_count INTEGER NOT NULL DEFAULT 0,
    remote_tracks_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(service)
);

-- 8. FOLDERS (Überwachung)
CREATE TABLE folders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_path TEXT NOT NULL UNIQUE,
    active BOOLEAN NOT NULL DEFAULT 1,
    scan_recursive BOOLEAN NOT NULL DEFAULT 0,
    fixed_extensions BOOLEAN NOT NULL DEFAULT 0,
    file_extensions TEXT NOT NULL DEFAULT '',
    max_depth INTEGER NOT NULL DEFAULT 1,
    last_scanned INTEGER,
    created_at INTEGER DEFAULT (unixepoch()),
    updated_at INTEGER DEFAULT (unixepoch())
);

-- 9. TAG EMBEDDINGS (Cache für semantische Vektoren, 384 f32 = 1536 bytes)
CREATE TABLE tag_embeddings (
    tag_id INTEGER PRIMARY KEY,
    embedding BLOB NOT NULL,
    model_version TEXT NOT NULL DEFAULT 'all-MiniLM-L6-v2',
    updated_at INTEGER DEFAULT (unixepoch()),
    FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
);

-- 10. TAG ENERGY LEVELS (Phase-Tag → Energielevel Mapping für Digging/Suggestion Engine)
CREATE TABLE tag_energy_levels (
    tag_id INTEGER PRIMARY KEY,
    energy_level INTEGER NOT NULL CHECK (energy_level BETWEEN 0 AND 5),
    created_at INTEGER DEFAULT (unixepoch()),
    FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
);

-- 11. PLAYLIST SUBSCRIPTIONS (Auto-Polling für einzelne Playlists)
CREATE TABLE playlist_subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    service TEXT NOT NULL,
    playlist_id TEXT NOT NULL,
    service_playlist_id INTEGER,
    subscribed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_polled_at INTEGER,
    poll_interval_secs INTEGER NOT NULL DEFAULT 300,
    is_active BOOLEAN NOT NULL DEFAULT 1,
    UNIQUE(service, playlist_id),
    FOREIGN KEY (service_playlist_id) REFERENCES service_playlists(id) ON DELETE SET NULL
);

CREATE INDEX idx_tag_energy_levels_tag_id ON tag_energy_levels(tag_id);

CREATE INDEX idx_playlist_subscriptions_service_playlist_id
    ON playlist_subscriptions(service_playlist_id);
CREATE INDEX idx_playlist_subscriptions_is_active
    ON playlist_subscriptions(is_active);

-- 12. TAG SIMILARITIES (Pairwise cosine similarity between tag embeddings)
CREATE TABLE tag_similarities (
    tag_a_id INTEGER NOT NULL,
    tag_b_id INTEGER NOT NULL,
    similarity REAL NOT NULL,
    updated_at INTEGER DEFAULT (unixepoch()),
    PRIMARY KEY (tag_a_id, tag_b_id),
    FOREIGN KEY (tag_a_id) REFERENCES tags(id) ON DELETE CASCADE,
    FOREIGN KEY (tag_b_id) REFERENCES tags(id) ON DELETE CASCADE
);

CREATE INDEX idx_tag_similarities_tag_a_id ON tag_similarities(tag_a_id);
CREATE INDEX idx_tag_similarities_tag_b_id ON tag_similarities(tag_b_id);

-- Indexes for Performance
CREATE INDEX idx_tags_name ON tags(name);
CREATE INDEX idx_tags_category_id ON tags(category_id);

CREATE INDEX idx_service_tracks_service_id ON service_tracks(service, service_id);
CREATE INDEX idx_service_tracks_isrc ON service_tracks(isrc);

CREATE INDEX idx_service_playlists_name ON service_playlists(name);
CREATE INDEX idx_service_playlists_service_playlist_id ON service_playlists(service, playlist_id);

CREATE INDEX idx_service_playlist_tracks_track_id ON service_playlist_tracks(track_id);
CREATE INDEX idx_service_playlist_tracks_playlist_id ON service_playlist_tracks(playlist_id);

CREATE INDEX idx_files_file_path ON files(file_path);
CREATE INDEX idx_files_isrc ON files(isrc);
CREATE INDEX idx_files_bpm ON files(bpm);
CREATE INDEX idx_files_musical_key ON files(musical_key);
CREATE INDEX idx_files_rating ON files(rating);
CREATE INDEX idx_files_last_scanned ON files(last_scanned);

CREATE INDEX idx_folders_folder_path ON folders(folder_path);
CREATE INDEX idx_folders_active ON folders(active);

CREATE INDEX idx_tag_embeddings_tag_id ON tag_embeddings(tag_id);

-- Unified tracks view for Explorer feature only (internal use)
CREATE VIEW unified_tracks AS
SELECT
    'file' as source_type,
    f.id,
    f.file_path as identifier,
    COALESCE(f.title, '') as title,
    COALESCE(f.artist, '') as artist,
    f.bpm,
    f.musical_key as key,
    f.isrc,
    f.duration_ms,
    f.rating,
    '[]' as tags_json
FROM files f
UNION ALL
SELECT
    'service' as source_type,
    -st.id as id,  -- Negative IDs to avoid conflicts with file IDs
    st.service_id as identifier,
    st.title,
    st.artist,
    NULL as bpm,
    NULL as key,
    st.isrc,
    st.duration_ms,
    NULL as rating,
    '[]' as tags_json
FROM service_tracks st;

-- Initial data
INSERT INTO tag_categories (name, icon, prefix, sort_order, is_default) VALUES
    ('Setlist', 'fa-solid fa-list-music', 'S', 0, TRUE),
    ('Phase', 'fa-solid fa-layers', 'P', 1, FALSE),
    ('Mood', 'fa-solid fa-heart', 'M', 2, FALSE),
    ('Vibe', 'fa-solid fa-sparkles', 'V', 3, FALSE),
    ('Merkmal', 'fa-solid fa-hashtag', 'E', 4, FALSE);

INSERT INTO tags (name, category_id, created_at, reviewed_at) VALUES
    ('start', 2, unixepoch(), unixepoch()),
    ('build', 2, unixepoch(), unixepoch()),
    ('peak', 2, unixepoch(), unixepoch()),
    ('release', 2, unixepoch(), unixepoch()),
    ('sustain', 2, unixepoch(), unixepoch()),
    ('end', 2, unixepoch(), unixepoch());

INSERT INTO tag_energy_levels (tag_id, energy_level, created_at) VALUES
    ((SELECT id FROM tags WHERE name = 'start'), 1, unixepoch()),
    ((SELECT id FROM tags WHERE name = 'build'), 2, unixepoch()),
    ((SELECT id FROM tags WHERE name = 'peak'), 5, unixepoch()),
    ((SELECT id FROM tags WHERE name = 'release'), 3, unixepoch()),
    ((SELECT id FROM tags WHERE name = 'sustain'), 2, unixepoch()),
    ((SELECT id FROM tags WHERE name = 'end'), 1, unixepoch());


-- Verification
SELECT 'Migration 001 applied successfully: 12-table schema (added tag_similarities)' as status;

SELECT
    (SELECT COUNT(*) FROM tag_categories) as tag_categories_count,
    (SELECT COUNT(*) FROM tags) as tags_count,
    (SELECT COUNT(*) FROM service_tracks) as service_tracks_count,
    (SELECT COUNT(*) FROM service_playlists) as service_playlists_count,
    (SELECT COUNT(*) FROM service_playlist_tracks) as service_playlist_tracks_count,
    (SELECT COUNT(*) FROM files) as files_count,
    (SELECT COUNT(*) FROM service_config) as service_config_count,
    (SELECT COUNT(*) FROM folders) as folders_count;
