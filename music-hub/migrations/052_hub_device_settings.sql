-- Per-device Kellner settings (issue #369, ADR-081 §3/§10.2).
--
-- Additive migration: two new tables + indexes only. Per-device configuration is
-- authoritative in the Hub (not on the device). A device with no settings row
-- reads as the defaults (`flac`, empty template, no active backpacks) — no row is
-- required. Both foreign keys cascade on delete so removing a user / device /
-- backpack cleans the settings up with it.

-- 1:1 scalar settings per device.
CREATE TABLE hub_device_settings (
    device_id         INTEGER PRIMARY KEY REFERENCES hub_devices(id) ON DELETE CASCADE,
    preferred_format  TEXT NOT NULL DEFAULT 'flac'
                      CHECK (preferred_format IN ('flac','mp3','wav','m4a')),
    comment_template  TEXT NOT NULL DEFAULT '',       -- '' = v1 default (alphabetical tag list)
    updated_at        TEXT NOT NULL
);

-- N:N active backpacks per device (order preserved via position).
CREATE TABLE hub_device_backpacks (
    device_id   INTEGER NOT NULL REFERENCES hub_devices(id)   ON DELETE CASCADE,
    backpack_id INTEGER NOT NULL REFERENCES hub_backpacks(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL DEFAULT 0,
    added_at    TEXT NOT NULL,
    PRIMARY KEY (device_id, backpack_id)
);

CREATE INDEX idx_hub_device_backpacks_device   ON hub_device_backpacks(device_id);
CREATE INDEX idx_hub_device_backpacks_backpack ON hub_device_backpacks(backpack_id);
