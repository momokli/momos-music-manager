-- Spelunke — web accounts + sessions (walking skeleton, simple username/password).
-- Additive to 001: adds a password hash to users and a server-side session table.

ALTER TABLE hub_users ADD COLUMN password_hash TEXT;

CREATE TABLE hub_web_sessions (
    id         TEXT PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX idx_hub_web_sessions_user ON hub_web_sessions(user_id);
