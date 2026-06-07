CREATE TABLE tag_bundles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    bundle_tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    member_tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    created_at INTEGER DEFAULT (unixepoch()),
    UNIQUE (bundle_tag_id, member_tag_id)
);

CREATE INDEX idx_tag_bundles_bundle ON tag_bundles(bundle_tag_id);
CREATE INDEX idx_tag_bundles_member ON tag_bundles(member_tag_id);

SELECT 'Migration 017 applied: tag_bundles table for bundle/curation tags' as status;
