-- Case-insensitive usernames: enforce uniqueness ignoring case.
CREATE UNIQUE INDEX IF NOT EXISTS idx_hub_users_slug_nocase ON hub_users(slug COLLATE NOCASE);
