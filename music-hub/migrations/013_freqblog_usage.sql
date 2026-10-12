-- Per-provider API quota tracking. FreqBlog's free tier is 1,000 requests a
-- month, so the backfill must never exceed a configurable cap. One row per
-- (provider, calendar month, UTC).
CREATE TABLE hub_api_usage (
    provider   TEXT NOT NULL,
    period     TEXT NOT NULL,          -- 'YYYY-MM' (UTC)
    used       INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT,
    PRIMARY KEY (provider, period)
);
CREATE INDEX idx_hub_api_usage_period ON hub_api_usage(period);
