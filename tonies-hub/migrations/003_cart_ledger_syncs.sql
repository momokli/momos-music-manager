-- Cart (what is currently on each Tonie), per-user album ledger, sync history.
-- Valid values of `mode`/`album_mode`/`status` are validated in Rust, not by
-- CHECK constraints (same convention as the hub migrations).
CREATE TABLE cart_items (
    id       INTEGER PRIMARY KEY,
    user_id  INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    tonie_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    album_id TEXT NOT NULL,
    title    TEXT,
    cover_url TEXT,
    seconds  INTEGER,
    added_at TEXT NOT NULL,
    UNIQUE(user_id, tonie_id, album_id)
);

CREATE INDEX idx_cart_user ON cart_items(user_id, tonie_id, position);

CREATE TABLE albums_seen (
    user_id  INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    album_id TEXT NOT NULL,
    seen_at  TEXT NOT NULL,
    PRIMARY KEY(user_id, album_id)
);

CREATE TABLE syncs (
    id           INTEGER PRIMARY KEY,
    binding_id   INTEGER NOT NULL REFERENCES bindings(id) ON DELETE CASCADE,
    status       TEXT NOT NULL,
    started_at   TEXT NOT NULL,
    finished_at  TEXT,
    added        INTEGER,
    removed      INTEGER,
    skipped_json TEXT,
    error        TEXT
);

CREATE INDEX idx_syncs_binding_started ON syncs(binding_id, started_at DESC);

-- At most one running sync per binding, race-free.
CREATE UNIQUE INDEX idx_syncs_running ON syncs(binding_id) WHERE status = 'running';

SELECT 'Migration 003 applied: cart_items, albums_seen, syncs' AS status;
