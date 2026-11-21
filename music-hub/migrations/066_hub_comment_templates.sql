-- Comment-template library (#668, slice C2): user-managed, named bodies for the
-- deterministic Comment rendering/validation defined in the `comment-template`
-- crate (#664). A row is either owned by a user (`owner_user_id`), optionally
-- shared with everyone (`is_shared = 1`), or a system template (`owner_user_id
-- IS NULL`, read-only except for admins).
--
-- Purely additive: no existing table/column is altered.

CREATE TABLE hub_comment_templates (
    id            INTEGER PRIMARY KEY,
    owner_user_id INTEGER REFERENCES hub_users(id) ON DELETE CASCADE, -- NULL = System
    name          TEXT NOT NULL,
    body          TEXT NOT NULL,
    is_shared     INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

-- Names are unique per owner (case-insensitive). Two different users may use the
-- same name; a NULL owner (system) is covered by the partial index below because
-- SQLite treats NULLs as distinct in a plain UNIQUE index.
CREATE UNIQUE INDEX IF NOT EXISTS idx_comment_templates_owner_name
    ON hub_comment_templates(owner_user_id, name COLLATE NOCASE);

-- System templates: one name per system owner, case-insensitive.
CREATE UNIQUE INDEX IF NOT EXISTS idx_comment_templates_system_name
    ON hub_comment_templates(name COLLATE NOCASE)
    WHERE owner_user_id IS NULL;

-- Idempotent system seeds (owner NULL, not shared). The partial unique index on
-- the system name makes `INSERT OR IGNORE` a no-op on re-run. Fixed RFC3339
-- timestamps keep the migration deterministic (no Utc::now()).
INSERT OR IGNORE INTO hub_comment_templates
    (owner_user_id, name, body, is_shared, created_at, updated_at)
VALUES
    (NULL, 'Standard', '{tags} {bpm} {key} {energy}', 0,
     '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00');

INSERT OR IGNORE INTO hub_comment_templates
    (owner_user_id, name, body, is_shared, created_at, updated_at)
VALUES
    (NULL, 'Nur Tags', '{tags}', 0,
     '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00');
