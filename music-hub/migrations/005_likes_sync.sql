-- Liked-tracks sync state per account (driven by the backend worker).
ALTER TABLE hub_service_accounts ADD COLUMN likes_synced_at TEXT;
ALTER TABLE hub_service_accounts ADD COLUMN likes_status TEXT;
ALTER TABLE hub_service_accounts ADD COLUMN likes_error TEXT;
