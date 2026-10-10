-- Per-user uploads into the music-api object store (Part B).
--
-- The bytes live in the content-addressed store on `.200`; this table records
-- who uploaded which object, so an upload is referenceable per user.
CREATE TABLE hub_uploads (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    hash        TEXT NOT NULL,
    name        TEXT,
    isrc        TEXT,
    group_key   TEXT,
    stem_type   TEXT,
    size        INTEGER,
    stored      INTEGER NOT NULL DEFAULT 1,  -- 1 = new object, 0 = dedup hit
    uploaded_at TEXT NOT NULL
);

CREATE INDEX idx_hub_uploads_user ON hub_uploads(user_id, uploaded_at);
CREATE INDEX idx_hub_uploads_hash ON hub_uploads(hash);
