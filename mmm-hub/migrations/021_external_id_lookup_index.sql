-- Speed up external-id lookups (digging candidate matching does
-- `WHERE service = 'spotify' AND external_id = ?`, previously a full scan).
CREATE INDEX IF NOT EXISTS idx_hub_track_ext_ids_service
    ON hub_track_external_ids(service, external_id);
