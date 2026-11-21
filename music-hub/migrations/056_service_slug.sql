-- 056 — `hub_service_accounts.service`: Enumeration raus, Form rein.
--
-- Bisher: CHECK (service IN ('spotify','soundcloud','youtube')). Jede neue Quelle
-- (Apple Music, Tidal, Qobuz, …) hätte einen Tabellen-Rebuild erzwungen, weil SQLite
-- einen CHECK nicht ändern kann. Der nächste Rebuild ist der letzte.
--
-- Neu geprüft wird nur die *Form*: kleingeschriebener Slug. Die fachliche Allowlist
-- gehört in den Code (dort, wo geschrieben wird), nicht ins Schema — `hub_playlists`
-- und `hub_tracks` haben ohnehin nie eine gehabt.
--
-- Rebuild-Reihenfolge: neu → kopieren → droppen → umbenennen.
-- Sicher mit aktiven FKs: nichts referenziert diese Tabelle (weder FK noch View);
-- nur 001 (Definition) und 005 (drei ADD COLUMN) erwähnen sie.

CREATE TABLE hub_service_accounts_new (
    id              INTEGER PRIMARY KEY,
    user_id         INTEGER NOT NULL REFERENCES hub_users(id) ON DELETE CASCADE,
    service         TEXT NOT NULL
                    CHECK (length(service) BETWEEN 2 AND 32
                           AND service NOT GLOB '*[^a-z0-9_]*'
                           AND substr(service, 1, 1) GLOB '[a-z]'),
    remote_user_id  TEXT,
    display_name    TEXT,
    access_token    TEXT,
    refresh_token   TEXT,
    token_expiry    TEXT,
    scopes          TEXT,
    authorized_at   TEXT,
    connected_at    TEXT,
    updated_at      TEXT,
    -- aus 005_likes_sync.sql (Reihenfolge = ALTER-Reihenfolge; fehlt eine: stiller Datenverlust)
    likes_synced_at TEXT,
    likes_status    TEXT,
    likes_error     TEXT,
    -- Ziel von `ON CONFLICT(user_id, service)` in ingest.rs — muss erhalten bleiben
    UNIQUE (user_id, service)
);

INSERT INTO hub_service_accounts_new
    (id, user_id, service, remote_user_id, display_name, access_token, refresh_token,
     token_expiry, scopes, authorized_at, connected_at, updated_at,
     likes_synced_at, likes_status, likes_error)
SELECT id, user_id, service, remote_user_id, display_name, access_token, refresh_token,
       token_expiry, scopes, authorized_at, connected_at, updated_at,
       likes_synced_at, likes_status, likes_error
  FROM hub_service_accounts;

DROP TABLE hub_service_accounts;

ALTER TABLE hub_service_accounts_new RENAME TO hub_service_accounts;

SELECT 'Migration 056 applied: service is a slug, not an enumeration' AS status;
