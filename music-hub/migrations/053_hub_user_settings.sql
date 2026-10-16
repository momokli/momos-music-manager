-- Per-user Hub settings (issue #267).
--
-- Additive migration: one key/value table + index. Per-user configuration
-- (currently `tag_scope`) is authoritative in the Hub, scoped to the owning
-- user. A user without a row reads as the registry default (`own` for
-- `tag_scope`) — no row is materialised. The foreign key cascades on delete so
-- removing a user cleans the settings up with it.
--
-- Additive only: no existing migration is touched (sqlx checksum). At release
-- time this table folds into the earliest new migration of the release.

CREATE TABLE hub_user_settings (
    user_id    INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    key        TEXT    NOT NULL,
    value      TEXT    NOT NULL,
    updated_at TEXT    NOT NULL,
    PRIMARY KEY (user_id, key)
);

CREATE INDEX idx_hub_user_settings_user ON hub_user_settings(user_id);
