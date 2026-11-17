-- Tonies source bindings (#500, M2 "Quelle wählen"): one row per
-- (user, Kreativ-Tonie) that records which playlist feeds the Tonie, at which
-- format, and in which mode. Additive and small — the sync machinery
-- (`hub_tonie_syncs`, fit/diff, run) comes with #501/#502.
--
-- `source_kind` is intentionally left without a CHECK: #504 adds a `tag` /
-- backpack source, so validity is enforced in `tonies::bindings` instead of
-- freezing the schema. `format`/`mode` follow the 056 convention of being
-- validated in the app rather than via CHECK.
--
-- `source_id` references the user's own playlist; deleting that playlist
-- cascades the binding away. `dirty_since` is set on every create/update so
-- the (future) sync run can detect changes; it is only written here, consumed
-- by #502.

CREATE TABLE hub_tonie_bindings (
    id               INTEGER PRIMARY KEY,
    user_id          INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    tonie_id         TEXT NOT NULL,          -- Kreativ-Tonie ctid (API)
    tonie_name       TEXT,                   -- display name, display/log only
    source_kind      TEXT NOT NULL DEFAULT 'playlist',  -- #500: only 'playlist'
    source_id        INTEGER REFERENCES hub_playlists(id) ON DELETE CASCADE,
    format           TEXT NOT NULL DEFAULT 'mp3-128',   -- 'mp3-128' | 'mp3-320'
    cap_seconds      INTEGER NOT NULL DEFAULT 5400,
    mode             TEXT NOT NULL DEFAULT 'manual',    -- 'manual' | 'auto'
    dirty_since      TEXT,                   -- set on create/update (#502)
    last_sync_at     TEXT,
    last_sync_status TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    UNIQUE(user_id, tonie_id)
);

CREATE INDEX idx_hub_tonie_bindings_user ON hub_tonie_bindings(user_id);

SELECT 'Migration 057 applied: tonie bindings' AS status;
