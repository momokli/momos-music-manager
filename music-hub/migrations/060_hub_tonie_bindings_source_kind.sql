-- no-transaction
-- Tonies source bindings (#504): allow a backpack or a tag as the binding
-- source.
--
-- Acceptance #2 requires that deleting the bound source leaves the binding in
-- place (the sync then reports a clear error) instead of silently dropping it.
-- The original 057 schema tied `source_id` to `hub_playlists` with
-- `ON DELETE CASCADE`, which deleted the whole binding when the playlist went
-- away. `source_kind` was already left CHECK-free for exactly this issue, but
-- the FK cannot be dropped in place — SQLite has no
-- `ALTER TABLE ... DROP CONSTRAINT` — so the table is rebuilt.
--
-- The rebuild runs with `foreign_keys=OFF` so that
-- `hub_tonie_syncs.binding_id` (ON DELETE CASCADE) does not fire while the old
-- table is dropped; row ids are copied verbatim so the run log keeps pointing
-- at the same bindings. Columns, defaults, `UNIQUE(user_id, tonie_id)` and
-- `idx_hub_tonie_bindings_user` are preserved; only the `source_id` FK is gone.
-- The validity of a source is enforced in `tonies::bindings` / the web layer,
-- not by the schema.

PRAGMA foreign_keys = OFF;

CREATE TABLE hub_tonie_bindings_new (
    id               INTEGER PRIMARY KEY,
    user_id          INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    tonie_id         TEXT NOT NULL,
    tonie_name       TEXT,
    source_kind      TEXT NOT NULL DEFAULT 'playlist',  -- 'playlist' | 'backpack' | 'tag'
    source_id        INTEGER,                           -- no FK: see header
    format           TEXT NOT NULL DEFAULT 'mp3-128',
    cap_seconds      INTEGER NOT NULL DEFAULT 5400,
    mode             TEXT NOT NULL DEFAULT 'manual',
    dirty_since      TEXT,
    last_sync_at     TEXT,
    last_sync_status TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    UNIQUE(user_id, tonie_id)
);

INSERT INTO hub_tonie_bindings_new
    (id, user_id, tonie_id, tonie_name, source_kind, source_id, format, cap_seconds,
     mode, dirty_since, last_sync_at, last_sync_status, created_at, updated_at)
SELECT id, user_id, tonie_id, tonie_name, source_kind, source_id, format, cap_seconds,
       mode, dirty_since, last_sync_at, last_sync_status, created_at, updated_at
  FROM hub_tonie_bindings;

DROP TABLE hub_tonie_bindings;

ALTER TABLE hub_tonie_bindings_new RENAME TO hub_tonie_bindings;

CREATE INDEX idx_hub_tonie_bindings_user ON hub_tonie_bindings(user_id);

PRAGMA foreign_key_check;
PRAGMA foreign_keys = ON;

SELECT 'Migration 060 applied: tonie binding source_kind (no source FK)' AS status;
