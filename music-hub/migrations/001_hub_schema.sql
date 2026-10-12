-- MMM Hub — skeleton schema (walking skeleton).
-- Table/view names match plans/mmm-hub/00-interface.md so this converges with
-- the planned M1/M2/M3. Deviations for the skeleton are additive only:
--   * hub_users.slug — CLI-addressable local user handle (OIDC comes in M2).
--   * hub_playlists.items_available — 0 for followed playlists (metadata only).
--   * hub_service_accounts.authorized_at — start of the 6-month refresh clock.
-- Timestamps are ISO-8601 TEXT.

CREATE TABLE hub_users (
    id           INTEGER PRIMARY KEY,
    slug         TEXT NOT NULL UNIQUE,
    display_name TEXT,
    email        TEXT,
    created_at   TEXT NOT NULL
);

CREATE TABLE hub_service_accounts (
    id             INTEGER PRIMARY KEY,
    user_id        INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    service        TEXT NOT NULL CHECK (service IN ('spotify', 'soundcloud', 'youtube')),
    remote_user_id TEXT,
    display_name   TEXT,
    access_token   TEXT,
    refresh_token  TEXT,
    token_expiry   TEXT,
    scopes         TEXT,
    authorized_at  TEXT,
    connected_at   TEXT,
    updated_at     TEXT,
    UNIQUE (user_id, service)
);

CREATE TABLE hub_tracks (
    id               INTEGER PRIMARY KEY,
    service          TEXT NOT NULL,
    service_track_id TEXT NOT NULL,
    isrc             TEXT,
    title            TEXT,
    artists          TEXT,
    album            TEXT,
    duration_ms      INTEGER,
    explicit         INTEGER,
    image_url        TEXT,
    first_seen_at    TEXT,
    UNIQUE (service, service_track_id)
);

CREATE TABLE hub_playlists (
    id              INTEGER PRIMARY KEY,
    user_id         INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    service         TEXT NOT NULL,
    playlist_id     TEXT NOT NULL,
    name            TEXT,
    description     TEXT,
    is_liked        INTEGER NOT NULL DEFAULT 0,
    track_count     INTEGER,
    snapshot_id     TEXT,
    items_available INTEGER NOT NULL DEFAULT 0,
    fetched_at      TEXT,
    UNIQUE (user_id, service, playlist_id)
);

CREATE TABLE hub_playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES hub_playlists(id) ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    position    INTEGER,
    added_at    TEXT,
    PRIMARY KEY (playlist_id, track_id)
);

CREATE TABLE hub_liked_tracks (
    user_id  INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES hub_tracks(id) ON DELETE CASCADE,
    liked_at TEXT,
    PRIMARY KEY (user_id, track_id)
);

CREATE INDEX idx_hub_playlists_user_id            ON hub_playlists(user_id);
CREATE INDEX idx_hub_playlist_tracks_track_id     ON hub_playlist_tracks(track_id);
CREATE INDEX idx_hub_liked_tracks_track_id        ON hub_liked_tracks(track_id);

-- ── Overlap views ────────────────────────────────────────────────────────────

-- Every (track, user, why) fact: liked, or present in one of the user's playlists.
CREATE VIEW hub_v_track_presence AS
SELECT hlt.track_id           AS track_id,
       hlt.user_id            AS user_id,
       'liked'                AS source,
       NULL                   AS playlist_id,
       NULL                   AS playlist_name,
       hlt.liked_at           AS added_at
FROM hub_liked_tracks hlt
UNION ALL
SELECT hpt.track_id, hp.user_id, 'playlist', hp.id, hp.name, hpt.added_at
FROM hub_playlist_tracks hpt
JOIN hub_playlists hp ON hp.id = hpt.playlist_id;

-- track -> (user, playlist) for "who has this where".
CREATE VIEW hub_v_track_playlists AS
SELECT hpt.track_id AS track_id,
       hp.user_id   AS user_id,
       hp.id        AS playlist_id,
       hp.name      AS playlist_name
FROM hub_playlist_tracks hpt
JOIN hub_playlists hp ON hp.id = hpt.playlist_id;

-- Tracks present for >= 2 distinct users (liked or playlisted).
CREATE VIEW hub_v_shared_tracks AS
WITH presence AS (
    SELECT track_id, user_id FROM hub_liked_tracks
    UNION
    SELECT hpt.track_id, hp.user_id
    FROM hub_playlist_tracks hpt
    JOIN hub_playlists hp ON hp.id = hpt.playlist_id
)
SELECT track_id,
       COUNT(DISTINCT user_id)      AS user_count,
       GROUP_CONCAT(DISTINCT user_id) AS user_ids
FROM presence
GROUP BY track_id
HAVING COUNT(DISTINCT user_id) >= 2;

-- Pairwise overlap counts (each unordered pair once, no self-pairs).
CREATE VIEW hub_v_user_overlap AS
WITH presence AS (
    SELECT track_id, user_id FROM hub_liked_tracks
    UNION
    SELECT hpt.track_id, hp.user_id
    FROM hub_playlist_tracks hpt
    JOIN hub_playlists hp ON hp.id = hpt.playlist_id
),
distinct_presence AS (
    SELECT DISTINCT track_id, user_id FROM presence
)
SELECT a.user_id AS user_a_id,
       b.user_id AS user_b_id,
       COUNT(*)  AS shared_tracks
FROM distinct_presence a
JOIN distinct_presence b
  ON a.track_id = b.track_id
 AND a.user_id < b.user_id
GROUP BY a.user_id, b.user_id;
