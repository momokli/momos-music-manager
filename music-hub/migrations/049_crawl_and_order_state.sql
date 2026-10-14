-- Bounded touch-cascade crawl state + download-priority order ledger
-- (#hub-crawl-everything, plan §2/§3/§6). Additive only.
--
-- `hub_crawl_state` tracks the frontier of the Spotify identity crawl
-- (artist → albums → album tracks) so the worker never re-crawls endlessly:
-- a `done` row is only revisited after its TTL expires.
--
-- `hub_order_state` records the last priority we ordered a track at (music-api),
-- so the priority loop is idempotent and only re-orders on a real change.
CREATE TABLE hub_crawl_state (
    kind       TEXT NOT NULL,                    -- 'artist' | 'album'
    ref        TEXT NOT NULL,                    -- Spotify artist/album id
    state      TEXT NOT NULL DEFAULT 'pending',  -- pending | done | error
    attempts   INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    crawled_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (kind, ref)
);
CREATE INDEX IF NOT EXISTS idx_hub_crawl_state_state ON hub_crawl_state(kind, state);

CREATE TABLE hub_order_state (
    track_id   INTEGER PRIMARY KEY REFERENCES hub_tracks(id) ON DELETE CASCADE,
    priority   INTEGER NOT NULL,
    ordered_at TEXT NOT NULL
);
