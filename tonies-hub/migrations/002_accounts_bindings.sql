-- Tonies account link (one per user) and per-Tonie bindings.
-- secret_enc stays unused in T2; T3 adds Base64(nonce||AES-GCM).
-- Deliberately no Spotify / hub_playlists FKs: this is a standalone DB.
CREATE TABLE tonies_accounts (
    user_id       INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    household_id  TEXT,
    tonie_email   TEXT,
    secret_enc    TEXT NOT NULL,
    last_login_at TEXT,
    last_error    TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE TABLE bindings (
    id               INTEGER PRIMARY KEY,
    user_id          INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    tonie_id         TEXT NOT NULL,
    tonie_name       TEXT,
    source_kind      TEXT NOT NULL DEFAULT 'albums',
    artist_id        TEXT,
    artist_name      TEXT,
    cap_seconds      INTEGER NOT NULL DEFAULT 5400,
    mode             TEXT NOT NULL DEFAULT 'manual',
    album_mode       TEXT NOT NULL DEFAULT 'newest',
    order_by         TEXT NOT NULL DEFAULT 'newest',
    last_sync_at     TEXT,
    last_sync_status TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    UNIQUE(user_id, tonie_id)
);

CREATE INDEX idx_bindings_user ON bindings(user_id);

SELECT 'Migration 002 applied: tonies_accounts, bindings' AS status;
