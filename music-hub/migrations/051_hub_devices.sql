-- Hub-side per-device token registry (issue #363, ADR-081 §10.2).
--
-- Each kellner (device) generates two tokens locally and sends only hashes to
-- the hub: a durable *device token* (future Bearer credential, #364/#365) and a
-- one-time *pairing token* embedded in the approval URL. While a row is
-- pending/unbound (`user_id IS NULL`) `token_hash` holds the pairing hash and
-- `pending_token_hash` stages the device-token hash. On approval the pairing
-- hash is rotated to the device-token hash (single-use guarantee).
--
-- Additive migration: creates a new table + indexes only.

CREATE TABLE hub_devices (
    id                  INTEGER PRIMARY KEY,
    user_id             INTEGER REFERENCES hub_users(id) ON DELETE CASCADE, -- NULL while pending
    name                TEXT NOT NULL,                                      -- device label (hostname)
    token_hash          TEXT NOT NULL,                                      -- sha256 hex: pairing hash while pending, then device-token hash
    pending_token_hash  TEXT,                                               -- sha256 hex of device token held until binding (NULL once bound)
    created_at          TEXT NOT NULL,
    last_seen_at        TEXT,                                               -- touched by device-gated routes (#364+)
    paired_at           TEXT,                                               -- set on binding
    expires_at          TEXT,                                               -- pre-binding TTL; NULL after binding
    revoked_at          TEXT
);

CREATE UNIQUE INDEX idx_hub_devices_token_hash ON hub_devices(token_hash);
CREATE INDEX idx_hub_devices_user ON hub_devices(user_id);
