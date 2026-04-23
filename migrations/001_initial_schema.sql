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
    created_at INTEGER DEFAULT (unixepoch()),
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
    ('Setlist', 'ListMusic', 'S', 0, TRUE),
    ('Phase', 'Layers', 'P', 1, FALSE),
    ('Mood', 'Heart', 'M', 2, FALSE),
    ('Vibe', 'Sparkles', 'V', 3, FALSE),
    ('Merkmal', 'Hash', 'E', 4, FALSE);

-- Verification
SELECT 'Migration 001 applied successfully: 8-table schema created (sync fields removed from service_config)' as status;

SELECT
    (SELECT COUNT(*) FROM tag_categories) as tag_categories_count,
    (SELECT COUNT(*) FROM tags) as tags_count,
    (SELECT COUNT(*) FROM service_tracks) as service_tracks_count,
    (SELECT COUNT(*) FROM service_playlists) as service_playlists_count,
    (SELECT COUNT(*) FROM service_playlist_tracks) as service_playlist_tracks_count,
    (SELECT COUNT(*) FROM files) as files_count,
    (SELECT COUNT(*) FROM service_config) as service_config_count,
    (SELECT COUNT(*) FROM folders) as folders_count;
