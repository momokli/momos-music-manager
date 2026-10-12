-- Admins + runtime settings (editable in the web backend).
ALTER TABLE hub_users ADD COLUMN is_admin INTEGER NOT NULL DEFAULT 0;
-- Everyone already present (the three of us) becomes admin.
UPDATE hub_users SET is_admin = 1;

CREATE TABLE hub_settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Registration closed by default; toggleable from the admin page.
INSERT INTO hub_settings (key, value, updated_at)
VALUES ('registration_open', '0', '2026-01-01T00:00:00+00:00');
