-- Accounts: groundwork for magic links (issue #556, `plans/proposed/accounts-mail.md`).
--
-- Purely additive: one new table plus a unique index on an existing column.
--
-- `hub_email_tokens` is the ledger for password resets and invitations. Only the
-- SHA-256 of the token is stored, so a DB leak cannot be replayed; `used_at`
-- makes single-use enforceable and `created_by` records which admin issued a
-- link (audit trail for admin-generated links).
--
-- `hub_users.email` becomes uniquely addressable (case-insensitively) so that
-- `/forgot` resolves an address to exactly one account. `email` stays nullable:
-- accounts without an address keep working with password login only.
--
-- Pre-check before applying on a copied DB: duplicates would make the unique
-- index fail (and a failing migration keeps the service down):
--   SELECT lower(email), count(*) FROM hub_users
--    WHERE email IS NOT NULL GROUP BY 1 HAVING count(*) > 1;

-- An empty string is not an address; normalize before the index exists so a
-- stray `''` cannot collide with itself.
UPDATE hub_users SET email = NULL WHERE email IS NOT NULL AND trim(email) = '';

CREATE UNIQUE INDEX IF NOT EXISTS idx_hub_users_email
    ON hub_users (email COLLATE NOCASE)
    WHERE email IS NOT NULL;

CREATE TABLE IF NOT EXISTS hub_email_tokens (
    id          INTEGER PRIMARY KEY,
    -- NULL for an invitation to an address without an account yet.
    user_id     INTEGER REFERENCES hub_users(id) ON DELETE CASCADE,
    -- The address the token is bound to (the invitation form cannot change it).
    email       TEXT NOT NULL,
    purpose     TEXT NOT NULL CHECK (purpose IN ('register', 'reset')),
    -- SHA-256 (hex) of the plaintext token; the plaintext is never persisted.
    token_hash  TEXT NOT NULL,
    expires_at  TEXT NOT NULL,
    -- Set on redemption; single-use is enforced by `used_at IS NULL` in the
    -- redemption query, not by deleting the row (keeps the audit trail).
    used_at     TEXT,
    -- Admin-issued links record the issuer; self-service links leave it NULL.
    created_by  INTEGER REFERENCES hub_users(id),
    created_at  TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_hub_email_tokens_hash
    ON hub_email_tokens (token_hash);

CREATE INDEX IF NOT EXISTS idx_hub_email_tokens_user
    ON hub_email_tokens (user_id, purpose, created_at);
