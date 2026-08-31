-- Migration 023: Tag inbox staging table (tag roundtrip inbox — full feature set)
--
-- The tag inbox is where a NEW tag (one that is not yet canonically part of the
-- system) is edited BEFORE it is written into file comments:
--
--   * rename  — fix the spelling of a new/typo tag (raw_tag → target_tag)
--   * merge   — fold a typo tag into an existing canonical tag
--               (raw_tag → target_tag, target must exist in `tags`)
--   * dismiss — acknowledge the tag without any mapping effect
--
-- Staging semantics: a mapping row is ONLY a decision record. Nothing is
-- applied to comments or to the `tags` vocabulary at resolve time. The next
-- comment write (write-comment task / sync path) consults the open mappings
-- and writes the canonical (mapped) spelling instead of the raw tag. The exact
-- typo wording therefore disappears from every written comment; it survives
-- only here (inbox history) and in the inbox UI until the files are written.
--
-- Keys are normalized to lowercase (comments store tags lowercased via
-- `parse_comment`), so the table is keyed case-insensitively via
-- `UNIQUE COLLATE NOCASE`.

CREATE TABLE IF NOT EXISTS tag_inbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- The new/typo tag exactly as it appears in comments (lowercase, trimmed).
    raw_tag TEXT NOT NULL COLLATE NOCASE UNIQUE,
    -- 'rename' | 'merge' | 'dismiss'
    action TEXT NOT NULL DEFAULT 'rename',
    -- Canonical spelling to write instead of raw_tag. For 'dismiss' this is
    -- the raw_tag itself (no effect). Lowercase, trimmed.
    target_tag TEXT NOT NULL,
    -- 'open' (active decision) | 'applied' (written) | 'dismissed'
    status TEXT NOT NULL DEFAULT 'open',
    created_at INTEGER DEFAULT (unixepoch()),
    resolved_at INTEGER,
    -- Files affected at decision time (informational, for the UI badge).
    file_count INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_tag_inbox_status ON tag_inbox(status);
CREATE INDEX IF NOT EXISTS idx_tag_inbox_raw ON tag_inbox(raw_tag);

SELECT 'Migration 023 applied: tag_inbox staging table' as status;
